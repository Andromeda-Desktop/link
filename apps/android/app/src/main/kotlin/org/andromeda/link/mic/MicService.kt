package org.andromeda.link.mic

import android.Manifest
import android.annotation.SuppressLint
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.andromeda.link.LinkApplication
import org.andromeda.link.R
import org.andromeda.link.audio.RATE
import org.andromeda.link.audio.encodeOpus
import org.andromeda.link.audio.packetBytes
import org.andromeda.link.notifications.Channels
import org.andromeda.link.notifications.NotificationId

/**
 * This phone's microphone for one desktop: the phone's voice processing (echo and noise suppression), 48 kHz mono Opus
 * in 20 ms packets, until Stop here, the desktop's stop, or the link's end. One desktop at a time.
 */
class MicService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var desktop: String? = null

    @Volatile
    private var stopping = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        if (intent?.action == ACTION_STOP) {
            finish(tellDesktop = true)
            return START_NOT_STICKY
        }
        val desktop = intent?.getStringExtra(PhoneMic.EXTRA_DESKTOP)
        if (desktop == null || running) {
            if (!running) stopSelf()
            return START_NOT_STICKY
        }
        this.desktop = desktop
        val repository = (application as LinkApplication).container.repository
        val name =
            repository.desktops.value
                .firstOrNull { it.id == desktop }
                ?.name ?: desktop
        // Android 10 has no microphone type and no while-in-use limit on the microphone.
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(name), type)
        instance = this
        running = true
        scope.launch {
            val stream =
                repository.startMic(desktop).getOrElse {
                    Log.i(TAG, "$desktop: no microphone stream: ${it.message}")
                    null
                }
            if (stream != null) {
                Log.i(TAG, "$desktop: microphone on")
                try {
                    record()?.let { encodeOpus(it, CHANNELS, BITRATE, { stopping }, stream) }
                } finally {
                    withContext(NonCancellable) { stream.finish() }
                }
                Log.i(TAG, "$desktop: microphone off")
            }
            main.post { finish(tellDesktop = true) }
        }
        return START_NOT_STICKY
    }

    @SuppressLint("MissingPermission")
    private fun record(): AudioRecord? {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            Log.i(TAG, "no microphone permission")
            return null
        }
        val minimum = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val record =
            try {
                AudioRecord
                    .Builder()
                    .setAudioSource(MediaRecorder.AudioSource.VOICE_COMMUNICATION)
                    .setAudioFormat(
                        AudioFormat
                            .Builder()
                            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                            .setSampleRate(RATE)
                            .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                            .build(),
                    ).setBufferSizeInBytes(maxOf(minimum, packetBytes(CHANNELS)) * 4)
                    .build()
            } catch (error: Exception) {
                Log.w(TAG, "no microphone: ${error.message}")
                return null
            }
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            Log.w(TAG, "the microphone did not open")
            record.release()
            return null
        }
        return record
    }

    /** Runs on the main thread; [tellDesktop] is false when the desktop ended it. */
    private fun finish(tellDesktop: Boolean) {
        if (!running || instance != this) return
        running = false
        instance = null
        stopping = true
        val desktop = desktop
        val repository = (application as LinkApplication).container.repository
        if (tellDesktop && desktop != null) scope.launch { repository.stopMic(desktop) }
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        // The encoder sees `stopping` within one 20 ms read and ends the stream itself; the scope then holds no work.
        finish(tellDesktop = true)
        super.onDestroy()
    }

    private fun notification(name: String) =
        NotificationCompat
            .Builder(this, Channels.MIC)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle(getString(R.string.mic_running, name))
            .setOngoing(true)
            .setSilent(true)
            .addAction(
                0,
                getString(R.string.mic_stop),
                PendingIntent.getService(
                    this,
                    0,
                    Intent(this, MicService::class.java).setAction(ACTION_STOP),
                    PendingIntent.FLAG_IMMUTABLE,
                ),
            ).build()

    companion object {
        @Volatile
        var running = false
            private set

        @Volatile
        private var instance: MicService? = null
        private val main = Handler(Looper.getMainLooper())

        fun start(
            context: Context,
            desktop: String,
        ) {
            context.startForegroundService(Intent(context, MicService::class.java).putExtra(PhoneMic.EXTRA_DESKTOP, desktop))
        }

        /** The desktop stopped; nothing is sent back to it. */
        fun stop(
            context: Context,
            desktop: String,
        ) {
            main.post {
                val service = instance ?: return@post
                if (service.desktop == desktop) service.finish(tellDesktop = false)
            }
        }

        private const val TAG = "MicService"
        private const val ACTION_STOP = "org.andromeda.link.mic.STOP"
        private val NOTIFICATION_ID = NotificationId.Mic.id
        private const val CHANNELS = 1
        private const val BITRATE = 32_000
    }
}
