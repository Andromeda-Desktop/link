package org.andromeda.link.ui.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.unit.dp

/** Old dimension names over the tokens; each screen moves to [Tok] when it is rebuilt. */
object Space {
    val s2 = Tok.Space.p2.dp
    val s4 = Tok.Space.p4.dp
    val s8 = Tok.Space.p8.dp
    val s12 = Tok.Space.p12.dp
    val s16 = Tok.Space.p16.dp
    val s20 = Tok.Space.p20.dp
    val s24 = Tok.Space.p24.dp
    val s32 = Tok.Space.p32.dp
    val s48 = Tok.Space.p48.dp
    val page = Tok.Space.p20.dp
    val section = Tok.Space.p16.dp
    val block = Tok.Space.p12.dp

    // No token: the rebuilt screens use the 4-step grid only.
    val s6 = 6.dp
    val s10 = 10.dp
    val s14 = 14.dp
    val s28 = 28.dp
}

object Radius {
    val micro = RoundedCornerShape(Tok.Radius.r4.dp)
    val small = RoundedCornerShape(Tok.Radius.r8.dp)
    val button = RoundedCornerShape(Tok.Radius.r12.dp)
    val list = RoundedCornerShape(Tok.Radius.r16.dp)
    val hero = RoundedCornerShape(Tok.Radius.r20.dp)
    val softTech = RoundedCornerShape(Tok.Radius.r28.dp)
    val pill = RoundedCornerShape(Tok.Radius.pill.dp)
}

object Size {
    val iconSmall = Tok.Size.symbol.dp
    val icon = Tok.Size.controlS.dp
    val iconLarge = Tok.Size.controlM.dp
    val iconContainer = Tok.Size.controlL.dp
    val pill = Tok.Size.controlXl.dp
    val touch = Tok.Phone.hit.dp
    val touchLarge = Tok.Size.rowTwo.dp
}

/** Tween timings of the old screens; rebuilt screens use springs from [Tok.Spring]. */
object Motion {
    const val BUTTON_PRESS = 100
    const val FADE_OUT = 150
    const val FAST = 200
    const val MEDIUM_FAST = 250
    const val NORMAL = 320
    const val MORPH = 380
    const val SLOW = 500
    val standard = CubicBezierEasing(0.645f, 0.045f, 0.355f, 1f)
    val enter = CubicBezierEasing(0.215f, 0.61f, 0.355f, 1f)
    val exit = CubicBezierEasing(0.55f, 0.055f, 0.675f, 0.19f)
    val smoothEnter = CubicBezierEasing(0.165f, 0.84f, 0.44f, 1f)
}
