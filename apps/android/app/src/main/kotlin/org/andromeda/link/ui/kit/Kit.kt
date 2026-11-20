package org.andromeda.link.ui.kit

import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.Image
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.indication
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import org.andromeda.link.R
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
import org.andromeda.link.ui.theme.spring

/** Text in a phone type role. */
@Composable
fun Text(
    text: String,
    style: TextStyle,
    modifier: Modifier = Modifier,
    color: Color = LinkTheme.scheme.label1,
    maxLines: Int = Int.MAX_VALUE,
    align: TextAlign = TextAlign.Start,
) {
    BasicText(text, modifier, style.copy(color = color, textAlign = align), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
}

@Composable
fun Symbol(
    icon: ImageVector,
    tint: Color,
    modifier: Modifier = Modifier,
    size: Dp = Tok.Phone.symbol.dp,
) {
    Image(rememberVectorPainter(icon), contentDescription = null, modifier = modifier.size(size), colorFilter = ColorFilter.tint(tint))
}

enum class CapsuleKind { Neutral, Primary, Dark, Danger }

/** A one-shot action: fill_2, accent, label_1 or danger text on fill_1; r = h / 2. */
@Composable
fun Capsule(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    kind: CapsuleKind = CapsuleKind.Neutral,
    large: Boolean = false,
    enabled: Boolean = true,
) {
    val s = LinkTheme.scheme
    val (fill, ink) =
        when (kind) {
            CapsuleKind.Neutral -> s.fill2 to s.label1
            CapsuleKind.Primary -> s.accent to s.onAccent
            CapsuleKind.Dark -> s.label1 to s.view
            CapsuleKind.Danger -> s.fill1 to s.dangerText
        }
    Row(
        modifier
            .height(if (large) Tok.Size.rowTwo.dp else Tok.Phone.hit.dp)
            .alpha(if (enabled) 1f else Tok.Phone.offAlpha)
            .clip(CircleShape)
            .background(fill)
            .clickable(enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = Tok.Space.p16.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) Symbol(icon, ink, size = Tok.Size.symbolL.dp)
        Text(text, LinkTheme.phoneType.headline, color = ink, maxLines = 1)
    }
}

enum class PillKind { Quiet, Primary }

/** A small action: 32 visible inside a 48 hit; Quiet = fill_2 with accent text. */
@Composable
fun Pill(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    kind: PillKind = PillKind.Quiet,
) {
    val s = LinkTheme.scheme
    val (fill, ink) = if (kind == PillKind.Primary) s.accent to s.onAccent else s.fill2 to s.accentText
    val press = remember { MutableInteractionSource() }
    Box(
        modifier
            .heightIn(min = Tok.Phone.hit.dp)
            .clickable(press, indication = null, role = Role.Button, onClick = onClick)
            .padding(horizontal = Tok.Space.p2.dp),
        contentAlignment = Alignment.Center,
    ) {
        Row(
            Modifier
                .height(Tok.Size.controlM.dp)
                .clip(CircleShape)
                .background(fill)
                .indication(press, LocalIndication.current)
                .padding(horizontal = Tok.Phone.pillPad.dp),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (icon != null) Symbol(icon, ink, size = Tok.Size.symbol.dp)
            Text(text, LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold), color = ink, maxLines = 1)
        }
    }
}

/** A view card r16 with a 0.5 stroke ring; rows inside are separated by full-width separators. */
@Composable
fun GroupCard(
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
    val s = LinkTheme.scheme
    val shape = RoundedCornerShape(Tok.Radius.r16.dp)
    Column(
        modifier
            .fillMaxWidth()
            .clip(shape)
            .background(s.view)
            .border(Dp.Hairline, s.stroke, shape),
        content = content,
    )
}

/** The separator line: 1 dp snapped to whole device pixels (rulebook Hairline). Dp.Hairline is 0 dp outside border(). */
@Composable
fun hairline(): Dp = with(LocalDensity.current) { 1.dp.roundToPx().toDp() }

@Composable
fun Separator(modifier: Modifier = Modifier) {
    Box(
        modifier
            .fillMaxWidth()
            .height(hairline())
            .background(LinkTheme.scheme.separator),
    )
}

