package org.andromeda.link.ui.kit

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp

/** The one stroke icon set of the approved mockups (design-link/phone/icons.js): 24 grid, 2 px stroke, round caps. */
object Icons {
    val Play by lazy { icon("play", "M7 4v16l13-8z") }
    val Call by lazy {
        icon(
            "call",
            "M22 16.9v3a2 2 0 0 1-2.2 2 19.8 19.8 0 0 1-8.6-3.1 19.5 19.5 0 0 1-6-6A19.8 19.8 0 0 1 2.1 4.2 2 2 0 " +
                "0 1 4.1 2h3a2 2 0 0 1 2 1.7c.1.9.4 1.8.7 2.7a2 2 0 0 1-.5 2.1L8 9.8a16 16 0 0 0 6 6l1.3-1.3a2 2 0 0 " +
                "1 2.1-.4c.9.3 1.8.6 2.7.7a2 2 0 0 1 1.7 2z",
        )
    }
    val Folder by lazy { icon("folder", "M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z") }
    val Msg by lazy { icon("msg", "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z") }
    val Screen by lazy { icon("screen", "M8 2h8a2 2 0 0 1 2 2v16a2 2 0 0 1 -2 2h-8a2 2 0 0 1 -2 -2v-16a2 2 0 0 1 2 -2z M9 18h6") }
    val Term by lazy { icon("term", "m4 17 6-5-6-5M12 19h8") }
    val Ping by lazy { icon("ping", "M12 2v4M12 18v4M4.9 4.9l2.8 2.8M16.3 16.3l2.8 2.8M2 12h4M18 12h4") }
    val Sliders by lazy { icon("sliders", "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6") }
    val Shield by lazy { icon("shield", "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z") }
    val Battery by lazy { icon("battery", "M4 7h14a2 2 0 0 1 2 2v6a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-6a2 2 0 0 1 2 -2z M22 11v2") }
    val Log by lazy { icon("log", "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M8 13h8M8 17h5") }
    val Plus by lazy { icon("plus", "M12 5v14M5 12h14") }
    val Unlock by lazy {
        icon(
            "unlock",
            "M6 11h12a2 2 0 0 1 2 2v6a2 2 0 0 1 -2 2h-12a2 2 0 0 1 -2 -2v-6a2 2 0 0 1 2 -2z M8 11V7a4 4 0 0 1 " +
                "7.8-1.2",
        )
    }
    val Gear by lazy {
        icon(
            "gear",
            "M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0z M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 " +
                "1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 " +
                "0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 " +
                "0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 " +
                "1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 " +
                "1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 " +
                "1z",
        )
    }
    val Lock by lazy {
        icon(
            "lock",
            "M6 11h12a2 2 0 0 1 2 2v6a2 2 0 0 1 -2 2h-12a2 2 0 0 1 -2 -2v-6a2 2 0 0 1 2 -2z M8 11V7a4 4 0 0 1 8 " +
                "0v4",
        )
    }
    val Moon by lazy { icon("moon", "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z") }
    val Monoff by lazy {
        icon(
            "monoff",
            "M5 4h14a2 2 0 0 1 2 2v8a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-8a2 2 0 0 1 2 -2z M8 20h8M12 16v4M3 3l18 " +
                "18",
        )
    }
    val Bell by lazy { icon("bell", "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9 M10.3 21a1.9 1.9 0 0 0 3.4 0") }
    val Pad by lazy { icon("pad", "M6 5h12a3 3 0 0 1 3 3v8a3 3 0 0 1 -3 3h-12a3 3 0 0 1 -3 -3v-8a3 3 0 0 1 3 -3z M3 15h18M12 15v4") }
    val Pres by lazy { icon("pres", "M3 4h18M5 4v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V4M12 16v4M9 20h6 M9 10l2-2 2 2 2-2") }
    val Clip by lazy {
        icon(
            "clip",
            "M9 2h6a1 1 0 0 1 1 1v2a1 1 0 0 1 -1 1h-6a1 1 0 0 1 -1 -1v-2a1 1 0 0 1 1 -1z M16 4h2a2 2 0 0 1 2 " +
                "2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2",
        )
    }
    val Dnd by lazy { icon("dnd", "M3 12a9 9 0 1 0 18 0a9 9 0 1 0 -18 0z M8 12h8") }
    val Vol by lazy { icon("vol", "M11 5 6 9H3v6h3l5 4z M15.5 8.5a5 5 0 0 1 0 7M19 5a10 10 0 0 1 0 14") }
    val VolOff by lazy { icon("vol-off", "M11 5 6 9H3v6h3l5 4z M16 9l6 6M22 9l-6 6") }
    val Sun by lazy {
        icon(
            "sun",
            "M8 12a4 4 0 1 0 8 0a4 4 0 1 0 -8 0z M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 " +
                "12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4",
        )
    }
    val Prev by lazy { icon("prev", "M19 20 9 12l10-8zM5 19V5") }
    val Next by lazy { icon("next", "m5 4 10 8-10 8zM19 5v14") }
    val Pause by lazy {
        icon(
            "pause",
            "M7 4h2a1 1 0 0 1 1 1v14a1 1 0 0 1 -1 1h-2a1 1 0 0 1 -1 -1v-14a1 1 0 0 1 1 -1z M15 4h2a1 1 0 0 1 1 " +
                "1v14a1 1 0 0 1 -1 1h-2a1 1 0 0 1 -1 -1v-14a1 1 0 0 1 1 -1z",
        )
    }
    val Photo by lazy {
        icon(
            "photo",
            "M6 3h12a3 3 0 0 1 3 3v12a3 3 0 0 1 -3 3h-12a3 3 0 0 1 -3 -3v-12a3 3 0 0 1 3 -3z M7 9a2 2 0 1 0 4 0a2 " +
                "2 0 1 0 -4 0z m21 15-5-5L5 21",
        )
    }
    val Video by lazy { icon("video", "M4 6h10a2 2 0 0 1 2 2v8a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2v-8a2 2 0 0 1 2 -2z m22 8-6 4 6 4z") }
    val File by lazy { icon("file", "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6") }
    val Down by lazy { icon("down", "M12 4v14M6 12l6 6 6-6") }
    val Up by lazy { icon("up", "M12 20V6M6 12l6-6 6 6") }
    val Home by lazy { icon("home", "M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z") }
    val Swap by lazy { icon("swap", "M7 4v16M3 8l4-4 4 4M17 20V4M13 16l4 4 4-4") }
    val Chev by lazy { icon("chev", "m9 6 6 6-6 6") }
    val Bolt by lazy { icon("bolt", "M13 2 4 14h7l-1 8 9-12h-7z") }
    val Phone by lazy { icon("phone", "M9 2h6a3 3 0 0 1 3 3v14a3 3 0 0 1 -3 3h-6a3 3 0 0 1 -3 -3v-14a3 3 0 0 1 3 -3z M11 18h2") }
    val Laptop by lazy { icon("laptop", "M6 4h12a2 2 0 0 1 2 2v7a2 2 0 0 1 -2 2h-12a2 2 0 0 1 -2 -2v-7a2 2 0 0 1 2 -2z M2 19h20") }
    val Ring by lazy { icon("ring", "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9 M10.3 21a1.9 1.9 0 0 0 3.4 0") }
    val Send by lazy { icon("send", "m22 2-7 20-4-9-9-4z M22 2 11 13") }
    val Search by lazy { icon("search", "M4 11a7 7 0 1 0 14 0a7 7 0 1 0 -14 0z m21 21-4.3-4.3") }
    val X by lazy { icon("x", "M18 6 6 18M6 6l12 12") }
    val Check by lazy { icon("check", "M20 6 9 17l-5-5") }
    val Wifi by lazy { icon("wifi", "M5 12.6a10 10 0 0 1 14 0M8.5 16.1a5 5 0 0 1 7 0M2 8.8a15 15 0 0 1 20 0 M12 20h.01") }
    val Bt by lazy { icon("bt", "m7 7 10 10-5 5V2l5 5L7 17") }
    val Missed by lazy {
        icon(
            "missed",
            "M22 16.9v3a2 2 0 0 1-2.2 2 19.8 19.8 0 0 1-8.6-3.1 19.5 19.5 0 0 1-6-6A19.8 19.8 0 0 1 2.1 4.2 2 2 0 " +
                "0 1 4.1 2h3a2 2 0 0 1 2 1.7c.1.9.4 1.8.7 2.7a2 2 0 0 1-.5 2.1L8 9.8a16 16 0 0 0 6 6l1.3-1.3a2 2 0 0 " +
                "1 2.1-.4c.9.3 1.8.6 2.7.7a2 2 0 0 1 1.7 2z m16 2 6 6M22 2l-6 6",
        )
    }
    val More by lazy {
        icon(
            "more",
            "M4 12a1 1 0 1 0 2 0a1 1 0 1 0 -2 0z M11 12a1 1 0 1 0 2 0a1 1 0 1 0 -2 0z M18 12a1 1 0 1 0 2 0a1 1 0 " +
                "1 0 -2 0z",
        )
    }
    val Minus by lazy { icon("minus", "M5 12h14") }
    val Maxi by lazy { icon("maxi", "M7 7h10v10H7z") }
    val Clock by lazy { icon("clock", "M3 12a9 9 0 1 0 18 0a9 9 0 1 0 -18 0z M12 7v5l3 2") }
    val Back by lazy { icon("back", "m15 18-6-6 6-6") }
    val Music by lazy { icon("music", "M9 18V5l12-2v13 M3 18a3 3 0 1 0 6 0a3 3 0 1 0 -6 0z M15 16a3 3 0 1 0 6 0a3 3 0 1 0 -6 0z") }
    val Refresh by lazy { icon("refresh", "M21 12a9 9 0 1 1-2.6-6.4L21 8 M21 3v5h-5") }
    val Alert by lazy { icon("alert", "M3 12a9 9 0 1 0 18 0a9 9 0 1 0 -18 0z M12 8v4M12 16h.01") }
    val Monitor by lazy { icon("monitor", "M5 4h14a2 2 0 0 1 2 2v8a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-8a2 2 0 0 1 2 -2z M8 20h8M12 16v4") }
    val Kbd by lazy {
        icon(
            "kbd",
            "M4 5h16a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-16a2 2 0 0 1 -2 -2v-10a2 2 0 0 1 2 -2z M6 9h.01M10 9h.01M14 9h.01M18 9h.01M7 15h10",
        )
    }
    val Qr by lazy { icon("qr", "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3z M14 14h3v3h-3zM20 14v.01M14 20h.01M17 17h4v4h-4") }
}

private fun icon(
    name: String,
    path: String,
) = ImageVector
    .Builder(name, 24.dp, 24.dp, 24f, 24f)
    .addPath(
        PathParser().parsePathString(path).toNodes(),
        stroke = SolidColor(Color.Black),
        strokeLineWidth = 2f,
        strokeLineCap = StrokeCap.Round,
        strokeLineJoin = StrokeJoin.Round,
    ).build()
