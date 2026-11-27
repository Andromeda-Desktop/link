package org.andromeda.link.screen

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.Surface
import org.andromeda.link.core.domain.MirrorStream
import org.andromeda.link.core.domain.VideoCodec

/**
 * One hardware encoder recording its input [surface]: realtime priority, low latency, constant bitrate stepped by
 * [MirrorRate], no B-frames, a keyframe every 2 s or on request. [drain] puts the codec configuration in front of every
 * keyframe, so the desktop can start from any of them. The screen mirror and the webcam share it.
 */
class VideoEncoder private constructor(
    private val codec: MediaCodec,
    val surface: Surface,
) {
    /** Why [drain] returned: the input ended ([endInput]), the capture stops, the stream failed, or the codec did. */
    enum class End { Input, Stopped, Failed, Codec }

    /**
     * Sends the encoder's output on [stream] until it ends; [onBitrate] hears each bitrate step after it applies. With
     * [clockPts] each unit is stamped when the encoder gives it out, not with its input's time: camera timestamps
     * follow no fixed clock (the A7 Pro's run 1000 times fast), and the lag checks compare them with this clock.
     */
    suspend fun drain(
        stream: MirrorStream,
        rate: MirrorRate,
        stopping: () -> Boolean,
        onBitrate: (Int) -> Unit,
        clockPts: Boolean = false,
    ): End {
        val info = MediaCodec.BufferInfo()
        var config: ByteArray? = null
        while (!stopping()) {
            val index =
                try {
                    codec.dequeueOutputBuffer(info, DEQUEUE_TIMEOUT_US)
                } catch (error: MediaCodec.CodecException) {
                    Log.w(TAG, "encoder failed: ${error.diagnosticInfo}")
                    return End.Codec
                } catch (_: IllegalStateException) {
                    return End.Stopped
                }
            if (index < 0) continue
            val bytes = ByteArray(info.size)
            codec.getOutputBuffer(index)?.apply {
                position(info.offset)
                get(bytes)
            }
            codec.releaseOutputBuffer(index, false)
            if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) return End.Input
            if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                config = bytes
                continue
            }
            val keyframe = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
            if (rate.shouldSkip(keyframe)) continue
            val prefix = config.takeIf { keyframe }
            val unit = if (prefix != null) prefix + bytes else bytes
            val ptsUs = if (clockPts) System.nanoTime() / 1000 else info.presentationTimeUs
            if (stream.send(ptsUs, keyframe, prefix != null, unit).isFailure) return End.Failed
            val wasSkipping = rate.skipping
            rate.sent(ptsUs, System.nanoTime() / 1000, keyframe)?.let { bitrate ->
                Log.i(TAG, "bitrate ${bitrate / 1000} kbit/s")
                setBitrate(bitrate)
                onBitrate(bitrate)
            }
            if (rate.skipping && !wasSkipping) {
                Log.i(TAG, "far behind: skipping to the next keyframe")
                keyframe()
            }
        }
        return End.Stopped
    }

    /** Ends the input, so [drain] returns [End.Input] once the encoder has sent what it holds. */
    fun endInput() {
        runCatching { codec.signalEndOfInputStream() }
    }

    fun keyframe() {
        try {
            codec.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        } catch (error: IllegalStateException) {
            Log.i(TAG, "keyframe: ${error.message}")
        }
    }

    fun release() {
        runCatching { codec.stop() }
        runCatching { codec.release() }
    }

    private fun setBitrate(bitrate: Int) {
        try {
            codec.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, bitrate) })
        } catch (error: IllegalStateException) {
            Log.i(TAG, "bitrate: ${error.message}")
        }
    }

    companion object {
        /**
         * A started [mime] encoder for [width] by [height] at [fps] and [bitrate]; with [halfRate], half the frames
         * are dropped before it. Throws when the phone has no such encoder.
         */
        fun start(
            mime: String,
            width: Int,
            height: Int,
            fps: Int,
            bitrate: Int,
            halfRate: Boolean,
        ): VideoEncoder {
            val codec = encoderInfo(mime)?.let { MediaCodec.createByCodecName(it.name) } ?: MediaCodec.createEncoderByType(mime)
            try {
                codec.configure(format(codec, mime, width, height, fps, bitrate, halfRate), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                val surface = codec.createInputSurface()
                codec.start()
                return VideoEncoder(codec, surface)
            } catch (error: Exception) {
                codec.release()
                throw error
            }
        }

        /** The encoder [start] uses for [mime]: the first one listed, as createEncoderByType would pick. */
        private fun encoderInfo(mime: String): MediaCodecInfo? =
            MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.firstOrNull { info ->
                info.isEncoder && info.supportedTypes.any { it.equals(mime, ignoreCase = true) }
            }

        /** Whether the [mime] encoder [start] uses takes [width] by [height]; encoders such as MediaTek's stop at 1080p. */
        fun takes(
            mime: String,
            width: Int,
            height: Int,
        ): Boolean = encoderInfo(mime)?.getCapabilitiesForType(mime)?.videoCapabilities?.isSizeSupported(width, height) ?: false

        /**
         * The codecs besides H.264 this phone has a hardware encoder for at [width] by [height] in either orientation: a
         * software encoder adds tens of ms a frame, worse than H.264's larger frames.
         */
        fun hardwareCodecs(
            width: Int,
            height: Int,
        ): List<VideoCodec> {
            val encoders = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter { it.isEncoder && it.isHardwareAccelerated }
            return VideoCodec.entries.filter { codec ->
                encoders.any { info ->
                    info.supportedTypes.any { it.equals(codec.mime, ignoreCase = true) } &&
                        info.getCapabilitiesForType(codec.mime).videoCapabilities.let {
                            it.isSizeSupported(width, height) && it.isSizeSupported(height, width)
                        }
                }
            }
        }

        private fun format(
            codec: MediaCodec,
            mime: String,
            width: Int,
            height: Int,
            fps: Int,
            bitrate: Int,
            halfRate: Boolean,
        ) = MediaFormat.createVideoFormat(mime, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            if (halfRate) setFloat("max-fps-to-encoder", (fps / 2).toFloat())
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, KEYFRAME_EVERY_S)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            setInteger(MediaFormat.KEY_OPERATING_RATE, Short.MAX_VALUE.toInt())
            setInteger(MediaFormat.KEY_LATENCY, 1)
            setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            // A still picture sends nothing otherwise, and the desktop would wait on a frame the encoder holds back.
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, REPEAT_AFTER_US)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            // Qualcomm encoders take their low-latency mode only by this vendor key; others ignore it.
            setInteger("vendor.qti-ext-enc-low-latency.enable", 1)
            val capabilities = codec.codecInfo.getCapabilitiesForType(mime)
            // Constrained Baseline cannot reorder frames; the desktop relies on that to show each frame as it is decoded.
            // HEVC Main has no such profile: KEY_MAX_B_FRAMES 0 keeps it in order.
            val profile =
                when (mime) {
                    MediaFormat.MIMETYPE_VIDEO_AVC -> MediaCodecInfo.CodecProfileLevel.AVCProfileConstrainedBaseline
                    else -> MediaCodecInfo.CodecProfileLevel.HEVCProfileMain
                }
            if (capabilities.profileLevels.any { it.profile == profile }) setInteger(MediaFormat.KEY_PROFILE, profile)
            val modes = capabilities.encoderCapabilities
            if (modes.isBitrateModeSupported(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)) {
                setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            }
        }

        private const val TAG = "VideoEncoder"
        private const val REPEAT_AFTER_US = 100_000L
        private const val KEYFRAME_EVERY_S = 2
        private const val DEQUEUE_TIMEOUT_US = 20_000L
    }
}