/** The row frame: symbol 22, name, trailing control; 56 min height, one line by default. */
@Composable
private fun RowFrame(
    title: String,
    subtitle: String?,
    icon: ImageVector?,
    enabled: Boolean,
    modifier: Modifier = Modifier,
    on: Boolean = false,
    accent: Boolean = false,
    leading: (@Composable () -> Unit)? = null,
    trailing: @Composable RowScope.() -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = Tok.Size.rowTwo.dp)
            .padding(horizontal = Tok.Space.p16.dp, vertical = Tok.Space.p8.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        leading?.invoke()
        if (icon != null) {
            Symbol(
                icon,
                if (!enabled) {
                    s.label3
                } else if (on || accent) {
                    s.accent
                } else {
                    s.label2
                },
            )
        }
        Column(Modifier.weight(1f)) {
            when {
                !enabled -> Text(title, t.body, color = s.label3)
                accent -> Text(title, t.body.copy(fontWeight = FontWeight.SemiBold), color = s.accentText)
                else -> Text(title, t.body)
            }
            if (subtitle != null) Text(subtitle, t.footnote, color = s.label2)
        }
        trailing()
    }
}

/** A row whose trailing side shows a value, a pill or a chevron; the whole row opens something when [onClick] is set. */
@Composable
fun ValueRow(
    title: String,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    subtitle: String? = null,
    value: String? = null,
    chevron: ImageVector? = null,
    accent: Boolean = false,
    onClick: (() -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: @Composable RowScope.() -> Unit = {},
) {
    val s = LinkTheme.scheme
    RowFrame(
        title,
        subtitle,
        icon,
        true,
        if (onClick != null) modifier.clickable(onClick = onClick) else modifier,
        accent = accent,
        leading = leading,
    ) {
        if (value !=
            null
        ) {
            Text(
                value,
                LinkTheme.phoneType.body,
                color = s.label2,
                modifier = Modifier.widthIn(max = Tok.Phone.valueMax.dp),
                maxLines = 1,
                align = TextAlign.End,
            )
        }
        trailing()
        if (chevron != null) Symbol(chevron, s.label3, size = Tok.Size.symbol.dp)
    }
}

/** A row with a switch; the whole row toggles. The knob moves on the snap spring, the only motion here. */
@Composable
fun SwitchRow(
    title: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    subtitle: String? = null,
    enabled: Boolean = true,
) {
    RowFrame(
        title,
        subtitle,
        icon,
        enabled,
        modifier.toggleable(checked, enabled = enabled, role = Role.Switch, onValueChange = onChange),
        on = checked,
    ) {
        Switch(checked)
    }
}

@Composable
fun Switch(
    checked: Boolean,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val inset = Tok.Space.p2.dp
    val knob = Tok.Size.symbolL.dp
    val travel = Tok.Size.controlL.dp - knob - inset * 2
    val x by animateDpAsState(if (checked) travel else 0.dp, spring(Tok.Spring.snap), label = "knob")
    Box(
        modifier
            .size(Tok.Size.controlL.dp, Tok.Size.controlS.dp)
            .clip(CircleShape)
            .background(if (checked) s.accent else s.fill2)
            .padding(inset),
    ) {
        Box(
            Modifier
                .offset { IntOffset(x.roundToPx(), 0) }
                .size(knob)
                .elevation(Tok.Elevation.e1, knob / 2)
                .background(Color.White, CircleShape),
        )
    }
}

/** A short heading over a group: headline, 32 above, 8 below; [detail] follows it in label_2. */
@Composable
fun SectionTitle(
    text: String,
    modifier: Modifier = Modifier,
    detail: String? = null,
) {
    Row(
        modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p32.dp, bottom = Tok.Space.p8.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        Text(text, LinkTheme.phoneType.headline, maxLines = 1)
        if (detail != null) Text(detail, LinkTheme.phoneType.body, color = LinkTheme.scheme.label2, maxLines = 1)
    }
}

/** One of a few options: fill_1 track, the selected option on view with e1. [tall] = 48, else 40 with a 4 inset. */
@Composable
fun <T> Segmented(
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
    tall: Boolean = false,
) {
    val s = LinkTheme.scheme
    val inset = Tok.Space.p4.dp
    Row(
        modifier
            .fillMaxWidth()
            .height(if (tall) Tok.Phone.hit.dp else Tok.Size.controlL.dp)
            .clip(CircleShape)
            .background(s.fill1)
            .padding(if (tall) 0.dp else inset),
        horizontalArrangement = Arrangement.spacedBy(if (tall) 0.dp else Tok.Space.p2.dp),
    ) {
        for ((value, label) in options) {
            val cur = value == selected
            Box(
                Modifier
                    .weight(1f)
                    .selectable(cur, role = Role.Tab, interactionSource = null, indication = null) { onSelect(value) }
                    .padding(if (tall) inset else 0.dp),
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .then(
                            if (cur) Modifier.elevation(Tok.Elevation.e1, Tok.Radius.r16.dp).background(s.view, CircleShape) else Modifier,
                        ).padding(horizontal = Tok.Space.p8.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        label,
                        LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold),
                        color = if (cur) s.label1 else s.label2,
                        modifier = Modifier.fillMaxWidth().align(Alignment.Center),
                        maxLines = 1,
                        align = TextAlign.Center,
                    )
                }
            }
        }
    }
}

