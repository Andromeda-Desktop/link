package org.andromeda.link.ui.theme

import androidx.compose.animation.core.SpringSpec
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.animation.core.spring as composeSpring

/** A spring from the tokens: damping is the ratio to critical, mass 1. */
fun <T> spring(t: Tok.SpringToken): SpringSpec<T> = composeSpring(dampingRatio = t.damping, stiffness = t.stiffness)

/** The two-layer token shadow (key + contact) under a rounded rect of [radius]; Modifier.shadow() has one layer. */
@Composable
fun Modifier.elevation(
    e: Tok.ElevationToken,
    radius: Dp,
): Modifier {
    val dark = LinkTheme.dark
    val key = Color.Black.copy(alpha = if (dark) e.keyDark else e.keyLight).toArgb()
    val contact = Color.Black.copy(alpha = if (dark) e.contactDark else e.contactLight).toArgb()
    return drawWithCache {
        val r = radius.toPx()
        val layers =
            listOf(Triple(e.keyY, e.keyBlur, key), Triple(e.contactY, e.contactBlur, contact))
                .filter { it.third ushr 24 != 0 }
                .map { (y, blur, argb) ->
                    android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
                        color = android.graphics.Color.TRANSPARENT
                        setShadowLayer(shadowRadius(blur.dp.toPx()), 0f, y.dp.toPx(), argb)
                    }
                }
        onDrawBehind {
            drawIntoCanvas { canvas -> layers.forEach { canvas.nativeCanvas.drawRoundRect(0f, 0f, size.width, size.height, r, r, it) } }
        }
    }
}

/** CSS blur b is a Gaussian with sigma b/2; Skia maps a shadow radius to sigma = 0.57735 r + 0.5. */
private fun shadowRadius(blurPx: Float) = ((blurPx / 2f - 0.5f) / 0.57735f).coerceAtLeast(0.01f)
