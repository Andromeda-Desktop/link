package org.andromeda.link.share

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.Desktop
import org.andromeda.link.core.domain.LinkFailure
import org.andromeda.link.core.domain.ShareDelivery
import org.andromeda.link.core.domain.ShareKind
import org.andromeda.link.core.domain.linkFailure
import org.andromeda.link.core.domain.shareKindOf

sealed interface ShareState {
    data object Loading : ShareState

    data class Choose(
        val desktops: List<Desktop>,
    ) : ShareState

    data class Sending(
        val desktop: Desktop,
    ) : ShareState
}

/** What the share target received: text (sent as a link when it is one), or files behind content URIs. */
sealed interface SharePayload {
    data class Text(
        val text: String,
    ) : SharePayload

    data class Files(
        val uris: List<String>,
    ) : SharePayload
}

/** Queued: the desktop is away, and the phone sends the share when it is back. */
enum class SentAs { Delivered, Queued, Files }

sealed interface ShareOutcome {
    data class Sent(
        val desktop: Desktop,
        val how: SentAs,
    ) : ShareOutcome

    data object NoDesktop : ShareOutcome

    data object Cancelled : ShareOutcome

    data class Failed(
        val failure: LinkFailure,
    ) : ShareOutcome
}

/** Sends one share: to the only paired desktop at once, or to the one the user picks. */
class ShareViewModel(
    private val repository: LinkRepository,
    private val payload: SharePayload,
) : ViewModel() {
    private val _state = MutableStateFlow<ShareState>(ShareState.Loading)
    private val outcomeChannel = Channel<ShareOutcome>(Channel.BUFFERED)

    val state: StateFlow<ShareState> = _state.asStateFlow()
    val outcome: Flow<ShareOutcome> = outcomeChannel.receiveAsFlow()

    init {
        viewModelScope.launch {
            repository.refresh()
            val desktops = repository.desktops.value
            when (desktops.size) {
                0 -> outcomeChannel.send(ShareOutcome.NoDesktop)
                1 -> send(desktops.single())
                else -> _state.value = ShareState.Choose(desktops)
            }
        }
    }

    fun send(desktop: Desktop) {
        _state.value = ShareState.Sending(desktop)
        viewModelScope.launch {
            val result =
                when (payload) {
                    is SharePayload.Text -> {
                        val kind = shareKindOf(payload.text.trim())
                        repository
                            .share(desktop.id, kind, if (kind == ShareKind.Link) payload.text.trim() else payload.text)
                            .map {
                                when (it) {
                                    ShareDelivery.Delivered -> SentAs.Delivered
                                    ShareDelivery.Queued -> SentAs.Queued
                                }
                            }
                    }
                    is SharePayload.Files -> repository.sendFiles(desktop.id, payload.uris).map { SentAs.Files }
                }
            val outcome = result.fold({ ShareOutcome.Sent(desktop, it) }, { ShareOutcome.Failed(it.linkFailure()) })
            outcomeChannel.send(outcome)
        }
    }

    fun cancel() {
        outcomeChannel.trySend(ShareOutcome.Cancelled)
    }
}
