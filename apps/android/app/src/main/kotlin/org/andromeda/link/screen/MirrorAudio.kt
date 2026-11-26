package org.andromeda.link.screen

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.projection.MediaProjection
import android.util.Log
import org.andromeda.link.audio.RATE
import org.andromeda.link.audio.encodeOpus
import org.andromeda.link.audio.packetBytes
import org.andromeda.link.core.domain.MirrorStream

/**
 * What apps play, captured through the screen's projection and encoded as 48 kHz stereo Opus, in 20 ms packets on
 * the video's stream. Needs the microphone permission, which Android requires for any playback capture; without it
 * the mirror is silent. Apps that opt out of capture stay silent too.
 */
class MirrorAudio(
    private val context: Context,
    private val projection: MediaProjection,
) {
    @Volatile
    private var stopping = false

    /** Captures and sends until [stop] or the stream fails; blocks, so it runs on an IO thread. */
    suspend fun run(stream: MirrorStream) {
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            Log.i(TAG, "no microphone permission: mirroring without sound")
            return
        }
        val record = record() ?: return
        encodeOpus(record, CHANNELS, BITRATE, { stopping }, stream)
    }

    fun stop() {
        stopping = true
    }

    @SuppressLint("MissingPermission")
    private fun record(): AudioRecord? {
        val capture =
            AudioPlaybackCaptureConfiguration
                .Builder(projection)
                .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                .addMatchingUsage(AudioAttributes.USAGE_GAME)
                .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                .build()
        val format =
            AudioFormat
                .Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(RATE)
                .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                .build()
        val minimum = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT)
        return try {
            AudioRecord
                .Builder()
                .setAudioFormat(format)
                .setBufferSizeInBytes(maxOf(minimum, packetBytes(CHANNELS)) * 4)
                .setAudioPlaybackCaptureConfig(capture)
                .build()
        } catch (error: Exception) {
            Log.w(TAG, "no playback capture: ${error.message}")
            null
        }
    }

    private companion object {
        const val TAG = "MirrorAudio"
        const val CHANNELS = 2
        const val BITRATE = 96_000
    }
}
