package org.andromeda.link.core.domain

/** A desktop asks for a camera of this phone as its webcam, ended it, or lost video and needs a keyframe. */
sealed interface CameraCommand {
    val desktopId: String

    data class Requested(
        override val desktopId: String,
        val front: Boolean,
        val width: Int,
        val height: Int,
        val fps: Int,
    ) : CameraCommand

    data class Stopped(
        override val desktopId: String,
    ) : CameraCommand

    data class Keyframe(
        override val desktopId: String,
    ) : CameraCommand
}
