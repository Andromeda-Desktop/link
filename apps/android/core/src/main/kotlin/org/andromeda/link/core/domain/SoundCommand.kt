package org.andromeda.link.core.domain

/** A desktop asks for this phone's media sound without its screen, or ended it. */
sealed interface SoundCommand {
    val desktopId: String

    data class Requested(
        override val desktopId: String,
    ) : SoundCommand

    data class Stopped(
        override val desktopId: String,
    ) : SoundCommand
}
