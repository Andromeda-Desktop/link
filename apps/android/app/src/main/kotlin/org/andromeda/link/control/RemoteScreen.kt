package org.andromeda.link.control

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.nativeKeyCode
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.core.content.edit
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.delay
import org.andromeda.link.R
import org.andromeda.link.core.domain.PointerButton
import org.andromeda.link.core.domain.RemoteInput
import org.andromeda.link.core.domain.RemoteKey
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.RoundKey
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
import android.view.KeyEvent as AndroidKeyEvent

/** What the Remote shows over its pad, one at a time. */
enum class RemoteMode {
    Pad,
    Keys,
    Typing,
    Present,
    ;

    /** [next] when the Remote can go there from here, else this mode: typing opens only from the keys. */
    fun to(next: RemoteMode): RemoteMode = if (next == Typing && this != Keys) this else next
}

/**
 * One desktop's pointer, keys and slides: the whole screen is the touchpad; round keys open the keys sheet, the
 * presenter and the desktop's screen. Without input on the desktop only the pad shows, dimmed.
 */
@Composable
fun RemoteScreen(
    viewModel: RemoteViewModel,
    start: RemoteMode,
    onBack: () -> Unit,
    onScreen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val failure by viewModel.failure.collectAsStateWithLifecycle()
    val desk by viewModel.desk.collectAsStateWithLifecycle()
    val held by viewModel.held.collectAsStateWithLifecycle()
    val shortcuts by viewModel.shortcuts.collectAsStateWithLifecycle()
    val touchpad by viewModel.touchpad.collectAsStateWithLifecycle()
    val actions: RemoteActions = viewModel
    val haptic = LocalHapticFeedback.current
    LaunchedEffect(viewModel) { viewModel.ticks.collect { haptic.performHapticFeedback(HapticFeedbackType.ContextClick) } }
    var mode by rememberSaveable { mutableStateOf(start) }
    val go = { next: RemoteMode -> mode = mode.to(next) }
    LaunchedEffect(desk.input) { if (!desk.input) go(RemoteMode.Pad) }
    BackHandler(mode != RemoteMode.Pad) { go(if (mode == RemoteMode.Typing) RemoteMode.Keys else RemoteMode.Pad) }
    val view = LocalView.current
    DisposableEffect(mode == RemoteMode.Present) {
        val presenting = mode == RemoteMode.Present
        if (presenting) {
            view.keepScreenOn = true
            viewModel.presenting(true)
        }
        onDispose {
            if (presenting) {
                view.keepScreenOn = false
                viewModel.presenting(false)
            }
        }
    }
    val resources = LocalResources.current
    Box(modifier) {
        Screen(toast = failure?.text(resources), onToastEnd = viewModel::failureShown) {
            if (mode == RemoteMode.Present) {
                Presenter(actions, go, onScreen, Modifier.weight(1f).fillMaxWidth())
            } else {
                Column(Modifier.weight(1f).fillMaxWidth().imePadding()) {
                    Pad(actions, desk, touchpad, mode, go, onBack, onScreen, Modifier.weight(1f).fillMaxWidth())
                    when (mode) {
                        RemoteMode.Keys -> KeysSheet(actions, held, shortcuts, desk.name) { go(RemoteMode.Typing) }
                        RemoteMode.Typing -> KeyStrip(actions, held, shortcuts)
                        else -> Unit
                    }
                }
            }
        }
    }
}

