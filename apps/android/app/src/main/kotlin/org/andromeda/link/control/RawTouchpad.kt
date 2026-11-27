package org.andromeda.link.control

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.SystemClock
import android.provider.Settings
import android.util.DisplayMetrics
import android.view.MotionEvent
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.changedToDownIgnoreConsumed
import androidx.compose.ui.input.pointer.motionEventSpy
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.andromeda.link.core.domain.PointerButton
import org.andromeda.link.core.domain.RemoteInput
import org.andromeda.link.core.domain.TouchContact
import kotlin.math.abs

/**
 * One finger on its own, kept on the phone: [move] moves the phone's pointer by a finger's change in px, and (0, 0)
 * puts the desktop's pointer where the phone shows it; [send] presses and clicks there.
 */
class Aim(
    val move: (dx: Float, dy: Float) -> Unit,
    val send: (RemoteInput) -> Unit,
)

/** Who takes a gesture's fingers: the phone's [Aim] (still, moving or holding the left button), or the desktop. */
private enum class Gesture { Still, Aiming, Held, Raw }

/**
 * Reports every finger on this area each frame, in 1/100 mm from its top left, for a desktop that makes a real
 * touchpad of them; an empty frame once the last finger lifts. The desktop's own settings turn them into moves,
 * taps, scrolls and gestures. [metrics] gives the screen's real pixel density. With [aim], one finger alone stays on
 * the phone: it moves, a tap clicks and a still finger holds the left button until it lifts; a second finger hands
 * the gesture to the desktop.
 */
@OptIn(ExperimentalComposeUiApi::class)
fun Modifier.rawTouchpad(
    metrics: DisplayMetrics,
    aim: Aim? = null,
    onPinch: ((centroid: Offset, factor: Float, moved: Offset) -> Unit)? = null,
    onTick: () -> Unit = {},
    onFrame: (List<TouchContact>) -> Unit,
): Modifier {
    // Pixels to 1/100 mm: an inch is 2540 of them.
    val (perX, perY) = UNITS_PER_INCH / metrics.xdpi to UNITS_PER_INCH / metrics.ydpi
    var cancelled = false
    return motionEventSpy { if (it.actionMasked == MotionEvent.ACTION_CANCEL) cancelled = true }.pointerInput(metrics, aim) {
        val ids = mutableMapOf<PointerId, Int>()
        var downAt = 0L
        var lift: Job? = null
        var hold: Job? = null
        var startSpread = 0f
        var pinching = false
        val starts = mutableMapOf<PointerId, Offset>()
        var fingers = 0
        var travel = 0f
        var swiped = false
        var touching = false
        var gesture = Gesture.Raw
        coroutineScope {
            awaitPointerEventScope {
                while (true) {
                    val event = awaitPointerEvent()
                    lift?.cancel()
                    val pressed = event.changes.filter { it.pressed }
                    if (!touching && pressed.isNotEmpty()) {
                        downAt = SystemClock.uptimeMillis()
                        fingers = 0
                        travel = 0f
                        swiped = false
                        gesture = if (aim == null) Gesture.Raw else Gesture.Still
                        if (aim != null) {
                            hold =
                                launch {
                                    delay(HOLD_MS)
                                    if (gesture == Gesture.Still) {
                                        gesture = Gesture.Held
                                        aim.move(0f, 0f)
                                        aim.send(RemoteInput.Press(PointerButton.Left))
                                    }
                                }
                        }
                    }
                    touching = pressed.isNotEmpty()
                    var still = 0f
                    for (change in pressed) {
                        val start = starts.getOrPut(change.id) { change.position }
                        still = maxOf(still, (change.position - start).getDistance())
                        val moved = (change.position - start).let { Offset(it.x * perX, it.y * perY) }.getDistance()
                        travel = maxOf(travel, moved)
                    }
                    fingers = maxOf(fingers, pressed.size)
                    if (aim != null && gesture != Gesture.Raw) {
                        if (gesture != Gesture.Held && pressed.size >= 2) {
                            gesture = Gesture.Raw
                        } else if (gesture == Gesture.Still && still > viewConfiguration.touchSlop) {
                            gesture = Gesture.Aiming
                            val (one) = pressed
                            (one.position - starts.getValue(one.id)).let { aim.move(it.x, it.y) }
                        } else if (gesture != Gesture.Still && pressed.isNotEmpty()) {
                            val change = pressed.fold(Offset.Zero) { sum, c -> sum + c.positionChange() } / pressed.size.toFloat()
                            aim.move(change.x, change.y)
                        }
                        if (gesture != Gesture.Still) hold?.cancel()
                    }
                    if (!swiped && fingers >= 3 && travel > SWIPE_TICK) {
                        swiped = true
                        onTick()
                    }
                    val contacts =
                        if (gesture != Gesture.Raw) {
                            emptyList()
                        } else {
                            pressed.mapNotNull { change ->
                                val id = ids.getOrPut(change.id) { (0..MAX_ID).firstOrNull { it !in ids.values } ?: return@mapNotNull null }
                                TouchContact(id, units(change.position.x * perX), units(change.position.y * perY))
                            }
                        }
                    event.changes.filterNot { it.pressed }.forEach {
                        ids.remove(it.id)
                        starts.remove(it.id)
                    }
                    event.changes.forEach { it.consume() }
                    if (gesture != Gesture.Raw) {
                        if (pressed.isEmpty()) {
                            hold?.cancel()
                            when {
                                gesture == Gesture.Held -> aim?.send(RemoteInput.Release(PointerButton.Left))
                                gesture == Gesture.Still && !cancelled && SystemClock.uptimeMillis() - downAt < AIM_TAP_MS -> {
                                    aim?.move(0f, 0f)
                                    aim?.send(RemoteInput.Click(PointerButton.Left))
                                }
                            }
                            cancelled = false
                        }
                        continue
                    }
                    if (pressed.isEmpty()) pinching = false
                    if (onPinch != null && pressed.size == 2) {
                        val (a, b) = pressed
                        val spread = (a.position - b.position).getDistance()
                        if (event.changes.any { it.changedToDownIgnoreConsumed() }) startSpread = spread
                        if (!pinching && startSpread > 0f && abs(spread / startSpread - 1f) > PINCH_START) {
                            // From here the fingers zoom this phone's picture; the desktop sees them lift.
                            pinching = true
                            onTick()
                            onFrame(emptyList())
                        }
                        if (pinching) {
                            val before = (a.previousPosition - b.previousPosition).getDistance()
                            val moved = (a.positionChange() + b.positionChange()) / 2f
                            onPinch((a.position + b.position) / 2f, if (before > 0f) spread / before else 1f, moved)
                        }
                    }
                    if (pinching) continue
                    if (contacts.isEmpty() && cancelled) {
                        // A system gesture took the touch: lifting at once would read as a tap on the desktop.
                        cancelled = false
                        val wait = CANCELLED_LIFT_MS - (SystemClock.uptimeMillis() - downAt)
                        lift =
                            launch {
                                delay(wait)
                                onFrame(emptyList())
                            }
                    } else {
                        // The desktop's own tap rule: lifted within its timeout, without moving.
                        if (contacts.isEmpty() && SystemClock.uptimeMillis() - downAt < TAP_MS && travel < TAP_SLOP) onTick()
                        onFrame(contacts.take(MAX_CONTACTS))
                    }
                }
            }
        }
    }
}

