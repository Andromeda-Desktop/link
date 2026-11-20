package org.andromeda.link.ui.kit

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
import org.andromeda.link.ui.theme.spring

/**
 * A bottom sheet over everything on the page: the scrim covers every page layer, the sheet is a view plane r28 inset 8
 * with e4 and a grab bar. Opens on the open spring, closes on close; back and a tap on the scrim close it.
 */
@Composable
fun BoxScope.Sheet(
    open: Boolean,
    onDismiss: () -> Unit,
    title: String,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val s = LinkTheme.scheme
    if (open) BackHandler(onBack = onDismiss)
    AnimatedVisibility(open, enter = fadeIn(spring(Tok.Spring.fade)), exit = fadeOut(spring(Tok.Spring.fadeOut))) {
        Box(
            Modifier
                .fillMaxSize()
                .background(s.scrim)
                .clickable(remember { MutableInteractionSource() }, indication = null, onClick = onDismiss),
        )
    }
    AnimatedVisibility(
        open,
        modifier.align(Alignment.BottomCenter),
        enter = slideInVertically(spring<IntOffset>(Tok.Spring.open)) { it } + fadeIn(spring(Tok.Spring.fade)),
        exit = slideOutVertically(spring<IntOffset>(Tok.Spring.close)) { it } + fadeOut(spring(Tok.Spring.fadeOut)),
    ) {
        val shape = RoundedCornerShape(Tok.Radius.r28.dp)
        Column(
            Modifier
                .navigationBarsPadding()
                .padding(Tok.Space.p8.dp)
                .fillMaxWidth()
                .elevation(Tok.Elevation.e4, Tok.Radius.r28.dp)
                .clip(shape)
                .background(s.view)
                .clickable(remember { MutableInteractionSource() }, indication = null) {}
                .padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, top = Tok.Space.p8.dp, bottom = Tok.Space.p12.dp),
        ) {
            Box(
                Modifier
                    .align(Alignment.CenterHorizontally)
                    .size(Tok.Phone.grab.dp, Tok.Space.p4.dp)
                    .clip(CircleShape)
                    .background(s.fill3),
            )
            Text(title, LinkTheme.phoneType.title3, modifier = Modifier.padding(top = Tok.Space.p16.dp))
            if (subtitle !=
                null
            ) {
                Text(subtitle, LinkTheme.phoneType.footnote, color = s.label2, modifier = Modifier.padding(bottom = Tok.Space.p4.dp))
            }
            Column(Modifier.padding(top = Tok.Space.p8.dp).verticalScroll(rememberScrollState()), content = content)
        }
    }
}
