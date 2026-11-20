package org.andromeda.link.ui.kit

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.Velocity
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.spring
import kotlin.math.roundToInt

/**
 * Pull to refresh over scrollable [content]: a pull past the top draws the spinner arc down; a release past the
 * threshold refreshes, and the spinner turns until [refreshing] is false.
 */
@Composable
fun PullRefresh(
    refreshing: Boolean,
    onRefresh: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable BoxScope.() -> Unit,
) {
    val threshold = with(LocalDensity.current) { (Tok.Phone.hit + Tok.Space.p24).dp.toPx() }
    val pull = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val connection =
        remember(threshold) {
            object : NestedScrollConnection {
                override fun onPreScroll(
                    available: Offset,
                    source: NestedScrollSource,
                ): Offset {
                    if (available.y >= 0 || pull.value <= 0f) return Offset.Zero
                    val used = maxOf(available.y, -pull.value)
                    scope.launch { pull.snapTo(pull.value + used) }
                    return Offset(0f, used)
                }

                override fun onPostScroll(
                    consumed: Offset,
                    available: Offset,
                    source: NestedScrollSource,
                ): Offset {
                    if (available.y <= 0 || source != NestedScrollSource.UserInput) return Offset.Zero
                    val next = pull.value + available.y * Tok.Motion.rubberBand
                    scope.launch { pull.snapTo(next.coerceAtMost(threshold * 1.5f)) }
                    return Offset(0f, available.y)
                }

                override suspend fun onPreFling(available: Velocity): Velocity {
                    if (pull.value >= threshold) onRefresh()
                    if (pull.value > 0f) pull.animateTo(0f, spring(Tok.Spring.close))
                    return Velocity.Zero
                }
            }
        }
    LaunchedEffect(refreshing) { if (!refreshing) pull.animateTo(0f, spring(Tok.Spring.close)) }
    Box(modifier.nestedScroll(connection)) {
        content()
        val shown = if (refreshing) threshold else pull.value
        if (shown > 0f) {
            val progress = (shown / threshold).coerceIn(0f, 1f)
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(top = Tok.Space.p8.dp)
                    .offset { IntOffset(0, (shown - threshold).roundToInt()) }
                    .alpha(progress),
                contentAlignment = Alignment.TopCenter,
            ) {
                if (refreshing) Spinner(Tok.Size.controlS.dp) else Arc(Tok.Size.controlS.dp, progress)
            }
        }
    }
}

/** The rulebook spinner: a 270 degree arc in label_2 that turns linearly. Compose it only while work runs. */
@Composable
fun Spinner(
    size: Dp,
    modifier: Modifier = Modifier,
) {
    val turn by rememberInfiniteTransition(label = "spinner").animateFloat(
        0f,
        360f,
        infiniteRepeatable(tween(Tok.Motion.spinnerPeriodMs, easing = LinearEasing)),
        label = "turn",
    )
    Arc(size, 1f, modifier.rotate(turn))
}

/** The spinner ring with [fill] of its arc drawn, still. */
@Composable
private fun Arc(
    size: Dp,
    fill: Float,
    modifier: Modifier = Modifier,
) {
    val color = LinkTheme.scheme.label2
    Canvas(modifier.size(size)) {
        val width = this.size.minDimension / 8f
        drawArc(
            color,
            startAngle = -90f,
            sweepAngle = Tok.Motion.spinnerArc * fill,
            useCenter = false,
            topLeft = Offset(width / 2f, width / 2f),
            size =
                androidx.compose.ui.geometry
                    .Size(this.size.width - width, this.size.height - width),
            style = Stroke(width, cap = StrokeCap.Round),
        )
    }
}
