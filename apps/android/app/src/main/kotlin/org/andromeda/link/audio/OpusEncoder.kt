package org.andromeda.link.audio

import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log

/**
 * Records [record] (16-bit PCM at [RATE] with [channels] channels) and encodes it as Opus in 20 ms packets, each handed
 * to [send], until [stopping] or a failed send. Starts and releases [record]. Blocks, so it runs on an IO thread.
 */
suspend fun encodeOpus(
    record: AudioRecord,
    channels: Int,
    bitrate: Int,
    stopping: () -> Boolean,
    send: suspend (ptsUs: Long, data: ByteArray) -> Result<Unit>,
) {
    val codec =
        try {
            MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS).apply {
                val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, RATE, channels)
                format.setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
                configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                start()
            }
        } catch (error: Exception) {
            Log.w(TAG, "no Opus encoder: ${error.message}")
            record.release()
            return
        }
    try {
        record.startRecording()
        val packetBytes = packetBytes(channels)
        val info = MediaCodec.BufferInfo()
        while (!stopping()) {
            val input = codec.dequeueInputBuffer(TIMEOUT_US)
            if (input >= 0) {
                val buffer = codec.getInputBuffer(input) ?: continue
                val read = record.read(buffer, minOf(buffer.capacity(), packetBytes), AudioRecord.READ_BLOCKING)
                codec.queueInputBuffer(input, 0, maxOf(read, 0), System.nanoTime() / 1000, 0)
            }
            while (true) {
                val output = codec.dequeueOutputBuffer(info, 0)
                if (output < 0) break
                val bytes = ByteArray(info.size)
                codec.getOutputBuffer(output)?.apply {
                    position(info.offset)
                    get(bytes)
                }
                codec.releaseOutputBuffer(output, false)
                if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0 || bytes.isEmpty()) continue
                if (send(info.presentationTimeUs, bytes).isFailure) return
            }
        }
    } finally {
        runCatching { record.stop() }
        record.release()
        runCatching { codec.stop() }
        codec.release()
    }
}

/** 20 ms of 16-bit PCM: one Opus packet. */
fun packetBytes(channels: Int) = RATE / 50 * channels * 2

const val RATE = 48_000
private const val TAG = "OpusEncoder"
private const val TIMEOUT_US = 20_000L
