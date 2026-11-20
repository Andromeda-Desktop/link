package org.andromeda.link.ui.kit

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
import org.andromeda.link.ui.theme.spring

/** A screen below the status bar, with an [overlay] over it and a toast host for the screen's one-line messages. */
@Composable
fun Screen(
    modifier: Modifier = Modifier,
    toast: String? = null,
    onToastEnd: () -> Unit = {},
    /** Room under the toast: more on a screen with a floating dock. */
    toastInset: Dp = Tok.Space.p24.dp,
    background: Color = LinkTheme.scheme.solidRegular,
    overlay: @Composable BoxScope.() -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    Box(modifier.fillMaxSize().background(background)) {
        Column(Modifier.fillMaxSize().statusBarsPadding(), content = content)
        overlay()
        ToastHost(toast, onToastEnd, toastInset, Modifier.align(Alignment.BottomCenter))
    }
}

/** A message over the bottom edge for a few seconds (rulebook §9.11): a 40 capsule, or a r20 card when it wraps. */
@Composable
private fun ToastHost(
    message: String?,
    onEnd: () -> Unit,
    inset: Dp,
    modifier: Modifier = Modifier,
) {
    val end by rememberUpdatedState(onEnd)
    LaunchedEffect(message) {
        if (message != null) {
            delay(TOAST_MS)
            end()
        }
    }
    // The last text stays while the toast slides out.
    var shown by remember { mutableStateOf("") }
    if (message != null) shown = message
    AnimatedVisibility(
        visible = message != null,
        modifier = modifier.navigationBarsPadding().padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = inset),
        enter = slideInVertically(spring<IntOffset>(Tok.Spring.open)) { it } + fadeIn(spring(Tok.Spring.fade)),
        exit = slideOutVertically(spring<IntOffset>(Tok.Spring.close)) { it } + fadeOut(spring(Tok.Spring.fadeOut)),
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
