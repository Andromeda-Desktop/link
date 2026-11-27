package org.andromeda.link.screen

import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.hardware.display.VirtualDisplayConfig
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import android.view.Display
import android.view.Surface
import android.view.WindowManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.IntentCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.andromeda.link.LinkApplication
import org.andromeda.link.R
import org.andromeda.link.core.domain.MirrorStream
import org.andromeda.link.notifications.Channels
import org.andromeda.link.notifications.NotificationId
import org.lsposed.hiddenapibypass.HiddenApiBypass
import java.lang.reflect.Method

/**
 * Captures the screen into the hardware H.264 encoder and streams its output to one desktop: at most 1440 pixels on
 * the short side, 60 fps, realtime priority and low latency, constant bitrate stepped by [MirrorRate], a keyframe
 * every 2 s or on the desktop's request. Codec configuration goes in front of every keyframe, so the desktop can
 * start from any of them. What apps play goes along through [MirrorAudio]. In [CaptureMode.Sound] only the sound goes,
 * with no display or encoder.
 */
class ScreenCapture : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val main = Handler(Looper.getMainLooper())
    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null

    @Volatile
    private var encoder: VideoEncoder? = null

    /** The largest short side the link carries now; see [shortSide]. */
    @Volatile
    private var shortSide = MAX_SHORT_SIDE

    /** The encoder's current width and height. */
    private var size = 0 to 0

    @Volatile
    private var resizing = false
    private var stream: MirrorStream? = null

    @Volatile
    private var rate: MirrorRate? = null
    private var audio: MirrorAudio? = null
    private var desktop: String? = null
    private var mode = CaptureMode.Mirror

    @Volatile
    private var stopping = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        if (intent?.action == ACTION_STOP) {
            finish(tellDesktop = true, reason = "stopped on the phone")
            return START_NOT_STICKY
        }
        val desktop = intent?.getStringExtra(ScreenMirror.EXTRA_DESKTOP)
        val data = intent?.let { IntentCompat.getParcelableExtra(it, EXTRA_DATA, Intent::class.java) }
        if (desktop == null || data == null || running) {
            stopSelf()
            return START_NOT_STICKY
        }
        this.desktop = desktop
        mode = CaptureMode.of(intent.getStringExtra(EXTRA_MODE))
        val repository = (application as LinkApplication).container.repository
        val name =
            repository.desktops.value
                .firstOrNull { it.id == desktop }
                ?.name ?: desktop
        // Android requires the foreground service before it hands out the projection.
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(name), ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        instance = this
        running = true
        val projection =
            getSystemService(MediaProjectionManager::class.java)
                .getMediaProjection(intent.getIntExtra(EXTRA_CODE, 0), data)
        if (projection == null) {
            finish(tellDesktop = true, reason = "Android refused the capture")
            return START_NOT_STICKY
        }
        projection.registerCallback(
            object : MediaProjection.Callback() {
                override fun onStop() = finish(tellDesktop = true, reason = "stopped on the phone")
            },
            main,
        )
        this.projection = projection
        InputCommandProvider.send("wifi-latency", code = 1)
        scope.launch {
            when (mode) {
                CaptureMode.Mirror -> capture(desktop, projection)
                CaptureMode.Sound -> sound(desktop, projection)
            }
        }
        return START_NOT_STICKY
    }

    private suspend fun sound(
        desktop: String,
        projection: MediaProjection,
    ) {
        val repository = (application as LinkApplication).container.repository
        val stream =
            repository.startSound(desktop).getOrElse {
                return finish(tellDesktop = false, reason = null)
            }
        this.stream = stream
        val audio = MirrorAudio(this, projection)
        this.audio = audio
        audio.run(stream)
        // run returns at a stop, a failed stream or a missing permission: nothing more plays.
        main.post { finish(tellDesktop = true, reason = null) }
    }

    private suspend fun capture(
        desktop: String,
        projection: MediaProjection,
    ) {
        val (width, height) = videoSize()
        val repository = (application as LinkApplication).container.repository
        val stream =
            repository.startMirror(desktop, width, height, VideoEncoder.hardwareCodecs(width, height)).getOrElse {
                return finish(tellDesktop = false, reason = null)
            }
        this.stream = stream
        val mime = stream.codec?.mime ?: MediaFormat.MIMETYPE_VIDEO_AVC
        Log.i(TAG, "mirroring in $mime")
        var encoder = encoder(mime, width, height) ?: return
        display = mirrorDisplay(projection, width, height, encoder.surface)
        sendShape(desktop)
        val audio = MirrorAudio(this, projection)
        this.audio = audio
        scope.launch { audio.run(stream) }
        getSystemService(DisplayManager::class.java).registerDisplayListener(rotation, main)
        val rate = MirrorRate(MAX_BITRATE, MIN_BITRATE)
        this.rate = rate
        // A rotation ends the encoder's stream; a new encoder at the turned size goes on in the same stream, its
        // codec configuration in front of its first keyframe as always.
        while (true) {
            val end = encoder.drain(stream, rate, { stopping }, ::stepSize)
            if (end == VideoEncoder.End.Failed) main.post { finish(tellDesktop = false, reason = null) }
            if (end == VideoEncoder.End.Codec) main.post { finish(tellDesktop = true, reason = "the phone's $mime encoder failed") }
            if (end != VideoEncoder.End.Input || stopping) break
            val (turnedWidth, turnedHeight) = videoSize()
            encoder.release()
            encoder = encoder(mime, turnedWidth, turnedHeight) ?: return
            display?.resize(turnedWidth, turnedHeight, resources.displayMetrics.densityDpi)
            display?.surface = encoder.surface
            resizing = false
            Log.i(TAG, "resized: ${turnedWidth}x$turnedHeight")
            sendShape(desktop)
            // A turn or size step during the rebuild was ignored; catch up with it now.
            if (videoSize() != size) {
                resizing = true
                encoder.endInput()
            }
        }
    }

    /**
     * Encoders such as the OnePlus 12's ignore a lower bitrate, so a smaller picture is what sends less. Up only with a
     * third to spare, so a bitrate near a threshold does not flip the size back and forth.
     */
    private fun stepSize(bitrate: Int) {
        val side = shortSide(bitrate).let { if (it > shortSide) maxOf(shortSide, shortSide(bitrate * 2 / 3)) else it }
        if (side != shortSide && !resizing) {
            shortSide = side
            resizing = true
            encoder?.endInput()
        }
    }

    /** A started [mime] encoder for [width] by [height]; null after telling the desktop why not. */
    private fun encoder(
        mime: String,
        width: Int,
        height: Int,
    ): VideoEncoder? {
        val encoder =
            try {
                // At the smallest size the link is slowest: half the frames, dropped before the encoder.
                VideoEncoder.start(mime, width, height, FPS, MAX_BITRATE, halfRate = minOf(width, height) <= MIN_SHORT_SIDE)
            } catch (error: Exception) {
                Log.w(TAG, "no $mime encoder: ${error.message}")
                main.post { finish(tellDesktop = true, reason = "the phone has no $mime encoder for ${width}x$height") }
                return null
            }
        this.encoder = encoder
        size = width to height
        return encoder
    }

    private val rotation =
        object : DisplayManager.DisplayListener {
            override fun onDisplayChanged(displayId: Int) {
                if (displayId != android.view.Display.DEFAULT_DISPLAY || resizing || videoSize() == size) return
                resizing = true
                encoder?.endInput()
            }

            override fun onDisplayAdded(displayId: Int) = Unit

            override fun onDisplayRemoved(displayId: Int) = Unit
        }

    /**
     * The display the encoder records. MediaProjection's public call makes it at 60 Hz, so a 120 Hz screen mirrored at
     * half its rate. Only a hidden call takes the screen's own rate; if Android refuses it, the public one.
     */
    private fun mirrorDisplay(
        projection: MediaProjection,
        width: Int,
        height: Int,
        surface: Surface,
    ): VirtualDisplay? {
        val dpi = resources.displayMetrics.densityDpi
        val flags = DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR
        val displays = getSystemService(DisplayManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            runCatching {
                val config =
                    VirtualDisplayConfig
                        .Builder("Link mirror", width, height, dpi)
                        .setFlags(flags)
                        .setSurface(surface)
                        .setRequestedRefreshRate(displays.getDisplay(Display.DEFAULT_DISPLAY).refreshRate)
                        .build()
                val create =
                    HiddenApiBypass.getDeclaredMethods(DisplayManager::class.java).filterIsInstance<Method>().first {
                        it.name == "createVirtualDisplay" &&
                            it.parameterTypes.contentEquals(
                                arrayOf(
                                    MediaProjection::class.java,
                                    VirtualDisplayConfig::class.java,
                                    VirtualDisplay.Callback::class.java,
                                    Handler::class.java,
                                ),
                            )
                    }
                return create.invoke(displays, projection, config, null, null) as VirtualDisplay
            }.onFailure { Log.i(TAG, "mirror display at the screen's rate: ${it.cause ?: it}") }
        }
        return projection.createVirtualDisplay("Link mirror", width, height, dpi, flags, surface, null, null)
    }

    /** The short side for a [bitrate]: full size on a fast link, smaller ones when the link is behind. */
    private fun shortSide(bitrate: Int) =
        when {
            bitrate >= MAX_SIZE_BITRATE -> MAX_SHORT_SIDE
            bitrate >= FULL_SIZE_BITRATE -> FULL_SHORT_SIDE
            bitrate >= MID_SIZE_BITRATE -> MID_SHORT_SIDE
            else -> MIN_SHORT_SIDE
        }

    /**
     * The screen scaled to at most [shortSide] on its short side, in multiples of 16 as encoders want, and down to a size
     * the H.264 encoder takes: every desktop accepts H.264, and other codecs are offered only at this size.
     */
    private fun videoSize(): Pair<Int, Int> {
        val (width, height) =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                getSystemService(WindowManager::class.java).maximumWindowMetrics.bounds.let { it.width() to it.height() }
            } else {
                resources.displayMetrics.let { it.widthPixels to it.heightPixels }
            }
        var scale = minOf(1.0, shortSide.toDouble() / minOf(width, height))

        fun fit(side: Int) = (side * scale).toInt() / ALIGN * ALIGN
        while (fit(minOf(width, height)) > MIN_SHORT_SIDE && !VideoEncoder.takes(MediaFormat.MIMETYPE_VIDEO_AVC, fit(width), fit(height))) {
            scale *= ENCODER_STEP
        }
        return fit(width) to fit(height)
    }

    /** Off the capture path: the frame on the desktop is only looks, so a failure is logged, not fatal. */
    private fun sendShape(desktop: String) {
        val repository = (application as LinkApplication).container.repository
        scope.launch {
            repository.sendScreenShape(desktop, screenShape(this@ScreenCapture)).onFailure { Log.w(TAG, "screen shape: ${it.message}") }
        }
    }

    private fun keyframe() {
        encoder?.keyframe()
    }

    private fun finish(
        tellDesktop: Boolean,
        reason: String?,
    ) {
        if (stopping) return
        stopping = true
        // The desktop turns the panel back on at a clean stop only; this covers a lost desktop.
        if (mode == CaptureMode.Mirror) InputCommandProvider.send("screen-on")
        InputCommandProvider.send("wifi-latency", code = 0)
        getSystemService(DisplayManager::class.java).unregisterDisplayListener(rotation)
        audio?.stop()
        display?.release()
        encoder?.release()
        projection?.stop()
        val (desktop, stream, mode) = Triple(desktop, stream, mode)
        val repository = (application as LinkApplication).container.repository
        scope.launch {
            stream?.finish()
            if (tellDesktop && desktop != null) {
                when (mode) {
                    CaptureMode.Mirror -> repository.stopMirror(desktop, reason)
                    CaptureMode.Sound -> repository.stopSound(desktop)
                }
            }
            scope.cancel()
        }
        running = false
        instance = null
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        finish(tellDesktop = true, reason = "stopped on the phone")
        super.onDestroy()
    }

    private fun notification(name: String) =
        NotificationCompat
            .Builder(this, Channels.SCREEN)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle(
                getString(
                    when (mode) {
                        CaptureMode.Mirror -> R.string.screen_mirroring
                        CaptureMode.Sound -> R.string.sound_playing
                    },
                    name,
                ),
            ).setOngoing(true)
            .setSilent(true)
            .addAction(
                0,
                getString(R.string.screen_stop),
                PendingIntent.getService(
                    this,
                    0,
                    Intent(this, ScreenCapture::class.java).setAction(ACTION_STOP),
                    PendingIntent.FLAG_IMMUTABLE,
                ),
            ).build()

    companion object {
        @Volatile
        var running = false
            private set

        @Volatile
        private var instance: ScreenCapture? = null

        fun start(
            context: Context,
            desktop: String,
            mode: CaptureMode,
            code: Int,
            data: Intent,
        ) {
            val intent =
                Intent(context, ScreenCapture::class.java)
                    .putExtra(ScreenMirror.EXTRA_DESKTOP, desktop)
                    .putExtra(EXTRA_MODE, mode.name)
                    .putExtra(EXTRA_CODE, code)
                    .putExtra(EXTRA_DATA, data)
            context.startForegroundService(intent)
        }

        fun requestKeyframe() {
            instance?.keyframe()
        }

        fun reportLag(lagMs: Int) {
            instance?.rate?.reported(lagMs * 1000L, System.nanoTime() / 1000)
        }

        /** The desktop stopped its [mode] capture; nothing is sent back to it. */
        fun stop(
            context: Context,
            desktop: String,
            mode: CaptureMode,
        ) {
            val capture = instance ?: return
            if (capture.desktop == desktop && capture.mode == mode) {
                Handler(context.mainLooper).post { capture.finish(tellDesktop = false, reason = null) }
            }
        }

        /** Ends a sound-only capture and tells its desktop, so a mirror can take the one projection; false when none runs. */
        fun endSound(context: Context): Boolean {
            val capture = instance?.takeIf { it.mode == CaptureMode.Sound } ?: return false
            Handler(context.mainLooper).post { capture.finish(tellDesktop = true, reason = null) }
            return true
        }

        private const val TAG = "ScreenCapture"
        private const val ACTION_STOP = "org.andromeda.link.screen.STOP"
        private const val EXTRA_CODE = "code"
        private const val EXTRA_DATA = "data"
        const val EXTRA_MODE = "mode"
        private val NOTIFICATION_ID = NotificationId.ScreenCapture.id

        // The OnePlus 12's own panel, 1440 by 3168, sent unscaled on a fast link.
        private const val MAX_SHORT_SIDE = 1440
        private const val FULL_SHORT_SIDE = 1080
        private const val MID_SHORT_SIDE = 720
        private const val MIN_SHORT_SIDE = 480
        private const val MAX_SIZE_BITRATE = 12_000_000
        private const val FULL_SIZE_BITRATE = 6_000_000
        private const val MID_SIZE_BITRATE = 2_500_000
        private const val ALIGN = 16
        private const val ENCODER_STEP = 0.95
        private const val MAX_BITRATE = 24_000_000
        private const val MIN_BITRATE = 1_000_000
        private const val FPS = 60
    }
}

/** What one projection carries to its desktop: the screen with its sound, or the sound alone. */
enum class CaptureMode {
    Mirror,
    Sound,
    ;

    companion object {
        /** The mode named in an intent extra; a start without one is a mirror. */
        fun of(name: String?) = entries.firstOrNull { it.name == name } ?: Mirror
    }
}
