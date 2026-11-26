package org.andromeda.link.ui.theme

import android.app.Activity
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.text.selection.LocalTextSelectionColors
import androidx.compose.foundation.text.selection.TextSelectionColors
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

private val LocalDark = staticCompositionLocalOf { false }
private val LocalScheme = staticCompositionLocalOf { Tok.Scheme.light }
private val LocalPhoneType = staticCompositionLocalOf { PhoneType() }

/** The app's own design system. Screens are built from foundation primitives and these tokens; no Material. */
object LinkTheme {
    val scheme: Tok.SchemeToken @Composable get() = LocalScheme.current
    val phoneType: PhoneType @Composable get() = LocalPhoneType.current
    val dark: Boolean @Composable get() = LocalDark.current
}

/** The dark scheme for one surface that stays dark in both themes (art, the night sky), with no window side effects. */
@Composable
fun DarkSurface(content: @Composable () -> Unit) {
    val scheme = monochrome(Tok.Scheme.dark)
    CompositionLocalProvider(
        LocalDark provides true,
        LocalScheme provides scheme,
        LocalIndication provides PressHighlight(scheme.fill2),
        content = content,
    )
}

@Composable
fun LinkTheme(
    dark: Boolean = preferredDarkTheme(),
    content: @Composable () -> Unit,
) {
    val scheme = monochrome(if (dark) Tok.Scheme.dark else Tok.Scheme.light)
    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as? Activity)?.window ?: return@SideEffect
            val insets = WindowCompat.getInsetsController(window, view)
            insets.isAppearanceLightStatusBars = !dark
            insets.isAppearanceLightNavigationBars = !dark
            window.decorView.setBackgroundColor(scheme.solidRegular.toArgb())
        }
    }
    CompositionLocalProvider(
        LocalDark provides dark,
        LocalScheme provides scheme,
        LocalPhoneType provides PhoneType(),
        LocalIndication provides PressHighlight(scheme.fill2),
        LocalTextSelectionColors provides TextSelectionColors(scheme.accent, scheme.accent.copy(alpha = 0.3f)),
        content = content,
    )
}

// Link on the phone is monochrome: the accent is the primary label (black on light, white on dark).
private fun monochrome(base: Tok.SchemeToken) = base.copy(accent = base.label1, accentText = base.label1, onAccent = base.view)
