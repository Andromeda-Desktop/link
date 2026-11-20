package org.andromeda.link.core.domain

enum class ShareKind { Text, Link }

/** Queued: the share waits on the phone and goes out when the desktop is back. */
enum class ShareDelivery { Delivered, Queued }

/** Text or a link a desktop sent, already checked by the Rust core. */
data class IncomingShare(
    val desktopId: String,
    val desktopName: String,
    val kind: ShareKind,
    val text: String,
)

/** A single http or https URL is shared as a link, so the desktop can offer to open it; anything else is text. */
fun shareKindOf(text: String): ShareKind {
    val lower = text.lowercase()
    val web = lower.startsWith("http://") || lower.startsWith("https://")
    return if (web && text.none(Char::isWhitespace)) ShareKind.Link else ShareKind.Text
}
