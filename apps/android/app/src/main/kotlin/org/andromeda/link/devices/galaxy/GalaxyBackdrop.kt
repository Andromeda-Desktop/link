package org.andromeda.link.devices.galaxy

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.andromeda.link.ui.theme.LinkTheme

/** Joins the backdrop's view to the gesture area over it, which sits in the scrolling list. */
class GalaxyControl {
    internal var view: GalaxyView? = null
    internal var origin = Offset.Zero

    /** [p] is in the gesture area's own coordinates. */
    internal fun toView(p: Offset): Offset {
        val v = view ?: return p
        val at = IntArray(2).also { v.getLocationInWindow(it) }
        return p + origin - Offset(at[0].toFloat(), at[1].toFloat())
    }
}

@Composable
fun rememberGalaxy() = remember { GalaxyControl() }

/**
 * The galaxy behind Home. [scroll] is the sheet's scroll in px; the scene moves [share] of it, as a layer move with no
 * GL frame.
 */
@Composable
fun GalaxyBackdrop(
    control: GalaxyControl,
    scroll: () -> Float,
    share: Float,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val tier = remember { galaxyTier(context) }
    val light = !LinkTheme.dark
    AndroidView(
        factory = { GalaxyView(it).also { v -> control.view = v } },
        modifier = modifier.graphicsLayer { translationY = -scroll() * share },
        update = { it.setLight(light) },
        onRelease = { if (control.view === it) control.view = null },
    )
    LaunchedEffect(tier) {
        val points = withContext(Dispatchers.Default) { GalaxyCache.get(tier) }
        control.view?.setPoints(points)
    }
}

/** One finger turns the galaxy, two fingers zoom about their centre, a double tap flies home. */
fun Modifier.galaxyGestures(control: GalaxyControl): Modifier =
    onGloballyPositioned { control.origin = it.positionInWindow() }
        .pointerInput(control) { detectTapGestures(onDoubleTap = { control.view?.home() }) }
        .pointerInput(control) {
            awaitEachGesture {
                awaitFirstDown(requireUnconsumed = false)
                var moved = 0f
                var active = false
                do {
                    val event = awaitPointerEvent()
                    val down = event.changes.count { it.pressed }
                    val view = control.view
                    if (down >= 2) {
                        val zoom = event.calculateZoom()
                        if (zoom != 1f) {
                            val c = control.toView(event.calculateCentroid())
                            view?.zoom(c.x, c.y, zoom)
                        }
                        active = true
                        event.changes.forEach { it.consume() }
                    } else if (down == 1) {
                        val pan = event.calculatePan()
                        moved += pan.getDistance()
                        if (!active && moved > viewConfiguration.touchSlop) active = true
                        if (active) {
                            view?.orbit(pan.x, pan.y)
                            event.changes.forEach { it.consume() }
                        }
                    }
                } while (event.changes.any { it.pressed })
                if (active) control.view?.release()
            }
        }