@Composable
private fun Pad(
    actions: RemoteActions,
    desk: RemoteDesk,
    touchpad: Boolean,
    mode: RemoteMode,
    go: (RemoteMode) -> Unit,
    onBack: () -> Unit,
    onScreen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val raw = touchpad
    val context = LocalContext.current
    DisposableEffect(raw) {
        val restore = if (raw) keepMultiFingerTouches(context) else null
        onDispose { restore?.invoke() }
    }
    val surface = Modifier.fillMaxSize().dots(s.fill2)
    val touchpad = stringResource(R.string.remote_touchpad)
    val off = Modifier.alpha(if (desk.input) 1f else Tok.Phone.offAlpha)
    Box(modifier) {
        Box(
            when {
                !desk.input -> surface
                raw ->
                    surface.rawTouchpad(
                        LocalResources.current.displayMetrics,
                        onTick = actions::tick,
                    ) { actions.send(RemoteInput.Touch(it)) }
                else -> surface.pointerInput(actions) { interpret(actions, actions::move) }
            }.semantics { contentDescription = touchpad },
        )
        if (mode == RemoteMode.Pad) Tips(off)
        if (mode != RemoteMode.Typing) {
            Rail(
                actions,
                desk.input,
                off
                    .align(Alignment.TopEnd)
                    .padding(
                        top = (Tok.Phone.hit + Tok.Space.p24).dp,
                        bottom =
                            if (mode ==
                                RemoteMode.Pad
                            ) {
                                (Tok.Phone.dockInset + Tok.Phone.roundKeyL + Tok.Space.p32).dp
                            } else {
                                Tok.Space.p24.dp
                            },
                        end = (Tok.Space.p4 + Tok.Space.p2).dp,
                    ),
            )
        }
        if (mode == RemoteMode.Pad) {
            Row(
                off
                    .align(Alignment.BottomCenter)
                    .navigationBarsPadding()
                    .padding(bottom = Tok.Phone.dockInset.dp),
                horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
            ) {
                val d = Tok.Phone.roundKeyL.dp
                RoundKey(Icons.Kbd, stringResource(R.string.remote_keys), { go(RemoteMode.Keys) }, size = d, enabled = desk.input)
                RoundKey(Icons.Pres, stringResource(R.string.presenter_title), { go(RemoteMode.Present) }, size = d, enabled = desk.input)
                RoundKey(Icons.Screen, stringResource(R.string.remote_screen), onScreen, size = d)
            }
        }
        if (mode == RemoteMode.Typing) {
            Composer(
                actions,
                desk.name,
                { go(RemoteMode.Keys) },
                Modifier
                    .align(Alignment.BottomCenter)
                    .padding(horizontal = Tok.Space.p12.dp, vertical = Tok.Space.p8.dp),
            )
        }
        TopBar(desk, onBack, Modifier.align(Alignment.TopStart))
        if (raw && needsMultiFingerGrant(context)) {
            Text(
                stringResource(R.string.remote_multi_finger_grant, context.packageName),
                LinkTheme.phoneType.footnote,
                color = s.label2,
                modifier =
                    Modifier
                        .align(Alignment.BottomCenter)
                        .padding(
                            start = Tok.Space.p24.dp,
                            end = Tok.Space.p24.dp,
                            bottom = (Tok.Phone.dockInset + Tok.Phone.roundKeyL + Tok.Space.p16).dp,
                        ),
            )
        }
    }
}

/** The pad's dot grid, drawn once per size change. */
internal fun Modifier.dots(dot: Color): Modifier =
    drawBehind {
        val step =
            Tok.Space.p20.dp
                .toPx()
        val r =
            Tok.Phone.padDot.dp
                .toPx()
        var y = step / 2
        while (y < size.height) {
            var x = step / 2
            while (x < size.width) {
                drawCircle(dot, r, Offset(x, y))
                x += step
            }
            y += step
        }
    }

/** Back and the desktop chip: the dot and the name, or why input is off. */
@Composable
private fun TopBar(
    desk: RemoteDesk,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier.fillMaxWidth().padding(start = Tok.Space.p8.dp, end = Tok.Space.p8.dp, top = Tok.Space.p4.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BackKey(LinkTheme.scheme.label1, onBack)
        DeskChip(desk, if (desk.input) null else stringResource(R.string.remote_input_off), Modifier.weight(1f, fill = false))
    }
}

