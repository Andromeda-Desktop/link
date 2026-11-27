package org.andromeda.link.core.domain

/** What a desktop asks of this phone's screen while mirroring. Coordinates are fractions of the screen in units of
 * 1/[MIRROR_UNIT]; a scroll's (x2, y2) is a signed delta. */
sealed interface MirrorCommand {
    val desktopId: String

    /** Show Android's capture prompt; start on consent, or refuse. */
    data class Requested(
        override val desktopId: String,
    ) : MirrorCommand

    data class Input(
        override val desktopId: String,
        val action: MirrorAction,
        val x: Int?,
        val y: Int?,
        val x2: Int?,
        val y2: Int?,
        val ms: Int?,
        val text: String?,
        val code: Int? = null,
        val meta: Int? = null,
    ) : MirrorCommand

    /** The desktop lost a frame: encode a keyframe next. */
    data class Keyframe(
        override val desktopId: String,
    ) : MirrorCommand

    /** How late the desktop receives the video, in [lagMs]; the bitrate steps down while it is high. */
    data class Lag(
        override val desktopId: String,
        val lagMs: Int,
    ) : MirrorCommand

    data class Stopped(
        override val desktopId: String,
        val reason: String?,
    ) : MirrorCommand
}

enum class MirrorAction {
    Tap,
    LongPress,
    Swipe,
    Scroll,
    Back,
    Home,
    Recents,
    Text,
    TouchDown,
    TouchMove,
    TouchUp,
    Wheel,
    KeyDown,
    KeyUp,
    ScreenOff,
    ScreenOn,
    Screenshot,
}

const val MIRROR_UNIT = 10_000

/** A mirror video codec besides H.264, with its MediaCodec type. */
enum class VideoCodec(
    val mime: String,
) {
    HEVC("video/hevc"),
}

/** The video stream to one desktop, from [org.andromeda.link.core.data.LinkRepository.startMirror]. */
interface MirrorStream {
    /** What [send] takes: H.264 when null. */
    val codec: VideoCodec?

    /** Camera video only: the quarter turns clockwise, 0 to 3, that make the picture upright; each unit carries them. */
    var quarterTurns: Int

    /** One access unit in [codec], Annex B; suspends while the network is behind. */
    suspend fun send(
        ptsUs: Long,
        keyframe: Boolean,
        config: Boolean,
        data: ByteArray,
    ): Result<Unit>

    /** Sends 20 ms frames of 16-bit PCM at 48 kHz with [channels] as Opus at [bitrate] bit/s: stereo in a mirror
     * and for the phone's sound, mono on a microphone stream. */
    fun audio(
        channels: Int,
        bitrate: Int,
    ): Result<AudioUnits>

    suspend fun finish()
}

/** From [MirrorStream.audio]. */
interface AudioUnits {
    /** One 20 ms frame captured at [ptsUs]; returns at once, fails once the desktop is gone. */
    fun push(
        ptsUs: Long,
        pcm: ByteArray,
    ): Result<Unit>

    /** Takes no more frames; returns once the queued ones are sent, at most 500 ms later. */
    suspend fun close()
}

/** The screen's rounded-corner radii (top-left, top-right, bottom-right, bottom-left; empty when unknown) and camera
 * cutouts, in pixels of a [width] by [height] screen as it is turned now. */
data class ScreenShape(
    val width: Int,
    val height: Int,
    val corners: List<Int>,
    val cutouts: List<android.graphics.Rect>,
)
