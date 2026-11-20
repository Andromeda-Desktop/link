package org.andromeda.link.control

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.Desktop
import org.andromeda.link.core.domain.LinkFailure
import org.andromeda.link.core.domain.RemoteInput
import org.andromeda.link.core.domain.RemoteKey
import org.andromeda.link.core.domain.linkFailure
import kotlin.math.roundToInt

data class RemoteDesk(
    val name: String = "",
    val connected: Boolean = false,
    /** The desktop takes this phone's pointer and keys. */
    val input: Boolean = false,
)

/** The input a remote surface sends; screens collect the state and pass only these events down. */
interface RemoteActions {
    fun move(
        dx: Float,
        dy: Float,
    )

    fun scroll(
        dx: Float,
        dy: Float,
    )

    fun tick()

    fun send(input: RemoteInput)

    fun typed(
        before: String,
        after: String,
    )

    fun press(key: RemoteKey)

    fun hold(modifier: String)

    fun shortcut(key: String)
}

/**
 * The phone as one desktop's touchpad and keyboard. Moves and scrolls add up and go once a frame, so a fast finger
 * stays under the desktop's input rate; everything goes out in the order it happened.
 */

class RemoteViewModel(
    private val repository: LinkRepository,
    private val desktopId: String,
) : ViewModel(),
    RemoteActions {
    private val outbox = Channel<RemoteInput>(Channel.UNLIMITED)
    private val wake = Channel<Unit>(Channel.CONFLATED)
    private var moveX = 0f
    private var moveY = 0f
    private var scrollX = 0f
    private var scrollY = 0f
    private var point: RemoteInput.Point? = null
    private val _failure = MutableStateFlow<LinkFailure?>(null)
    val failure: StateFlow<LinkFailure?> = _failure
    private val _held = MutableStateFlow<Set<String>>(emptySet())

    /** The modifiers held for the next key, as `super` or `ctrl`. */
    val held: StateFlow<Set<String>> = _held

    /** The desktop takes shortcuts: its input can hold modifiers. */
    val shortcuts: StateFlow<Boolean> =
        repository.capabilities
            .map { SHORTCUTS in it[desktopId].orEmpty() }
            .stateIn(viewModelScope, SharingStarted.Eagerly, false)

    /** The desktop makes a real touchpad of raw finger contacts, so its own touchpad settings and gestures apply. */
    val touchpad: StateFlow<Boolean> =
        repository.capabilities
            .map { TOUCHPAD in it[desktopId].orEmpty() }
            .stateIn(viewModelScope, SharingStarted.Eagerly, false)

    /** The desktop as the Remote's chip shows it. */
    val desk: StateFlow<RemoteDesk> =
        combine(repository.desktops, repository.capabilities, ::remoteDesk)
            .stateIn(viewModelScope, SharingStarted.Eagerly, remoteDesk(repository.desktops.value, repository.capabilities.value))

    private fun remoteDesk(
        desktops: List<Desktop>,
        caps: Map<String, Set<String>>,
    ): RemoteDesk {
        val desktop = desktops.firstOrNull { it.id == desktopId }
        return RemoteDesk(
            name = desktop?.name.orEmpty(),
            connected = desktop?.connected == true,
            input = desktop != null && desktop.control && (caps[desktopId]?.contains(INPUT) ?: true),
        )
    }

    init {
        viewModelScope.launch {
            for (input in outbox) {
                repository.sendInput(desktopId, input).exceptionOrNull()?.let { _failure.value = it.linkFailure() }
            }
        }
        viewModelScope.launch {
            // One send per touch frame: the display rate, with the frame's samples summed.
            for (ignored in wake) flush()
        }
    }

    /** By (dx, dy) of the phone's pixels. */
    override fun move(
        dx: Float,
        dy: Float,
    ) {
        moveX += dx * POINTER_SPEED
        moveY += dy * POINTER_SPEED
        wake.trySend(Unit)
    }

    /** The pointer to a place on remote control's picture; only the latest of a frame goes. */
    fun point(at: RemoteInput.Point) {
        point = at
        wake.trySend(Unit)
    }

    /** Fingers moved by (dx, dy) of the phone's pixels; the content follows them. */
    override fun scroll(
        dx: Float,
        dy: Float,
    ) {
        scrollX -= dx
        scrollY -= dy
        wake.trySend(Unit)
    }

    private val _ticks = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    /** A click, a key from the shortcut bar, or a gesture start, for the screen to make felt. */
    val ticks: SharedFlow<Unit> = _ticks

    override fun tick() {
        _ticks.tryEmit(Unit)
    }

    override fun send(input: RemoteInput) {
        if (input is RemoteInput.Click || input is RemoteInput.Press) tick()
        flush()
        outbox.trySend(input)
    }

    fun key(key: RemoteKey) = send(RemoteInput.Key(key))

    /** What changed in the typing field: removed characters as Backspace, then the added text. */
    override fun typed(
        before: String,
        after: String,
    ) {
        val kept = before.commonPrefixWith(after).length
        repeat(before.length - kept) { key(RemoteKey.Backspace) }
        val added = after.substring(kept)
        when {
            added.isEmpty() -> Unit
            _held.value.isNotEmpty() && added.length == 1 -> shortcut(added.lowercase())
            else -> send(RemoteInput.Text(added))
        }
    }

    /** [key], with the held modifiers when there are any. */
    override fun press(key: RemoteKey) {
        tick()
        if (_held.value.isEmpty()) {
            key(key)
        } else {
            shortcut(key.name.replace(Regex("(?<=[a-z])(?=[A-Z])"), "-").lowercase())
        }
    }

    /** The first tap holds [modifier] for the next key; a second tap, with no key between, presses it alone. */
    override fun hold(modifier: String) {
        if (modifier in _held.value) {
            _held.update { it - modifier }
            shortcut(modifier)
        } else {
            tick()
            _held.update { it + modifier }
        }
    }

    /** [key] with the held modifiers, which let go after it; [key] may carry its own, as `alt+tab`. */
    override fun shortcut(key: String) {
        tick()
        val held = MODIFIERS.filter { it in _held.value }
        send(RemoteInput.Shortcut((held + key).joinToString("+")))
        _held.value = emptySet()
    }

    /** The presenter opened or closed: the desktop stays awake meanwhile. Runs even as the view model clears. */
    fun presenting(on: Boolean) {
        viewModelScope.launch(NonCancellable) { repository.setPresenting(desktopId, on) }
    }

    fun failureShown() {
        _failure.value = null
    }

    private fun flush() {
        point?.let { outbox.trySend(it) }
        point = null
        val (mx, my) = moveX.roundToInt() to moveY.roundToInt()
        if (mx != 0 || my != 0) {
            outbox.trySend(RemoteInput.Move(mx, my))
            moveX -= mx
            moveY -= my
        }
        val (sx, sy) = scrollX.roundToInt() to scrollY.roundToInt()
        if (sx != 0 || sy != 0) {
            outbox.trySend(RemoteInput.Scroll(sx, sy))
            scrollX -= sx
            scrollY -= sy
        }
    }

    private companion object {
        const val SHORTCUTS = "shortcuts"
        const val TOUCHPAD = "touchpad"
        const val INPUT = "input"

        /** In the order a shortcut names them. */
        val MODIFIERS = listOf("super", "ctrl", "alt", "shift")
        const val POINTER_SPEED = 1.5f
    }
}
