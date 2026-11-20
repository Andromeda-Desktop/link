package org.andromeda.link.clipboard

import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/** How phone copies reach Link now; only [ClipboardWatcher] sets it. */
sealed interface CaptureMode {
    /** [LogHelper] reads each copy; as SystemUI under root (no toast), else as the shell. */
    data class Direct(
        val asSystemUi: Boolean,
    ) : CaptureMode

    /** Logcat denial lines bring up [ClipboardOverlay], which reads the copy while focused. */
    data object Denials : CaptureMode

    data class Off(
        val why: OffReason,
    ) : CaptureMode
}

enum class OffReason { Stopped, NoLogAccess, NoOverlay, Background, ReaderEnded }

/** Why a copy did not reach any desktop. */
enum class ClipDrop {
    /** No desktop is live with clipboard access; the newest copy goes out when one connects. */
    NoDesktop,
    SendFailed,
    Empty,

    /** A second copy came while the overlay still waited for focus. */
    OverlayBusy,
    OverlayFailed,

    /** The overlay got no focus in time; the user must tap the notification. */
    NoFocus,
}

sealed interface ClipOutcome {
    data class Sent(
        val desktops: Int,
    ) : ClipOutcome

    data class Dropped(
        val why: ClipDrop,
        val detail: String? = null,
    ) : ClipOutcome
}

data class ClipEvent(
    val at: Long,
    val preview: String?,
    val outcome: ClipOutcome,
)

/** The last [MAX] copy outcomes and the capture mode, for the diagnostics view; each also goes to logcat. */
object ClipEvents {
    private const val MAX = 20
    private const val PREVIEW = 40

    private val _events = MutableStateFlow<List<ClipEvent>>(emptyList())
    val events: StateFlow<List<ClipEvent>> = _events.asStateFlow()

    private val _mode = MutableStateFlow<CaptureMode>(CaptureMode.Off(OffReason.Stopped))
    val mode: StateFlow<CaptureMode> = _mode.asStateFlow()

    fun record(
        text: String?,
        outcome: ClipOutcome,
    ) {
        Log.i("link", "clip $outcome")
        val event = ClipEvent(System.currentTimeMillis(), text?.take(PREVIEW), outcome)
        _events.update { (listOf(event) + it).take(MAX) }
    }

    fun drop(
        why: ClipDrop,
        detail: String? = null,
    ) = record(null, ClipOutcome.Dropped(why, detail))

    internal fun mode(mode: CaptureMode) {
        if (_mode.value != mode) Log.i("link", "clip capture $mode")
        _mode.value = mode
    }
}
