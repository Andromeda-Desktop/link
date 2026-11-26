package org.andromeda.link.ui.kit

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
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
    /** A tap on the symbol, which then does not set the level; null leaves the symbol part of the track. */
    onIcon: Pair<String, () -> Unit>? = null,
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
        val start = if (large) Tok.Space.p16.dp else Tok.Space.p12.dp
        val tap =
            onIcon?.let { (label, action) ->
                Modifier
                    .clip(CircleShape)
                    .clickable(enabled = enabled, role = Role.Button, onClickLabel = label, onClick = action)
            } ?: Modifier
        Box(tap.fillMaxHeight().padding(horizontal = start), contentAlignment = Alignment.Center) {
            Symbol(icon, s.onAccent, size = if (large) Tok.Phone.symbol.dp else Tok.Size.symbolL.dp - Tok.Space.p2.dp)
        }
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

/**
 * A level as an upright pill: the fill rises from the bottom, the value at the top and the symbol at the foot. Dark: a
 * light fill on slider_track. Light: a plain white pill with a grey fill and one card shadow.
 */
@Composable
fun PillSlider(
    percent: Int?,
    icon: ImageVector,
    label: String,
    enabled: Boolean,
    onChange: (Int) -> Unit,
    modifier: Modifier = Modifier,
    onIcon: Pair<String, () -> Unit>? = null,
) {
    val s = LinkTheme.scheme
    val dark = LinkTheme.dark
    val fill = if (dark) s.accent else s.fill2
    val ink = if (dark) s.onAccent else s.label1
    val shadow = if (dark) Modifier else Modifier.elevation(Tok.Elevation.e2, Tok.Radius.pill.dp)
    DragTrack(
        (percent ?: 0) / 100f,
        label,
        enabled && percent != null,
        modifier
            .then(shadow)
            .clip(CircleShape)
            .background(if (dark) s.sliderTrack else s.view),
        onDone = { onChange((it * 100).toInt()) },
        onChange = { onChange((it * 100).toInt()) },
        vertical = true,
    ) { value, _ ->
        BoxWithConstraints(Modifier.fillMaxSize().alpha(if (enabled) 1f else Tok.Phone.offAlpha)) {
            val top = LinkTheme.phoneType.headline
            val density = LocalDensity.current
            // The ink flips where the fill passes the middle of the value text, and of the symbol.
            val filled = maxHeight * if (percent != null) value else 0f
            val textMiddle = maxHeight - Tok.Space.p20.dp - with(density) { top.lineHeight.toDp() } / 2
            val iconMiddle = Tok.Space.p12.dp + Tok.Phone.hit.dp / 2
            if (percent != null) {
                Box(
                    Modifier
                        .align(Alignment.BottomCenter)
                        .fillMaxWidth()
                        .fillMaxHeight(value)
                        .background(fill),
                )
            }
            Text(
                percent?.let { "${(value * 100).toInt()}" } ?: "–",
                top,
                color = if (filled > textMiddle) ink else s.label1,
                modifier = Modifier.align(Alignment.TopCenter).padding(top = Tok.Space.p20.dp),
                maxLines = 1,
            )
            val tap =
                onIcon?.let { (text, action) ->
                    Modifier
                        .clip(CircleShape)
                        .clickable(enabled = enabled, role = Role.Button, onClickLabel = text, onClick = action)
                } ?: Modifier
            Box(
                tap.align(Alignment.BottomCenter).padding(bottom = Tok.Space.p12.dp).size(Tok.Phone.hit.dp),
                contentAlignment = Alignment.Center,
            ) {
                Symbol(icon, if (filled > iconMiddle) ink else s.label1, size = Tok.Phone.symbol.dp)
            }
        }
    }
}

/** The fill passes under the value text above this level, so the text turns on_accent. */
private const val HIGH = 0.85f

/**
 * A track a finger taps or drags to a value from 0 to 1 (left to right, or bottom to top when [vertical]), reported
 * once when the finger lifts; [draw] shows the moving value. Accessibility services set it through its progress range.
 */
@Composable
fun DragTrack(
    value: Float,
    label: String,
    enabled: Boolean,
    modifier: Modifier = Modifier,
    onDone: (Float) -> Unit = {},
    onChange: ((Float) -> Unit)? = null,
    vertical: Boolean = false,
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
    var length by remember { mutableIntStateOf(1) }
    // The set value stays shown until a report comes close to it, so an older report does not pull the track back.
    LaunchedEffect(pending, value) {
        val target = pending ?: return@LaunchedEffect
        if (abs(value - target) > SETTLED) delay(SETTLE_MS)
        pending = null
    }
    val shown = (dragging ?: pending ?: value).coerceIn(0f, 1f)
    val at = { p: Offset -> (if (vertical) 1 - p.y / length else p.x / length).coerceIn(0f, 1f) }
    val input =
        if (enabled) {
            Modifier
                .pointerInput(Unit) { detectTapGestures { done(at(it)) } }
                .pointerInput(vertical) {
                    val start = { p: Offset -> dragging = at(p) }
                    val end = {
                        dragging?.let(done)
                        dragging = null
                    }
                    val cancel = { dragging = null }
                    val move = { event: PointerInputChange ->
                        val now = at(event.position)
                        dragging = now
                        val send = change
                        if (send != null && event.uptimeMillis - sentAt >= LIVE_MS) {
                            sentAt = event.uptimeMillis
                            pending = now
                            send(now)
                        }
                    }
                    if (vertical) {
                        detectVerticalDragGestures(start, end, cancel) { event, _ -> move(event) }
                    } else {
                        detectHorizontalDragGestures(start, end, cancel) { event, _ -> move(event) }
                    }
                }
        } else {
            Modifier
        }
    Box(
        modifier
            .onSizeChanged { length = (if (vertical) it.height else it.width).coerceAtLeast(1) }
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
