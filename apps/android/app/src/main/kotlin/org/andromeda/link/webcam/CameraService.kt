package org.andromeda.link.webcam

import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.OrientationEventListener
import android.view.Surface
import androidx.camera.core.CameraSelector
import androidx.camera.core.Preview
import androidx.camera.core.SurfaceRequest
import androidx.camera.core.resolutionselector.AspectRatioStrategy
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.andromeda.link.LinkApplication
import org.andromeda.link.R
import org.andromeda.link.capture.CaptureRequests
import org.andromeda.link.capture.desktopName
import org.andromeda.link.core.domain.CameraCommand
import org.andromeda.link.core.domain.MirrorStream
import org.andromeda.link.notifications.Channels
import org.andromeda.link.notifications.NotificationId
import org.andromeda.link.screen.MirrorRate
import org.andromeda.link.screen.VideoEncoder

/**
 * One camera of this phone as one desktop's webcam: CameraX draws into the shared [VideoEncoder]'s surface, whose
 * units go on the camera stream until Stop here, the desktop's stop, a camera error, or the link's end. One desktop
 * at a time. The picture keeps the sensor's orientation; each unit tells the desktop how far to turn it upright.
 */
class CameraService :
    Service(),
    LifecycleOwner {
    private val registry = LifecycleRegistry(this)
    override val lifecycle: Lifecycle get() = registry
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val repository get() = (application as LinkApplication).container.repository

    /** The service's one mode: Idle -> Opening -> Streaming, and from any of them to Stopped. */
    private sealed interface State {
        val desktop: String? get() = null

        data object Idle : State

        data class Opening(
            override val desktop: String,
        ) : State

        data class Streaming(
            override val desktop: String,
            val encoder: VideoEncoder,
            val stream: MirrorStream,
            val size: Size,
        ) : State

        data object Stopped : State
    }

    @Volatile
    private var state: State = State.Idle

    /** Follows how the phone is held while a camera is bound, so CameraX reports the turn that makes it upright. */
    private var turning: OrientationEventListener? = null

    /** The bound camera, so a switch rebinds [preview] to the other one; main thread only. */
    private var provider: ProcessCameraProvider? = null
    private var preview: Preview? = null
    private var front = false

    /** The only writer of [state] and [instance]; returns the state it left, or null for an invalid move. */
    @Synchronized
    private fun move(to: State): State? {
        val from = state
        val valid =
            when (to) {
                is State.Opening -> from == State.Idle
                is State.Streaming -> (from as? State.Opening)?.desktop == to.desktop
                State.Stopped -> from != State.Stopped
                State.Idle -> false
            }
        if (!valid) return null
        state = to
        if (to is State.Opening) instance = this
        if (to == State.Stopped && instance === this) instance = null
        return from
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        registry.currentState = Lifecycle.State.CREATED
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        if (intent?.action == ACTION_STOP) {
            finish(tellDesktop = true, reason = null)
            return START_NOT_STICKY
        }
        if (intent?.action == ACTION_SWITCH) {
            switchTo(!front)
            return START_NOT_STICKY
        }
        val desktop = intent?.getStringExtra(CaptureRequests.EXTRA_DESKTOP)
        if (desktop == null) {
            if (state == State.Idle) stopSelf()
            return START_NOT_STICKY
        }
        val current = state.desktop
        if (move(State.Opening(desktop)) == null) {
            if (current != desktop) scope.launch { repository.stopCamera(desktop, "busy") }
            return START_NOT_STICKY
        }
        // Android 10 has no camera type and no while-in-use limit on the camera.
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(desktopName(repository, desktop)), type)
        registry.currentState = Lifecycle.State.STARTED
        val front = intent.getBooleanExtra(EXTRA_FRONT, false)
        val size = Size(intent.getIntExtra(EXTRA_WIDTH, WIDTH), intent.getIntExtra(EXTRA_HEIGHT, HEIGHT))
        val fps = intent.getIntExtra(EXTRA_FPS, FPS)
        val provider = ProcessCameraProvider.getInstance(this)
        provider.addListener({
            val cameras =
                try {
                    provider.get()
                } catch (error: Exception) {
                    Log.w(TAG, "no camera provider: ${error.message}")
                    finish(tellDesktop = true, reason = "the phone's camera service failed")
                    return@addListener
                }
            bind(cameras, front, size, fps)
        }, mainExecutor)
        return START_NOT_STICKY
    }

    private fun bind(
        provider: ProcessCameraProvider,
        front: Boolean,
        size: Size,
        fps: Int,
    ) {
        if (state !is State.Opening) return
        val preview =
            Preview
                .Builder()
                .setResolutionSelector(
                    ResolutionSelector
                        .Builder()
                        // CameraX prefers 4:3 unless told; a 16:9 request would get 640x480.
                        .setAspectRatioStrategy(
                            if (size.width * 3 == size.height * 4) {
                                AspectRatioStrategy.RATIO_4_3_FALLBACK_AUTO_STRATEGY
                            } else {
                                AspectRatioStrategy.RATIO_16_9_FALLBACK_AUTO_STRATEGY
                            },
                        ).setResolutionStrategy(ResolutionStrategy(size, ResolutionStrategy.FALLBACK_RULE_CLOSEST_LOWER_THEN_HIGHER))
                        .build(),
                ).setTargetFrameRate(Range(fps, fps))
                .build()
        // A switch while streaming hands the new camera the same encoder, so the desktop's stream goes on.
        preview.setSurfaceProvider(mainExecutor) { request ->
            if (state is State.Streaming) reuse(request) else scope.launch { stream(request, fps) }
        }
        this.provider = provider
        this.preview = preview
        turning =
            object : OrientationEventListener(this) {
                override fun onOrientationChanged(degrees: Int) {
                    if (degrees == ORIENTATION_UNKNOWN) return
                    val rotation =
                        when (degrees) {
                            in 45 until 135 -> Surface.ROTATION_270
                            in 135 until 225 -> Surface.ROTATION_180
                            in 225 until 315 -> Surface.ROTATION_90
                            else -> Surface.ROTATION_0
                        }
                    if (rotation != preview.targetRotation) {
                        Log.i(TAG, "phone held at $degrees degrees: target rotation $rotation")
                        preview.targetRotation = rotation
                    }
                }
            }.apply { enable() }
        if (!bindCamera(front)) {
            finish(tellDesktop = true, reason = "the phone has no ${if (front) "front" else "back"} camera")
        }
    }

    /** Binds [preview] to the [front] or back camera; false when the phone has none. */
    private fun bindCamera(front: Boolean): Boolean {
        val provider = provider ?: return false
        val preview = preview ?: return false
        val selector = if (front) CameraSelector.DEFAULT_FRONT_CAMERA else CameraSelector.DEFAULT_BACK_CAMERA
        val camera =
            try {
                provider.bindToLifecycle(this, selector, preview)
            } catch (error: Exception) {
                Log.w(TAG, "no ${if (front) "front" else "back"} camera: ${error.message}")
                return false
            }
        this.front = front
        camera.cameraInfo.cameraState.observe(this) { state ->
            state.error?.let {
                Log.w(TAG, "camera error ${it.code} (${state.type})")
                finish(tellDesktop = true, reason = "the camera is in use or failed (${it.code})")
            }
        }
        return true
    }

    /** Moves a running stream to the [front] or back camera; the stream, its encoder and the desktop's node stay. */
    private fun switchTo(front: Boolean) {
        val streaming = state as? State.Streaming ?: return
        if (front == this.front) return
        Log.i(TAG, "${streaming.desktop}: switching to the ${if (front) "front" else "back"} camera")
        preview?.let { provider?.unbind(it) }
        if (!bindCamera(front) && !bindCamera(!front)) {
            finish(tellDesktop = true, reason = "the phone's camera failed")
        }
    }

    /** The surface request of a camera switched to while streaming: the running encoder takes it at the same size. */
    private fun reuse(request: SurfaceRequest) {
        val streaming = state as? State.Streaming
        if (streaming == null || request.resolution != streaming.size) {
            Log.w(TAG, "the ${if (front) "front" else "back"} camera gives ${request.resolution}, not ${streaming?.size}")
            request.willNotProvideSurface()
            if (streaming != null) main.post { switchTo(!front) }
            return
        }
        request.provideSurface(streaming.encoder.surface, mainExecutor) { if (state !is State.Streaming) streaming.encoder.release() }
        watchTurns(request, streaming.desktop, streaming.stream)
        streaming.encoder.keyframe()
    }

    /** Each unit carries the turn CameraX reports for [request]'s camera. */
    private fun watchTurns(
        request: SurfaceRequest,
        desktop: String,
        stream: MirrorStream,
    ) {
        request.setTransformationInfoListener(mainExecutor) {
            Log.i(TAG, "$desktop: picture turns ${it.rotationDegrees} degrees")
            stream.quarterTurns = it.rotationDegrees / 90 % 4
        }
    }

    /** Opens the stream at the size CameraX picked, then hands it the encoder's surface and sends until the end. */
    private suspend fun stream(
        request: SurfaceRequest,
        fps: Int,
    ) {
        val desktop = (state as? State.Opening)?.desktop
        if (desktop == null) {
            request.willNotProvideSurface()
            return
        }
        val (width, height) = request.resolution.let { it.width to it.height }
        val stream =
            repository.startCamera(desktop, width, height, VideoEncoder.hardwareCodecs(width, height)).getOrElse {
                Log.i(TAG, "$desktop: no camera stream: ${it.message}")
                request.willNotProvideSurface()
                main.post { finish(tellDesktop = true, reason = "the phone could not open the camera stream") }
                return
            }
        val mime = stream.codec?.mime ?: MediaFormat.MIMETYPE_VIDEO_AVC
        val encoder =
            try {
                check(VideoEncoder.takes(mime, width, height)) { "$mime encoder takes no ${width}x$height" }
                VideoEncoder.start(mime, width, height, fps, BITRATE, halfRate = false)
            } catch (error: Exception) {
                Log.w(TAG, "no $mime encoder: ${error.message}")
                request.willNotProvideSurface()
                withContext(NonCancellable) { stream.finish() }
                main.post { finish(tellDesktop = true, reason = "the phone has no $mime encoder for ${width}x$height") }
                return
            }
        if (move(State.Streaming(desktop, encoder, stream, Size(width, height))) == null) {
            request.willNotProvideSurface()
            encoder.release()
            withContext(NonCancellable) { stream.finish() }
            return
        }
        // The camera lets go of the surface only after it stops drawing; the encoder goes then, not before.
        // The camera lets go of the surface only after it stops drawing; a switch hands it on, so only the end releases.
        request.provideSurface(encoder.surface, mainExecutor) { if (state !is State.Streaming) encoder.release() }
        // The encoder takes the sensor's picture as it is; the desktop turns it upright by the turns each unit carries.
        watchTurns(request, desktop, stream)
        Log.i(TAG, "$desktop: webcam on at ${width}x$height $fps fps in $mime")
        val end =
            try {
                encoder.drain(stream, MirrorRate(BITRATE, MIN_BITRATE), { state == State.Stopped }, {}, clockPts = true)
            } finally {
                withContext(NonCancellable) { stream.finish() }
            }
        Log.i(TAG, "$desktop: webcam off ($end)")
        val reason = if (end == VideoEncoder.End.Codec) "the phone's $mime encoder failed" else null
        main.post { finish(tellDesktop = end != VideoEncoder.End.Failed, reason = reason) }
    }

    /** Runs on the main thread; [tellDesktop] is false when the desktop ended it or the link is gone. */
    private fun finish(
        tellDesktop: Boolean,
        reason: String?,
    ) {
        val from = move(State.Stopped) ?: return
        turning?.disable()
        turning = null
        provider = null
        preview = null
        // CameraX unbinds a destroyed owner's use cases and closes the camera.
        registry.currentState = Lifecycle.State.DESTROYED
        val desktop = from.desktop
        if (tellDesktop && desktop != null) scope.launch { repository.stopCamera(desktop, reason) }
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        finish(tellDesktop = true, reason = null)
        registry.currentState = Lifecycle.State.DESTROYED
        super.onDestroy()
    }

    private fun notification(name: String) =
        NotificationCompat
            .Builder(this, Channels.WEBCAM)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle(getString(R.string.webcam_running, name))
            .setOngoing(true)
            .setSilent(true)
            .addAction(
                0,
                getString(R.string.webcam_stop),
                PendingIntent.getService(
                    this,
                    0,
                    Intent(this, CameraService::class.java).setAction(ACTION_STOP),
                    PendingIntent.FLAG_IMMUTABLE,
                ),
            ).addAction(
                0,
                getString(R.string.webcam_switch),
                PendingIntent.getService(
                    this,
                    1,
                    Intent(this, CameraService::class.java).setAction(ACTION_SWITCH),
                    PendingIntent.FLAG_IMMUTABLE,
                ),
            ).build()

    companion object {
        /** The service while it is Opening or Streaming; written only by [move]. */
        @Volatile
        private var instance: CameraService? = null
        private val main = Handler(Looper.getMainLooper())

        val running get() = instance != null

        /** Copies [request] into [intent], which [start] later reads. */
        fun request(
            intent: Intent,
            request: CameraCommand.Requested,
        ): Intent =
            intent
                .putExtra(CaptureRequests.EXTRA_DESKTOP, request.desktopId)
                .putExtra(EXTRA_FRONT, request.front)
                .putExtra(EXTRA_WIDTH, request.width)
                .putExtra(EXTRA_HEIGHT, request.height)
                .putExtra(EXTRA_FPS, request.fps)

        /** Starts the service with the request [from] carries; call only while the app is visible. */
        fun start(
            context: Context,
            from: Intent,
        ) {
            context.startForegroundService(Intent(context, CameraService::class.java).putExtras(from))
        }

        /** The desktop stopped; nothing is sent back to it. */
        fun stop(desktop: String) {
            main.post { instance?.takeIf { it.state.desktop == desktop }?.finish(tellDesktop = false, reason = null) }
        }

        /** Moves [desktop]'s running stream to the [front] or back camera; false when none runs for it. */
        fun switch(
            desktop: String,
            front: Boolean,
        ): Boolean {
            val service = instance?.takeIf { it.state is State.Streaming && it.state.desktop == desktop } ?: return false
            main.post { service.switchTo(front) }
            return true
        }

        /** Moves [desktop]'s running stream to the other camera. */
        fun switchOther(desktop: String) {
            main.post {
                val service = instance?.takeIf { it.state is State.Streaming && it.state.desktop == desktop }
                service?.switchTo(!service.front)
            }
        }

        fun keyframe(desktop: String) {
            main.post {
                val streaming = instance?.state as? State.Streaming
                if (streaming?.desktop == desktop) streaming.encoder.keyframe()
            }
        }

        // Not "CameraService": Android's own camera service logs under that tag.
        private const val TAG = "LinkWebcam"
        private const val ACTION_STOP = "org.andromeda.link.webcam.STOP"
        private const val ACTION_SWITCH = "org.andromeda.link.webcam.SWITCH"
        private const val EXTRA_FRONT = "front"
        private const val EXTRA_WIDTH = "width"
        private const val EXTRA_HEIGHT = "height"
        private const val EXTRA_FPS = "fps"
        private val NOTIFICATION_ID = NotificationId.Webcam.id
        private const val WIDTH = 1280
        private const val HEIGHT = 720
        private const val FPS = 30
        private const val BITRATE = 4_000_000
        private const val MIN_BITRATE = 1_000_000
    }
}