/** A scrolling row of filters: 36 visible in a 48 hit, fill_1; the selected one is an accent tint with accent text. */
@Composable
fun <T> FilterChips(
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
    contentPadding: PaddingValues = PaddingValues(horizontal = Tok.Space.p16.dp),
) {
    val s = LinkTheme.scheme
    Row(
        modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(contentPadding),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
    ) {
        for ((value, label) in options) {
            val cur = value == selected
            Box(
                Modifier
                    .height(Tok.Phone.hit.dp)
                    .selectable(cur, role = Role.Tab, interactionSource = null, indication = null) { onSelect(value) },
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier
                        .height(Tok.Phone.chip.dp)
                        .clip(CircleShape)
                        .background(if (cur) s.accent.copy(alpha = Tok.Phone.chipTint) else s.fill1)
                        .padding(horizontal = Tok.Phone.pillPad.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        label,
                        LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold),
                        color = if (cur) s.accentText else s.label1,
                        maxLines = 1,
                    )
                }
            }
        }
    }
}

/** A floating round key: a view circle, e3 and a stroke ring, symbol 24; the current one is label_1 with view ink. */
@Composable
fun RoundKey(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    size: Dp = Tok.Size.rowTwo.dp,
    current: Boolean = false,
    enabled: Boolean = true,
) {
    val s = LinkTheme.scheme
    Box(
        modifier
            .size(size)
            .elevation(Tok.Elevation.e3, size / 2)
            .clip(CircleShape)
            .background(if (current) s.label1 else s.view)
            .border(Dp.Hairline, s.stroke, CircleShape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Symbol(icon, if (current) s.view else s.label1, size = Tok.Size.controlS.dp)
    }
}

/** Content that scrolls under a floating bar or to the screen edge dissolves into the page plane. */
@Composable
fun BottomFade(
    height: Dp,
    modifier: Modifier = Modifier,
) {
    val plane = LinkTheme.scheme.solidRegular
    Box(
        modifier
            .fillMaxWidth()
            .height(height)
            .background(
                Brush.verticalGradient(
                    0f to plane.copy(alpha = 0f),
                    0.45f to plane.copy(alpha = 0.85f),
                    0.8f to plane,
                ),
            ),
    )
}

/**
 * One offered or moving item: a plain symbol, the name on one line (two for several files), one meta line, an
 * optional hairline progress bar, then at most one pill and a dismiss x.
 */
@Composable
fun OfferRow(
    symbol: ImageVector,
    name: String,
    meta: String,
    modifier: Modifier = Modifier,
    nameLines: Int = 1,
    progress: Float? = null,
    pill: (@Composable () -> Unit)? = null,
    dismissLabel: String? = null,
    onDismiss: (() -> Unit)? = null,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = Tok.Size.rowTwo.dp)
            .padding(start = Tok.Space.p16.dp, end = Tok.Space.p4.dp, top = Tok.Space.p8.dp, bottom = Tok.Space.p8.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Symbol(symbol, s.label2, size = Tok.Size.symbolL.dp)
        Column(Modifier.weight(1f)) {
            Text(name, t.headline, maxLines = nameLines)
            Text(meta, t.footnote, color = s.label2, maxLines = 1)
            if (progress != null) {
                Box(
                    Modifier
                        .padding(top = Tok.Space.p4.dp)
                        .fillMaxWidth()
                        .height(Tok.Space.p4.dp)
                        .clip(CircleShape)
                        .background(s.fill2),
                ) { Box(Modifier.fillMaxWidth(progress.coerceIn(0f, 1f)).height(Tok.Space.p4.dp).background(s.accent)) }
            }
        }
        pill?.invoke()
        if (onDismiss != null) {
            Box(
                Modifier
                    .size(Tok.Phone.hit.dp)
                    .clip(CircleShape)
                    .clickable(role = Role.Button, onClick = onDismiss)
                    .semantics { dismissLabel?.let { contentDescription = it } },
                contentAlignment = Alignment.Center,
            ) { Symbol(Icons.X, s.label2, size = Tok.Size.symbolL.dp) }
        }
    }
}

/** The back key: a 48 hit with the back symbol 24 in [ink]. */
@Composable
fun BackKey(
    ink: Color,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val back = stringResource(R.string.back)
    Box(
        modifier
            .size(Tok.Phone.hit.dp)
            .clip(CircleShape)
            .clickable(role = Role.Button, onClick = onBack)
            .semantics { contentDescription = back },
        contentAlignment = Alignment.Center,
    ) { Symbol(Icons.Back, ink, size = Tok.Size.controlS.dp) }
}
