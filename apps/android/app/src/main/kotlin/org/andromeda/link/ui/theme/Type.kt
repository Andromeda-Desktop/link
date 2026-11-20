package org.andromeda.link.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import org.andromeda.link.R

/** Inter (SIL OFL 1.1, bundled with its license in assets/licenses), one instance per token weight. */
@OptIn(ExperimentalTextApi::class)
val Inter =
    FontFamily(
        listOf(300, 400, 500, 600).map {
            Font(R.font.inter_variable, FontWeight(it), variationSettings = FontVariation.Settings(FontVariation.weight(it)))
        },
    )

/** The token type roles at phone size: desktop size x text_scale_l, in sp so text follows the user's size. */
@Immutable
data class PhoneType(
    val caption: TextStyle = phone(Tok.Type.caption),
    val footnote: TextStyle = phone(Tok.Type.footnote),
    val body: TextStyle = phone(Tok.Type.body, singleLine = true),
    val paragraph: TextStyle = phone(Tok.Type.paragraph),
    val headline: TextStyle = phone(Tok.Type.headline, singleLine = true),
    val title3: TextStyle = phone(Tok.Type.title3),
    val title2: TextStyle = phone(Tok.Type.title2),
    val title1: TextStyle = phone(Tok.Type.title1),
    val large: TextStyle = phone(Tok.Type.large),
    val mono: TextStyle = phone(Tok.Type.mono).copy(fontFamily = FontFamily.Monospace),
)

/** Token tracking is already in em. [singleLine] roles (row text) take the unscaled paragraph line, as the mockups do. */
private fun phone(
    t: Tok.TypeToken,
    singleLine: Boolean = false,
): TextStyle {
    val scale = Tok.TypeRules.textScaleL
    return TextStyle(
        fontFamily = Inter,
        fontWeight = FontWeight(t.weight),
        fontSize = (t.size * scale).sp,
        lineHeight = (if (singleLine) Tok.Type.paragraph.line else t.line * scale).sp,
        letterSpacing = t.tracking.em,
        fontFeatureSettings = "tnum",
    )
}
