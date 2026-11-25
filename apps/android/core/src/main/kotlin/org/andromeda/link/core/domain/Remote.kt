package org.andromeda.link.core.domain

import kotlinx.coroutines.flow.SharedFlow

/** A monitor or window a desktop shares for remote control. */
data class RemoteSource(
    val name: String,
    val window: Boolean,
)

/** One unit of the desktop's stream: H.264 in Annex B form, or with [audio] one 48 kHz stereo Opus packet. */
class RemoteUnit(
    val ptsUs: Long,
    val flags: Int,
    val data: ByteArray,
) {
    val config get() = flags and MIRROR_CONFIG != 0
    val keyframe get() = flags and MIRROR_KEYFRAME != 0
    val audio get() = flags and MIRROR_AUDIO != 0
}

/** The desktop's screen and sound, from [RemoteEvent.Stream]. */
interface RemoteVideo {
    /** Null once the desktop ended the stream or [stop] ran. */
    suspend fun next(): RemoteUnit?

    suspend fun stop()
}

/** Desktops' screens for this phone: what a viewer needs of the repository. */
interface RemoteScreens {
    /** Desktops' answers to this phone's requests for their screens. */
    val remote: SharedFlow<RemoteEvent>

    /** Asks the desktop for its screen, or to switch to [source] of its last [RemoteEvent.Started]. */
    suspend fun startRemote(
        desktopId: String,
        source: Int?,
    ): Result<Unit>

    suspend fun stopRemote(desktopId: String): Result<Unit>
}

/** A desktop's sound for this phone's speaker: Opus units (48 kHz stereo, [RemoteUnit.audio]) until it ends. */
class SpeakerStream(
    val desktopId: String,
    val sound: RemoteVideo,
)

/** What a desktop answers this phone's request for its screen. */
sealed interface RemoteEvent {
    val desktopId: String

    data class Started(
        override val desktopId: String,
        val sources: List<RemoteSource>,
        val current: Int,
    ) : RemoteEvent

    class Stream(
        override val desktopId: String,
        val width: Int,
        val height: Int,
        val video: RemoteVideo,
    ) : RemoteEvent

    data class Stopped(
        override val desktopId: String,
        val reason: String?,
    ) : RemoteEvent
}

const val MIRROR_KEYFRAME = 1
const val MIRROR_CONFIG = 2
const val MIRROR_AUDIO = 4