/** The desktop's capsule: a presence dot and the name, or a shield and [warning] in the warning tint. */
@Composable
internal fun DeskChip(
    desk: RemoteDesk,
    warning: String?,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val h = Tok.Size.controlL.dp
    Row(
        modifier
            .height(h)
            .elevation(Tok.Elevation.e2, h / 2)
            .clip(CircleShape)
            .background(if (warning == null) s.view else lerp(s.view, s.warning, Tok.Phone.glyphTint))
            .border(Dp.Hairline, s.stroke, CircleShape)
            .padding(start = Tok.Space.p12.dp, end = Tok.Space.p16.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (warning == null) {
            Box(Modifier.size(Tok.Space.p8.dp).clip(CircleShape).background(if (desk.connected) s.success else s.label3))
        } else {
            Symbol(Icons.Shield, s.warning, size = Tok.Size.symbol.dp)
        }
        Text(warning ?: desk.name, LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold), maxLines = 1)
    }
}

/** How the pad works, on the pad itself; the touches go through to it. */
@Composable
private fun Tips(modifier: Modifier = Modifier) {
    Column(modifier.fillMaxSize().padding(start = Tok.Space.p24.dp, end = Tok.Space.p48.dp)) {
        Spacer(Modifier.fillMaxHeight(TIPS_TOP))
        Column(verticalArrangement = Arrangement.spacedBy((Tok.Space.p8 + Tok.Space.p2).dp)) {
            Tip("1", stringResource(R.string.remote_tip_one))
            Tip("2", stringResource(R.string.remote_tip_two))
            Tip(stringResource(R.string.remote_tip_hold_key), stringResource(R.string.remote_tip_hold))
        }
    }
}

@Composable
internal fun Tip(
    key: String,
    text: String,
    icon: ImageVector? = null,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Row(horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier
                .size(Tok.Phone.hit.dp, Tok.Size.controlM.dp)
                .clip(CircleShape)
                .background(s.view)
                .border(Dp.Hairline, s.stroke, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            if (icon != null) {
                Symbol(icon, s.label1, size = Tok.Size.symbol.dp)
            } else {
                Text(key, t.caption.copy(fontWeight = FontWeight.Bold), maxLines = 1)
            }
        }
        Text(text, t.body, color = s.label2)
    }
}