/** An aimed tap lifts within this; a finger still for [HOLD_MS] holds the left button. */
private const val AIM_TAP_MS = 250L
private const val HOLD_MS = 400L

/** How far two fingers must spread or close, as a fraction of their first distance, to zoom instead of scroll. */
private const val PINCH_START = 0.15f

/** libinput's tap timeout, and how far a tapping finger may move, in 1/100 mm. */
private const val TAP_MS = 180L
private const val TAP_SLOP = 200f

/** How far 3 or more fingers move, in 1/100 mm, before the phone ticks for a swipe. */
private const val SWIPE_TICK = 500f

/** Longer than libinput's 180 ms tap timeout. */
private const val CANCELLED_LIFT_MS = 250L

private fun units(value: Float): Int = value.toInt().coerceIn(0, MAX_POSITION)

private const val UNITS_PER_INCH = 2540f

/** link-proto's MAX_TOUCH_ID, MAX_TOUCH_CONTACTS and MAX_TOUCH_POSITION. */
private const val MAX_ID = 9
private const val MAX_CONTACTS = 5
private const val MAX_POSITION = 30_000

/**
 * OxygenOS and ColorOS take every 3-finger touch for their own gestures and cancel it in this app. Their
 * remote-control switch turns those gestures off, as their cast apps do. Returns what puts the old value back once the
 * last holder lets go, or null when this phone has no such switch or the app lacks the grant. Main thread only.
 */
fun keepMultiFingerTouches(context: Context): (() -> Unit)? {
    if (!canKeepMultiFingerTouches(context)) return null
    val resolver = context.contentResolver
    if (multiFingerHolders++ == 0) {
        multiFingerBefore = Settings.Global.getInt(resolver, OPLUS_REMOTE_CONTROL_GESTURES, 0)
        Settings.Global.putInt(resolver, OPLUS_REMOTE_CONTROL_GESTURES, 1)
    }
    return { if (--multiFingerHolders == 0) Settings.Global.putInt(resolver, OPLUS_REMOTE_CONTROL_GESTURES, multiFingerBefore) }
}

private var multiFingerHolders = 0
private var multiFingerBefore = 0

/** True on an OxygenOS or ColorOS phone where this app lacks the adb grant that [keepMultiFingerTouches] needs. */
fun needsMultiFingerGrant(context: Context): Boolean = isOplus() && !canKeepMultiFingerTouches(context)

private fun canKeepMultiFingerTouches(context: Context): Boolean =
    isOplus() && context.checkSelfPermission(Manifest.permission.WRITE_SECURE_SETTINGS) == PackageManager.PERMISSION_GRANTED

private fun isOplus(): Boolean = Build.MANUFACTURER.lowercase() in setOf("oneplus", "oppo", "realme")

private const val OPLUS_REMOTE_CONTROL_GESTURES = "remote_control_input_gesture_switch"
