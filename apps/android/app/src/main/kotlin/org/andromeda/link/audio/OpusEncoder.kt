package org.andromeda.link.audio

import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import kotlin.math.abs

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
        val probe = if (Log.isLoggable(TAG, Log.DEBUG)) OnsetProbe(record, channels) else null
        while (!stopping()) {
            val input = codec.dequeueInputBuffer(TIMEOUT_US)
            var wait = 0L
            if (input >= 0) {
                val buffer = codec.getInputBuffer(input) ?: continue
                val read = record.read(buffer, minOf(buffer.capacity(), packetBytes), AudioRecord.READ_BLOCKING)
                probe?.read(buffer, read)
                codec.queueInputBuffer(input, 0, maxOf(read, 0), System.nanoTime() / 1000, 0)
                // Its packet goes now, not after the next 20 ms read.
                wait = OUTPUT_WAIT_US
            }
            while (true) {
                val output = codec.dequeueOutputBuffer(info, wait)
                wait = 0
                if (output < 0) break
                val bytes = ByteArray(info.size)
                codec.getOutputBuffer(output)?.apply {
                    position(info.offset)
                    get(bytes)
                }
                codec.releaseOutputBuffer(output, false)
                if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0 || bytes.isEmpty()) continue
                if (send(info.presentationTimeUs, bytes).isFailure) return
                probe?.sent()
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
private const val OUTPUT_WAIT_US = 5_000L
private const val LOUD = 2000

/**
 * Debug timing of the sound path (setprop log.tag.OpusEncoder DEBUG): for the first loud frame after a quiet packet,
 * logs the wall-clock ms it entered the recorder (AudioRecord timestamp), when its read returned and when its packet
 * was sent.
 */
private class OnsetProbe(
    private val record: AudioRecord,
    private val channels: Int,
) {
    private var frames = 0L
    private var quiet = true
    private var pending = false

    fun read(
        buffer: java.nio.ByteBuffer,
        bytes: Int,
    ) {
        if (bytes <= 0) return
        val pcm = buffer.duplicate().order(java.nio.ByteOrder.LITTLE_ENDIAN)
        val count = bytes / (2 * channels)
        val loud = (0 until count).firstOrNull { abs(pcm.getShort(it * 2 * channels).toInt()) > LOUD }
        if (loud != null && quiet) {
            val stamp = android.media.AudioTimestamp()
            val entered =
                if (record.getTimestamp(stamp, android.media.AudioTimestamp.TIMEBASE_MONOTONIC) == AudioRecord.SUCCESS) {
                    val ns = stamp.nanoTime + (frames + loud - stamp.framePosition) * 1_000_000_000L / RATE
                    wall(ns)
                } else {
                    Double.NaN
                }
            Log.d(TAG, "onset entered %.1f read %.1f".format(entered, wall(System.nanoTime())))
            pending = true
        }
        quiet = loud == null
        frames += count
    }

    fun sent() {
        if (!pending) return
        pending = false
        Log.d(TAG, "onset sent %.1f".format(wall(System.nanoTime())))
    }

    private fun wall(monotonicNs: Long) = System.currentTimeMillis() + (monotonicNs - System.nanoTime()) / 1e6
}
