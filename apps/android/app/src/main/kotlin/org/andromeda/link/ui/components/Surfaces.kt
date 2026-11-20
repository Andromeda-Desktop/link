package org.andromeda.link.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Motion
import org.andromeda.link.ui.theme.Radius
import org.andromeda.link.ui.theme.Size
import org.andromeda.link.ui.theme.Space
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation

/** Text in a type role; the design system's only text primitive. */
@Composable
fun Label(
    text: String,
    style: TextStyle = LinkTheme.type.bodyLarge,
    color: Color = LinkTheme.colors.textPrimary,
    modifier: Modifier = Modifier,
    maxLines: Int = Int.MAX_VALUE,
    textAlign: TextAlign = TextAlign.Start,
) {
    BasicText(text, modifier, style.copy(color = color, textAlign = textAlign), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
}

@Composable
fun Glyph(
    icon: ImageVector,
    tint: Color = LinkTheme.colors.textPrimary,
    size: Dp = Size.icon,
    modifier: Modifier = Modifier,
) {
    Image(rememberVectorPainter(icon), contentDescription = null, modifier = modifier.size(size), colorFilter = ColorFilter.tint(tint))
}

/** A press shrinks the surface slightly, as the soft-tech card does. */
@Composable
private fun Modifier.pressScale(
    source: MutableInteractionSource,
    scale: Float = 0.99f,
): Modifier {
    val pressed by source.collectIsPressedAsState()
    val current by animateFloatAsState(if (pressed) scale else 1f, tween(Motion.BUTTON_PRESS, easing = Motion.enter), label = "press")
    return graphicsLayer {
        scaleX = current
        scaleY = current
    }
}

/** The canonical container: a hairline outline on the page itself, so structure comes from line, not lift. */
@Composable
fun SoftCard(
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    padding: Dp = Space.s20,
    content: @Composable ColumnScope.() -> Unit,
) {
    val colors = LinkTheme.colors
    val source = remember { MutableInteractionSource() }
    val shape = Radius.hero
    Column(
        modifier =
            modifier
                .then(if (onClick != null) Modifier.pressScale(source) else Modifier)
                .clip(shape)
                .border(1.dp, colors.borderPrimary, shape)
                .then(
                    if (onClick !=
                        null
                    ) {
                        Modifier.clickable(source, indication = LocalIndication.current, onClick = onClick)
                    } else {
                        Modifier
                    },
                ).padding(padding),
        content = content,
    )
}

/** The screen's one lead element: outlined in the accent, the only colored outline on a screen. */
@Composable
fun HeroSurface(
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
    val shape = Radius.hero
    Column(
        modifier = modifier.clip(shape).border(1.5.dp, LinkTheme.colors.accent, shape).padding(Space.s24),
        content = content,
    )
}

/**
 * A screen: an optional back glyph and a title over content with the 20 dp page gutter, and a toast host for the
 * screen's one-line messages. A null [title] drops the bar, for a screen whose content opens with its own heading.
 */
@Composable
fun Screen(
    title: String?,
    onBack: (() -> Unit)? = null,
    toast: String? = null,
    onToastShown: () -> Unit = {},
    /** Room under the toast: more on a screen with a floating dock. */
    toastInset: Dp = Tok.Space.p24.dp,
    actions: @Composable () -> Unit = {},
    overlay: @Composable BoxScope.() -> Unit = {},
    background: Color = LinkTheme.colors.surfacePrimary,
    content: @Composable ColumnScope.() -> Unit,
) {
    val colors = LinkTheme.colors
    Box(Modifier.fillMaxSize().background(background)) {
        Column(Modifier.fillMaxSize().statusBarsPadding()) {
            if (title != null || onBack != null) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = Space.s8, vertical = Space.s8),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (onBack != null) {
                        Box(
                            modifier =
                                Modifier
                                    .size(Size.touch)
                                    .clip(Radius.pill)
                                    .clickable(role = Role.Button, onClick = onBack)
                                    .semantics { contentDescription = "Back" },
                            contentAlignment = Alignment.Center,
                        ) { Glyph(Icons.AutoMirrored.Filled.ArrowBack) }
                    } else {
                        Box(Modifier.size(Space.s12))
                    }
                    if (title !=
                        null
                    ) {
                        Label(
                            title,
                            LinkTheme.type.displaySmall,
                            modifier = Modifier.weight(1f).padding(horizontal = Space.s4),
                            maxLines = 1,
                        )
                    }
                    actions()
                }
            }
            Column(Modifier.weight(1f).fillMaxWidth(), content = content)
        }
        overlay()
        ToastHost(toast, onToastShown, toastInset, Modifier.align(Alignment.BottomCenter))
    }
}

/** A message over the bottom edge for a few seconds (rulebook §9.11): a 40 capsule, or a r20 card when it wraps. */
@Composable
private fun ToastHost(
    message: String?,
    onShown: () -> Unit,
    inset: Dp,
    modifier: Modifier = Modifier,
) {
    LaunchedEffect(message) {
        if (message != null) {
            delay(TOAST_MS)
            onShown()
        }
    }
    // The last text stays while the toast slides out.
    var shown by remember { mutableStateOf("") }
    if (message != null) shown = message
    AnimatedVisibility(
        visible = message != null,
        modifier = modifier.navigationBarsPadding().padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = inset),
        enter = slideInVertically(tween(Motion.NORMAL, easing = Motion.smoothEnter)) { it } + fadeIn(tween(Motion.FAST)),
        exit = slideOutVertically(tween(Motion.FAST, easing = Motion.exit)) { it } + fadeOut(tween(Motion.FADE_OUT)),
    ) {
        val s = LinkTheme.scheme
        val radius = Tok.Size.controlL.dp / 2
        val shape = RoundedCornerShape(radius)
        Box(
            Modifier
                .heightIn(min = Tok.Size.controlL.dp)
                .elevation(Tok.Elevation.e3, radius)
                .clip(shape)
                .background(s.view)
                .border(Dp.Hairline, s.stroke, shape)
                .padding(horizontal = Tok.Space.p16.dp, vertical = Tok.Space.p8.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(shown, LinkTheme.phoneType.body, color = s.label1, align = TextAlign.Center)
        }
    }
}

private const val TOAST_MS = 4_000L

/** A section header: title, and a quieter note after it. */
@Composable
fun SectionHeader(
    title: String,
    note: String? = null,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier.padding(bottom = Space.block),
        horizontalArrangement = Arrangement.spacedBy(Space.s8),
        verticalAlignment = Alignment.Bottom,
    ) {
        Label(title, LinkTheme.type.titleLarge)
        if (note != null) Label(note, LinkTheme.type.bodySmall, LinkTheme.colors.textSecondary)
    }
}
