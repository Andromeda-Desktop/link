package org.andromeda.link.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

/** Old role names over the token scheme; each screen moves to [LinkTheme.scheme] when it is rebuilt. */
@Immutable
data class LinkColors(
    val dark: Boolean,
    val surfacePrimary: Color,
    val surfaceSecondary: Color,
    val surfaceTertiary: Color,
    val surfaceElevated: Color,
    val surfaceInput: Color,
    val surfaceOverlay: Color,
    val textPrimary: Color,
    val textSecondary: Color,
    val textTertiary: Color,
    val textInverse: Color,
    val textDisabled: Color,
    val accent: Color,
    /** The accent as text, readable on the surface. */
    val accentText: Color,
    /** Text and glyphs on an accent fill. */
    val onAccent: Color,
    val borderPrimary: Color,
    val borderSecondary: Color,
    val success: Color,
    val warning: Color,
    val error: Color,
)

private fun colors(
    s: Tok.SchemeToken,
    dark: Boolean,
) = LinkColors(
    dark = dark,
    surfacePrimary = s.solidRegular,
    surfaceSecondary = s.solidRegular,
    surfaceTertiary = s.fill1,
    surfaceElevated = s.view,
    surfaceInput = s.fill1,
    surfaceOverlay = s.scrim,
    textPrimary = s.label1,
    textSecondary = s.label2,
    textTertiary = s.label3,
    textInverse = s.view,
    textDisabled = s.label3,
    accent = s.accent,
    accentText = s.accentText,
    onAccent = s.onAccent,
    borderPrimary = s.stroke,
    borderSecondary = s.separator,
    success = s.success,
    warning = s.warning,
    error = s.dangerText,
)

val LightColors = colors(Tok.Scheme.light, dark = false)
val DarkColors = colors(Tok.Scheme.dark, dark = true)