/** A drag along the rail scrolls; the pad under it never sees the touch. */
@Composable
private fun Rail(
    actions: RemoteActions,
    enabled: Boolean,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val dash = s.label3
    val rail = stringResource(R.string.remote_rail)
    Column(
        modifier
            .width(Tok.Phone.rail.dp)
            .fillMaxHeight()
            .clip(CircleShape)
            .background(s.fill1)
            .then(
                if (enabled) {
                    Modifier.pointerInput(actions) {
                        detectVerticalDragGestures { change, dy ->
                            change.consume()
                            actions.scroll(0f, dy)
                        }
                    }
                } else {
                    Modifier
                },
            ).semantics { contentDescription = rail }
            .padding(vertical = (Tok.Space.p8 + Tok.Space.p2).dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Symbol(Icons.Chev, s.label2, size = Tok.Size.symbol.dp, modifier = Modifier.rotate(-90f))
        Box(
            Modifier
                .weight(1f)
                .width(Tok.Space.p4.dp)
                .padding(vertical = Tok.Space.p8.dp)
                .drawBehind {
                    val w = size.width
                    drawLine(
                        dash,
                        Offset(w / 2, 0f),
                        Offset(w / 2, size.height),
                        strokeWidth = w,
                        cap = StrokeCap.Round,
                        pathEffect =
                            PathEffect.dashPathEffect(
                                floatArrayOf(
                                    Tok.Space.p2.dp
                                        .toPx(),
                                    Tok.Space.p8.dp
                                        .toPx(),
                                ),
                            ),
                    )
                },
        )
        Symbol(Icons.Chev, s.label2, size = Tok.Size.symbol.dp, modifier = Modifier.rotate(90f))
    }
}

/**
 * For a desktop without a real touchpad: one finger moves by [move] and taps, two scroll and tap to right-click, a hold
 * drags.
 */
internal suspend fun PointerInputScope.interpret(
    actions: RemoteActions,
    move: (Float, Float) -> Unit,
) {
    awaitEachGesture {
        val down = awaitFirstDown()
        var fingers = 1
        var travelled = 0f
        var lifted = down.uptimeMillis
        var dragging = false
        val step = {
            val event = currentEvent
            val pressed = event.changes.filter { it.pressed }
            fingers = maxOf(fingers, pressed.size)
            if (pressed.isNotEmpty()) {
                val delta = pressed.fold(Offset.Zero) { sum, change -> sum + change.positionChange() } / pressed.size.toFloat()
                travelled += delta.getDistance()
                if (pressed.size >= 2) {
                    actions.scroll(delta.x, delta.y)
                } else if (fingers == 1) {
                    move(delta.x, delta.y)
                }
            }
            event.changes.forEach {
                lifted = it.uptimeMillis
                it.consume()
            }
            pressed.isNotEmpty()
        }
        // Down and still for HOLD_MS: the left button holds until the finger lifts, so a move drags.
        val settled =
            withTimeoutOrNull(HOLD_MS) {
                var up: Boolean
                do {
                    awaitPointerEvent()
                    up = !step()
                } while (!up && fingers == 1 && travelled < viewConfiguration.touchSlop)
                up
            }
        if (settled == null) {
            dragging = true
            actions.send(RemoteInput.Press(PointerButton.Left))
        }
        if (settled != true) {
            do {
                awaitPointerEvent()
            } while (step())
        }
        when {
            dragging -> actions.send(RemoteInput.Release(PointerButton.Left))
            lifted - down.uptimeMillis < TAP_MS && travelled < viewConfiguration.touchSlop ->
                actions.send(RemoteInput.Click(if (fingers >= 2) PointerButton.Right else PointerButton.Left))
        }
    }
}

/** The keys a phone keyboard lacks, the modifiers and the user's shortcuts; the composer opens typing. */
@Composable
private fun KeysSheet(
    actions: RemoteActions,
    held: Set<String>,
    shortcuts: Boolean,
    desktopName: String,
    onType: () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val r = Tok.Radius.r28.dp
    val side = Modifier.padding(horizontal = Tok.Space.p12.dp)
    val gap = Tok.Space.p4.dp
    Column(
        Modifier
            .fillMaxWidth()
            .elevation(Tok.Elevation.e4, r)
            .clip(RoundedCornerShape(topStart = r, topEnd = r))
            .background(s.view)
            .navigationBarsPadding()
            .padding(top = Tok.Space.p8.dp, bottom = Tok.Phone.dockInset.dp),
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
    ) {
        Box(
            Modifier
                .align(Alignment.CenterHorizontally)
                .padding(bottom = Tok.Space.p4.dp)
                .size(Tok.Phone.grab.dp, Tok.Space.p4.dp)
                .clip(CircleShape)
                .background(s.fill3),
        )
        Row(
            side
                .fillMaxWidth()
                .height(Tok.Phone.composer.dp)
                .clip(CircleShape)
                .background(s.fill1)
                .clickable(role = Role.Button, onClick = onType)
                .padding(start = (Tok.Space.p16 + Tok.Space.p2).dp, end = Tok.Space.p16.dp),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                stringResource(R.string.remote_type_on, desktopName),
                t.body,
                color = s.label2,
                modifier = Modifier.weight(1f),
                maxLines = 1,
            )
            Symbol(Icons.Kbd, s.label2)
        }
        Row(side, horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
            Column(Modifier.weight(CROSS_WEIGHT), verticalArrangement = Arrangement.spacedBy(gap)) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(gap)) {
                    Spacer(Modifier.weight(1f))
                    Key("↑", "Up") { actions.press(RemoteKey.Up) }
                    Spacer(Modifier.weight(1f))
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(gap)) {
                    Key("←", "Left") { actions.press(RemoteKey.Left) }
                    Key("↓", "Down") { actions.press(RemoteKey.Down) }
                    Key("→", "Right") { actions.press(RemoteKey.Right) }
                }
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(gap)) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(gap)) {
                    Key("Esc", bold = true) { actions.press(RemoteKey.Escape) }
                    Key("Tab", bold = true) { actions.press(RemoteKey.Tab) }
                }
                Row(
                    Modifier.fillMaxWidth(),
                ) { Key(stringResource(R.string.remote_enter), bold = true) { actions.press(RemoteKey.Enter) } }
            }
        }
        Modifiers(actions, held, shortcuts, side)
        if (shortcuts) {
            Shortcuts(actions)
        } else {
            Text(stringResource(R.string.remote_no_modifiers), t.footnote, color = s.label2, modifier = side)
        }
    }
}

