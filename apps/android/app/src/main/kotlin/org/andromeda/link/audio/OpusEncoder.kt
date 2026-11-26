package org.andromeda.link.audio

import android.media.AudioRecord
import android.util.Log
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import org.andromeda.link.core.domain.MirrorStream
import java.nio.ByteBuffer
import kotlin.math.abs

/**
 * Records [record] (16-bit PCM at [RATE] with [channels] channels) and sends it in 20 ms frames to [stream], which
 * encodes them as Opus at [bitrate], until [stopping] or a failed send. Starts and releases [record]. Blocks, so it runs
 * on an IO thread.
 */
suspend fun encodeOpus(
    record: AudioRecord,
    channels: Int,
    bitrate: Int,
    stopping: () -> Boolean,
    stream: MirrorStream,
) {
    val units =
        stream.audio(channels, bitrate).getOrElse {
            Log.w(TAG, "no Opus encoder: ${it.message}")
            record.release()
            return
        }
    try {
        record.startRecording()
        val pcm = ByteArray(packetBytes(channels))
        val probe = if (Log.isLoggable(TAG, Log.DEBUG)) OnsetProbe(record, channels) else null
        while (!stopping()) {
            var filled = 0
            while (filled < pcm.size && !stopping()) {
                val read = record.read(pcm, filled, pcm.size - filled, AudioRecord.READ_BLOCKING)
                if (read < 0) {
                    Log.w(TAG, "the recorder failed: $read")
                    return
                }
                filled += read
            }
            if (filled < pcm.size) return
            probe?.read(ByteBuffer.wrap(pcm), filled)
            if (units.push(System.nanoTime() / 1000, pcm).isFailure) return
            probe?.sent()
        }
    } finally {
        runCatching { record.stop() }
        record.release()
        withContext(NonCancellable) { units.close() }
    }
}

/** 20 ms of 16-bit PCM: one Opus packet. */
fun packetBytes(channels: Int) = RATE / 50 * channels * 2

const val RATE = 48_000
private const val TAG = "OpusEncoder"
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
