package org.andromeda.link.control

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.andromeda.link.core.domain.LinkFailure
import org.andromeda.link.core.domain.RemoteEvent
import org.andromeda.link.core.domain.RemoteScreens
import org.andromeda.link.core.domain.RemoteSource
import org.andromeda.link.core.domain.linkFailure

data class ViewerState(
    val sources: List<RemoteSource> = emptyList(),
    val current: Int = 0,
    val phase: ViewerPhase = ViewerPhase.Waiting,
    val muted: Boolean = false,
    /** The picture moves the pointer and clicks where it is, as the pad does, instead of clicking where it is touched. */
    val touchpad: Boolean = false,
) {
    /** A window's place on the desktop is unknown: the picture takes no clicks, and the pad moves the desktop's pointer. */
    val window get() = sources.getOrNull(current)?.window == true

    val stream get() = (phase as? ViewerPhase.Streaming)?.stream
}

sealed interface ViewerPhase {
    data object Waiting : ViewerPhase

    data class Streaming(
        val stream: RemoteEvent.Stream,
    ) : ViewerPhase

    /** The desktop ended or refused it (`reason`), or it never opened (`failure`); the viewer only offers to go back. */
    data class Ended(
        val reason: String?,
        val failure: LinkFailure? = null,
    ) : ViewerPhase
}

/**
 * One desktop's screen on this phone: [open] asks for it, [select] switches sources, [leave] ends it. The view model
 * outlives the screen, which survives rotation this way.
 */
class ViewerViewModel(
    private val repository: RemoteScreens,
    private val desktopId: String,
    private val background: CoroutineDispatcher = Dispatchers.IO,
) : ViewModel() {
    private val _state = MutableStateFlow(ViewerState())
    val state: StateFlow<ViewerState> = _state
    private var open = false

    init {
        viewModelScope.launch {
            repository.remote.filter { it.desktopId == desktopId }.collect { event ->
                when (event) {
                    is RemoteEvent.Started -> _state.update { it.copy(sources = event.sources, current = event.current) }
                    is RemoteEvent.Stream -> enter(ViewerPhase.Streaming(event))
                    is RemoteEvent.Stopped -> enter(ViewerPhase.Ended(event.reason))
                }
            }
        }
    }

    fun open() {
        if (open) return
        open = true
        _state.value = ViewerState()
        viewModelScope.launch {
            repository.startRemote(desktopId, null).exceptionOrNull()?.let { error ->
                enter(ViewerPhase.Ended(null, error.linkFailure()))
            }
        }
    }

    fun leave() {
        if (!open) return
        open = false
        val stream = _state.value.stream
        _state.value = ViewerState()
        // Not the view model's scope: the stop must reach the desktop after the screen is gone.
        CoroutineScope(SupervisorJob() + background).launch {
            stream?.video?.stop()
            repository.stopRemote(desktopId)
        }
    }

    fun select(source: Int) {
        if (source == _state.value.current) return
        viewModelScope.launch { repository.startRemote(desktopId, source) }
    }

    fun toggleMute() = _state.update { it.copy(muted = !it.muted) }

    fun setTouchpad(on: Boolean) = _state.update { it.copy(touchpad = on) }

    /** The one place the phase changes. A stream that arrives after [leave] is stopped, never shown. */
    private suspend fun enter(next: ViewerPhase) {
        if (!open) {
            (next as? ViewerPhase.Streaming)?.stream?.video?.stop()
            return
        }
        _state.value.stream
            ?.video
            ?.stop()
        _state.update { it.copy(phase = next) }
    }

    override fun onCleared() = leave()
}