@Composable
private fun RowScope.Key(
    text: String,
    label: String = text,
    bold: Boolean = false,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    val style = if (bold) LinkTheme.phoneType.body.copy(fontWeight = FontWeight.SemiBold) else LinkTheme.phoneType.headline
    Box(
        Modifier
            .weight(1f)
            .height(Tok.Size.rowTwo.dp)
            .clip(RoundedCornerShape(Tok.Radius.r16.dp))
            .background(s.fill1)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Text(text, style, maxLines = 1) }
}

/** Ctrl, Alt, Super and Shift stay held, in accent, until the next key. */
@Composable
private fun Modifiers(
    actions: RemoteActions,
    held: Set<String>,
    enabled: Boolean,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    Row(
        modifier.alpha(if (enabled) 1f else Tok.Phone.offAlpha),
        horizontalArrangement = Arrangement.spacedBy((Tok.Space.p4 + Tok.Space.p2).dp),
    ) {
        for (name in MODIFIERS) {
            val on = name in held
            Box(
                Modifier
                    .weight(1f)
                    .height(Tok.Phone.keyS.dp)
                    .clip(CircleShape)
                    .background(if (on) s.accent else s.fill1)
                    .clickable(enabled = enabled, role = Role.Button) { actions.hold(name) },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    label(name),
                    LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold),
                    color = if (on) s.onAccent else s.label1,
                    maxLines = 1,
                )
            }
        }
    }
}

