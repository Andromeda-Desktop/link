package org.andromeda.link.ui.kit

import androidx.compose.foundation.background
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

/**
 * Row [index] of [count] as one slice of a [GroupCard], so a long list stays lazy: the view fill on every slice, the
 * r16 corners and the stroke ring closed only at the ends.
 */
@Composable
fun Modifier.groupSlice(
    index: Int,
    count: Int,
): Modifier {
    val s = LinkTheme.scheme
    val r = Tok.Radius.r16.dp
    val first = index == 0
    val last = index == count - 1
    val top = if (first) r else 0.dp
    val bottom = if (last) r else 0.dp
    return clip(RoundedCornerShape(top, top, bottom, bottom))
        .background(s.view)
        .drawWithContent {
            drawContent()
            // The ring of the whole card, drawn past the slice's open ends so only its sides show there.
            val rp = r.toPx()
            val y0 = if (first) 0f else -rp
            val y1 = if (last) size.height else size.height + rp
            drawRoundRect(s.stroke, Offset(0f, y0), Size(size.width, y1 - y0), CornerRadius(rp), style = Stroke(0f))
        }
}
