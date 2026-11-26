package org.andromeda.link.notifications

/** Every untagged notification Link posts: one entry each, so no feature replaces another's notification. */
enum class NotificationId {
    Presence,
    Ring,
    Transfer,
    MirrorRequest,
    ScreenCapture,
    ScreenControl,
    MirrorAsk,
    SoundRequest,
    MicRequest,
    Mic,
    Camera,
    Trash,
    ;

    /** Android refuses 0 for a foreground service. */
    val id get() = ordinal + 1
}
