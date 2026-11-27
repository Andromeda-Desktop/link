package org.andromeda.link.control

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.util.Log
import android.view.Surface
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.andromeda.link.core.domain.RemoteEvent
import org.andromeda.link.core.domain.RemoteUnit
import kotlin.coroutines.coroutineContext

/**
 * Plays a desktop's stream: H.264 into [surface] through the hardware decoder, and its Opus sound through an
 * [AudioTrack] unless [muted] says otherwise. Runs until the stream ends or the caller is cancelled.
 */
suspend fun play(
    stream: RemoteEvent.Stream,
    surface: Surface,
    muted: () -> Boolean,
) = withContext(Dispatchers.IO) {
    Log.i(TAG, "decoding ${stream.width}x${stream.height}")
    val codec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
    val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, stream.width, stream.height)
    // Without it a decoder may hold several frames before the first output.
    val lowLatency =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            codec.codecInfo
                .getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC)
                .isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency)
    if (lowLatency) format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
    // Qualcomm's decoders hold frames to reorder them unless told the stream's order; the desktop sends no B-frames.
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && QTI_PICTURE_ORDER in codec.supportedVendorParameters) {
        format.setInteger(QTI_PICTURE_ORDER, 1)
    }
    Log.i(TAG, "${codec.name} low latency $lowLatency")
    codec.configure(format, surface, null, 0)
    codec.start()
    // The sound runs beside the picture; a full queue drops the oldest packets, so the sound never falls behind.
    val sound = Channel<RemoteUnit>(SOUND_QUEUE, BufferOverflow.DROP_OLDEST)
    try {
        coroutineScope {
            launch { playSound(sound) }
            // Each frame shows as soon as it is decoded, not when the next unit comes in.
            val shown = launch { show(codec) }
            while (coroutineContext.isActive) {
                val unit = stream.video.next() ?: break
                if (unit.audio) {
                    if (!muted()) sound.trySend(unit)
                } else {
                    decode(codec, unit)
                }
            }
            sound.close()
            shown.cancel()
        }
    } finally {
        codec.stop()
        codec.release()
    }
}

private fun decode(
    codec: MediaCodec,
    unit: RemoteUnit,
) {
    var index = codec.dequeueInputBuffer(INPUT_WAIT_US)
    while (index < 0) index = codec.dequeueInputBuffer(INPUT_WAIT_US)
    codec.getInputBuffer(index)?.apply {
        clear()
        put(unit.data)
    }
    val flags = if (unit.config) MediaCodec.BUFFER_FLAG_CODEC_CONFIG else 0
    codec.queueInputBuffer(index, 0, unit.data.size, unit.ptsUs, flags)
}

/** Shows every decoded frame at once: the picture is live, not timed. Runs until cancelled. */
private suspend fun show(codec: MediaCodec) {
    val info = MediaCodec.BufferInfo()
    while (coroutineContext.isActive) {
        val index = codec.dequeueOutputBuffer(info, OUTPUT_WAIT_US)
        if (index >= 0) codec.releaseOutputBuffer(index, true)
    }
}

private const val TAG = "RemotePlayer"
private const val INPUT_WAIT_US = 10_000L
private const val OUTPUT_WAIT_US = 10_000L
private const val QTI_PICTURE_ORDER = "vendor.qti-ext-dec-picture-order.enable"
private const val SOUND_QUEUE = 25
