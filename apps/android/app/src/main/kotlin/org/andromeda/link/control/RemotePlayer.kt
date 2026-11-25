package org.andromeda.link.control

import android.media.MediaCodec
import android.media.MediaFormat
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
    codec.configure(MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, stream.width, stream.height), surface, null, 0)
    codec.start()
    // The sound runs beside the picture; a full queue drops the oldest packets, so the sound never falls behind.
    val sound = Channel<RemoteUnit>(SOUND_QUEUE, BufferOverflow.DROP_OLDEST)
    try {
        coroutineScope {
            launch { playSound(sound) }
            while (coroutineContext.isActive) {
                val unit = stream.video.next() ?: break
                if (unit.audio) {
                    if (!muted()) sound.trySend(unit)
                } else {
                    decode(codec, unit)
                }
            }
            sound.close()
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
    while (index < 0) {
        render(codec)
        index = codec.dequeueInputBuffer(INPUT_WAIT_US)
    }
    codec.getInputBuffer(index)?.apply {
        clear()
        put(unit.data)
    }
    val flags = if (unit.config) MediaCodec.BUFFER_FLAG_CODEC_CONFIG else 0
    codec.queueInputBuffer(index, 0, unit.data.size, unit.ptsUs, flags)
    render(codec)
}

/** Shows every decoded frame at once: the picture is live, not timed. */
private fun render(codec: MediaCodec) {
    val info = MediaCodec.BufferInfo()
    while (true) {
        val index = codec.dequeueOutputBuffer(info, 0)
        if (index >= 0) {
            codec.releaseOutputBuffer(index, true)
        } else if (index != MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
            return
        }
    }
}

private const val TAG = "RemotePlayer"
private const val INPUT_WAIT_US = 10_000L
private const val SOUND_QUEUE = 25
