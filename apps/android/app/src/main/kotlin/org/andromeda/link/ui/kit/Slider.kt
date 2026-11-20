package org.andromeda.link.ui.kit

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import kotlin.math.abs

/**
 * A level as a capsule: slider_track, the accent fill grows from the left (never narrower than the height), the
 * symbol on the fill and the value at the right end. Drag follows the finger 1:1, with no spring on release.
 */
@Composable
fun CapsuleSlider(
    percent: Int,
    icon: ImageVector,
    label: String,
    enabled: Boolean,
    onChange: (Int) -> Unit,
    modifier: Modifier = Modifier,
    large: Boolean = true,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val h = if (large) Tok.Phone.slider.dp else Tok.Size.controlL.dp
    DragTrack(
        percent / 100f,
        label,
        enabled,
        modifier
            .fillMaxWidth()
            .height(h)
            .clip(CircleShape)
            .background(s.sliderTrack),
        onDone = { onChange((it * 100).toInt()) },
        onChange = { onChange((it * 100).toInt()) },
    ) { value, _ ->
        Box(
            Modifier
                .fillMaxHeight()
                .fillMaxWidth(value)
                .widthIn(min = h)
                .clip(CircleShape)
                .background(s.accent),
        )
        Symbol(
            icon,
            s.onAccent,
            size = if (large) Tok.Phone.symbol.dp else Tok.Size.symbolL.dp - Tok.Space.p2.dp,
            modifier = Modifier.padding(start = if (large) Tok.Space.p16.dp else Tok.Space.p12.dp),
        )
        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
            Text(
                "${(value * 100).toInt()}",
                if (large) t.headline else t.footnote.copy(fontWeight = FontWeight.SemiBold),
                color = if (value > HIGH) s.onAccent else s.label1,
                modifier = Modifier.padding(end = if (large) Tok.Space.p16.dp + Tok.Space.p2.dp else Tok.Space.p12.dp + Tok.Space.p2.dp),
                maxLines = 1,
            )
        }
    }
}

/** The fill passes under the value text above this level, so the text turns on_accent. */
private const val HIGH = 0.85f

/**
 * A horizontal track a finger taps or drags to a value from 0 to 1, reported once when the finger lifts; [draw] shows
 * the moving value. Accessibility services set it through its progress range.
 */
@Composable
fun DragTrack(
    value: Float,
    label: String,
    enabled: Boolean,
    modifier: Modifier = Modifier,
    onDone: (Float) -> Unit = {},
    onChange: ((Float) -> Unit)? = null,
    draw: @Composable (value: Float, dragging: Boolean) -> Unit,
) {
    var pending by remember { mutableStateOf<Float?>(null) }
    val done by rememberUpdatedState<(Float) -> Unit> {
        pending = it
        onDone(it)
    }
    val change by rememberUpdatedState(onChange)
    var dragging by remember { mutableStateOf<Float?>(null) }
    var sentAt by remember { mutableLongStateOf(0L) }
    var width by remember { mutableIntStateOf(1) }
    // The set value stays shown until a report comes close to it, so an older report does not pull the track back.
    LaunchedEffect(pending, value) {
        val target = pending ?: return@LaunchedEffect
        if (abs(value - target) > SETTLED) delay(SETTLE_MS)
        pending = null
    }
    val shown = (dragging ?: pending ?: value).coerceIn(0f, 1f)
    val at = { x: Float -> (x / width).coerceIn(0f, 1f) }
    val input =
        if (enabled) {
            Modifier
                .pointerInput(Unit) { detectTapGestures { done(at(it.x)) } }
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragStart = { dragging = at(it.x) },
                        onDragEnd = {
                            dragging?.let(done)
                            dragging = null
                        },
                        onDragCancel = { dragging = null },
                    ) { event, _ ->
                        val now = at(event.position.x)
                        dragging = now
                        val send = change
                        if (send != null && event.uptimeMillis - sentAt >= LIVE_MS) {
                            sentAt = event.uptimeMillis
                            pending = now
                            send(now)
                        }
                    }
                }
        } else {
            Modifier
        }
    Box(
        modifier
            .onSizeChanged { width = it.width.coerceAtLeast(1) }
            .semantics {
                contentDescription = label
                progressBarRangeInfo = ProgressBarRangeInfo(shown, 0f..1f)
                if (enabled) {
                    setProgress { target ->
                        done(target.coerceIn(0f, 1f))
                        true
                    }
                }
            }.then(input),
        contentAlignment = Alignment.CenterStart,
    ) { draw(shown, dragging != null) }
}

/** A level slider sends at most this often while dragged, ms. */
private const val LIVE_MS = 100L

/** How close a report must come to the set value to count as caught up, as a fraction of the track. */
private const val SETTLED = 0.02f

/** How long the set value is shown when no report comes close to it, ms. */
private const val SETTLE_MS = 1500L