/** The user's shortcuts as named tiles; a long press removes one, Add takes a new one as `ctrl+w`. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Shortcuts(actions: RemoteActions) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val context = LocalContext.current
    var saved by remember { mutableStateOf(savedShortcuts(context)) }
    var adding by rememberSaveable { mutableStateOf(false) }
    val save = { list: List<String> ->
        saved = list
        prefs(context).edit { putString(SHORTCUTS_KEY, list.joinToString("\n")) }
    }
    val r = RoundedCornerShape(Tok.Radius.r16.dp)
    val tile =
        Modifier
            .heightIn(min = Tok.Phone.composer.dp)
            .clip(r)
    val pad = Modifier.padding(horizontal = Tok.Phone.pillPad.dp, vertical = (Tok.Space.p4 + Tok.Space.p2).dp)
    Row(
        Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = Tok.Space.p12.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
    ) {
        for (shortcut in saved) {
            val name = shortcutName(shortcut)
            Column(
                tile
                    .background(s.view)
                    .border(Dp.Hairline, s.stroke, r)
                    .combinedClickable(role = Role.Button, onLongClick = { save(saved - shortcut) }) { actions.shortcut(shortcut) }
                    .then(pad),
                verticalArrangement = Arrangement.Center,
            ) {
                val keys = label(shortcut).replace('+', ' ')
                Text(name ?: keys, t.headline, maxLines = 1)
                if (name != null) Text(keys, t.caption, color = s.label2, maxLines = 1)
            }
        }
        if (adding) {
            var text by rememberSaveable { mutableStateOf("") }
            val focus = remember { FocusRequester() }
            LaunchedEffect(Unit) { focus.requestFocus() }
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                modifier =
                    tile
                        .widthIn(min = Tok.Phone.valueMax.dp)
                        .background(s.fill1)
                        .focusRequester(focus)
                        .then(pad),
                textStyle = t.headline.copy(color = s.label1),
                cursorBrush = SolidColor(s.accent),
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions =
                    KeyboardActions(onDone = {
                        adding = false
                        val shortcut = text.trim().lowercase().replace(" ", "")
                        if (shortcut.isNotEmpty() && shortcut !in saved) save(saved + shortcut)
                    }),
                decorationBox = { field ->
                    Box(Modifier.heightIn(min = Tok.Size.controlL.dp), contentAlignment = Alignment.CenterStart) {
                        if (text.isEmpty()) Text(stringResource(R.string.shortcut_hint), t.headline, color = s.label3, maxLines = 1)
                        field()
                    }
                },
            )
        } else {
            Row(
                tile
                    .background(s.fill1)
                    .clickable(role = Role.Button) { adding = true }
                    .then(pad)
                    .heightIn(min = Tok.Size.controlL.dp),
                horizontalArrangement = Arrangement.spacedBy((Tok.Space.p4 + Tok.Space.p2).dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Symbol(Icons.Plus, s.accentText, size = Tok.Size.symbolL.dp)
                Text(
                    stringResource(R.string.shortcut_add),
                    t.body.copy(fontWeight = FontWeight.SemiBold),
                    color = s.accentText,
                    maxLines = 1,
                )
            }
        }
    }
}

@Composable
private fun shortcutName(shortcut: String): String? =
    when (shortcut) {
        "ctrl+c" -> stringResource(R.string.shortcut_copy)
        "ctrl+v" -> stringResource(R.string.shortcut_paste)
        "alt+tab" -> stringResource(R.string.shortcut_switch)
        "super+t" -> stringResource(R.string.shortcut_terminal)
        "ctrl+w" -> stringResource(R.string.shortcut_close)
        else -> null
    }

/** The typing field on the pad, above the key strip and the system keyboard; closing the keyboard ends typing. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun Composer(
    actions: RemoteActions,
    desktopName: String,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    var text by rememberSaveable { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val ime = WindowInsets.isImeVisible
    var shown by remember { mutableStateOf(false) }
    val close by rememberUpdatedState(onClose)
    LaunchedEffect(ime) {
        if (ime) {
            shown = true
        } else if (shown) {
            close()
        }
    }
    val enter = {
        actions.press(RemoteKey.Enter)
        text = ""
    }
    Row(
        modifier
            .fillMaxWidth()
            .height(Tok.Phone.composer.dp)
            .clip(CircleShape)
            .background(s.view)
            .border(Tok.Space.p2.dp, s.accent, CircleShape)
            .padding(start = (Tok.Space.p16 + Tok.Space.p2).dp, end = (Tok.Space.p4 + Tok.Space.p2).dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BasicTextField(
            value = text,
            onValueChange = {
                actions.typed(text, it)
                text = it
            },
            // An empty field has nothing to diff: Backspace there deletes on the desktop (pasted or sent text).
            modifier =
                Modifier.weight(1f).focusRequester(focus).onPreviewKeyEvent {
                    val delete = text.isEmpty() && it.key == Key.Backspace
                    if (delete && it.type == KeyEventType.KeyDown) actions.press(RemoteKey.Backspace)
                    delete
                },
            textStyle = t.body.copy(color = s.label1),
            cursorBrush = SolidColor(s.accent),
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
            keyboardActions = KeyboardActions(onSend = { enter() }),
            decorationBox = { field ->
                Box {
                    if (text.isEmpty()) Text(stringResource(R.string.remote_type_on, desktopName), t.body, color = s.label3, maxLines = 1)
                    field()
                }
            },
        )
        Box(
            Modifier
                .height(Tok.Size.controlL.dp)
                .clip(CircleShape)
                .background(s.accent)
                .clickable(role = Role.Button, onClick = enter)
                .padding(horizontal = Tok.Space.p16.dp),
            contentAlignment = Alignment.Center,
        ) { Text(stringResource(R.string.remote_enter), t.body.copy(fontWeight = FontWeight.SemiBold), color = s.onAccent, maxLines = 1) }
    }
}

/** While typing: the modifiers and the keys a phone keyboard lacks, in one row on the system keyboard. */
@Composable
internal fun KeyStrip(
    actions: RemoteActions,
    held: Set<String>,
    shortcuts: Boolean,
) {
    val s = LinkTheme.scheme
    Row(
        Modifier
            .fillMaxWidth()
            .background(s.fill1)
            .horizontalScroll(rememberScrollState())
            .padding(start = Tok.Space.p8.dp, end = Tok.Space.p8.dp, top = Tok.Space.p8.dp),
        horizontalArrangement = Arrangement.spacedBy((Tok.Space.p4 + Tok.Space.p2).dp),
    ) {
        if (shortcuts) {
            for (name in MODIFIERS) StripKey(label(name), name in held) { actions.hold(name) }
        }
        StripKey("Esc") { actions.press(RemoteKey.Escape) }
        StripKey("Tab") { actions.press(RemoteKey.Tab) }
        StripKey("←") { actions.press(RemoteKey.Left) }
        StripKey("↑") { actions.press(RemoteKey.Up) }
        StripKey("↓") { actions.press(RemoteKey.Down) }
        StripKey("→") { actions.press(RemoteKey.Right) }
    }
}

