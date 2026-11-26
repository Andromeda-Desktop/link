package org.andromeda.link.core.domain

/** A desktop asks for this phone's microphone, or ended it. */
sealed interface MicCommand {
    val desktopId: String

    data class Requested(
        override val desktopId: String,
    ) : MicCommand

    data class Stopped(
        override val desktopId: String,
    ) : MicCommand
}