@Composable
private fun StripKey(
    text: String,
    on: Boolean = false,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    Box(
        Modifier
            .height(Tok.Size.controlL.dp)
            .widthIn(min = Tok.Phone.hit.dp)
            .clip(CircleShape)
            .background(if (on) s.accent else s.view)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = Tok.Space.p12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold),
            color = if (on) s.onAccent else s.label1,
            maxLines = 1,
        )
    }
}

/**
 * Slides, eyes-free: the whole accent plane is Next, a swipe right or the Previous zone goes back, the volume keys do
 * both. The timer counts from the plane's start, once a second.
 */
@Composable
private fun Presenter(
    actions: RemoteActions,
    go: (RemoteMode) -> Unit,
    onScreen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    var seconds by rememberSaveable { mutableIntStateOf(0) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(SECOND_MS)
            seconds++
        }
    }
    val next = { actions.press(RemoteKey.PageDown) }
    val previous = { actions.press(RemoteKey.PageUp) }
    val area = stringResource(R.string.presenter_area)
    val key = s.onAccent.copy(alpha = Tok.Phone.planeKey)
    Box(
        modifier
            .background(s.accent)
            .focusRequester(focus)
            .focusable()
            .onPreviewKeyEvent { event ->
                val action =
                    when (event.key.nativeKeyCode) {
                        AndroidKeyEvent.KEYCODE_VOLUME_DOWN -> next
                        AndroidKeyEvent.KEYCODE_VOLUME_UP -> previous
                        else -> return@onPreviewKeyEvent false
                    }
                if (event.type == KeyEventType.KeyDown) action()
                true
            }.pointerInput(actions) {
                awaitEachGesture {
                    awaitFirstDown()
                    var dx = 0f
                    var travelled = 0f
                    // A key on the plane took the touch: it is no slide gesture.
                    var taken = false
                    do {
                        val event = awaitPointerEvent()
                        taken = taken || event.changes.any { it.isConsumed }
                        event.changes.forEach {
                            val d = it.positionChange()
                            dx += d.x
                            travelled += d.getDistance()
                            it.consume()
                        }
                    } while (event.changes.any { it.pressed })
                    when {
                        taken -> Unit
                        dx >
                            Tok.Phone.hit.dp
                                .toPx()
                        -> previous()
                        travelled < viewConfiguration.touchSlop -> next()
                    }
                }
            }.semantics { contentDescription = area },
    ) {
        BackKey(s.onAccent, { go(RemoteMode.Pad) }, Modifier.padding(start = Tok.Space.p8.dp, top = Tok.Space.p4.dp))
        Box(
            Modifier
                .align(Alignment.TopEnd)
                .padding(top = Tok.Space.p8.dp, end = Tok.Space.p12.dp)
                .height(Tok.Size.controlL.dp)
                .clip(CircleShape)
                .background(key)
                .clickable(role = Role.Button) {
                    actions.press(RemoteKey.Escape)
                    go(RemoteMode.Pad)
                }.padding(horizontal = Tok.Phone.pillPad.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                stringResource(R.string.presenter_end_show),
                t.body.copy(fontWeight = FontWeight.SemiBold),
                color = s.onAccent,
                maxLines = 1,
            )
        }
        val prev = stringResource(R.string.presenter_previous_slide)
        Box(
            Modifier
                .align(Alignment.CenterStart)
                .padding(start = Tok.Space.p12.dp)
                .size(Tok.Size.rowTwo.dp, Tok.Phone.prevZone.dp)
                .clip(CircleShape)
                .background(key)
                .clickable(role = Role.Button, onClick = previous)
                .semantics { contentDescription = prev },
            contentAlignment = Alignment.Center,
        ) { Symbol(Icons.Prev, s.onAccent, size = Tok.Size.controlS.dp) }
        Column(
            Modifier.align(Alignment.Center).padding(horizontal = Tok.Space.p32.dp),
            verticalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(stringResource(R.string.presenter_tap), t.title3, color = s.onAccent, align = TextAlign.Center)
            Text(
                "${seconds / 60}:${(seconds % 60).toString().padStart(2, '0')}",
                t.large.copy(
                    fontSize = t.large.fontSize * Tok.Phone.timerScale,
                    lineHeight = t.large.fontSize * Tok.Phone.timerScale,
                    fontWeight = FontWeight.SemiBold,
                    fontFeatureSettings = "tnum",
                    letterSpacing = TIMER_TRACKING.em,
                ),
                color = s.onAccent,
                maxLines = 1,
            )
            Text(stringResource(R.string.presenter_how), t.footnote, color = s.onAccent, align = TextAlign.Center)
        }
        Row(
            Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding()
                .padding(bottom = Tok.Phone.dockInset.dp),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
        ) {
            PlaneKey(Icons.Kbd, stringResource(R.string.remote_keys), key, s.onAccent) { go(RemoteMode.Keys) }
            PlaneKey(Icons.Pres, stringResource(R.string.presenter_title), s.onAccent, s.accent) { go(RemoteMode.Pad) }
            PlaneKey(Icons.Screen, stringResource(R.string.remote_screen), key, s.onAccent, onScreen)
        }
    }
}

@Composable
private fun PlaneKey(
    icon: ImageVector,
    label: String,
    fill: Color,
    ink: Color,
    onClick: () -> Unit,
) {
    Box(
        Modifier
            .size(Tok.Phone.roundKeyL.dp)
            .clip(CircleShape)
            .background(fill)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Symbol(icon, ink, size = Tok.Size.controlS.dp) }
}

private val MODIFIERS = listOf("ctrl", "alt", "super", "shift")
private const val TAP_MS = 250L
private const val HOLD_MS = 400L
private const val SECOND_MS = 1000L

/** The tips start this far down the pad. */
private const val TIPS_TOP = 0.36f

/** The arrow cross is this much wider than Esc, Tab and Enter. */
private const val CROSS_WEIGHT = 1.25f
private const val TIMER_TRACKING = -0.02f
