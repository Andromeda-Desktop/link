package org.andromeda.link.files

import androidx.compose.ui.graphics.ImageBitmap
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.io.File

enum class PickerTab { Photos, Videos, Files }

data class PickerState(
    val open: Boolean = false,
    val tab: PickerTab = PickerTab.Photos,
    val access: MediaAccess = MediaAccess.None,
    val allFiles: Boolean = false,
    val photos: List<PhoneItem> = emptyList(),
    val videos: List<PhoneItem> = emptyList(),
    val folder: File? = null,
    /** [folder]'s names under the storage root; empty at the root. */
    val path: List<String> = emptyList(),
    /** Null when [folder] cannot be read. */
    val entries: List<PhoneItem>? = emptyList(),
    /** In the order picked. */
    val selected: Map<String, PhoneItem> = emptyMap(),
)

/** What the picker sheet does; the screen collects [PickerViewModel.state] and passes only these down. */
interface PickerActions {
    fun refresh()

    fun close()

    fun select(tab: PickerTab)

    fun toggle(item: PhoneItem)

    fun openFolder(folder: File)

    fun up(): Boolean

    suspend fun thumbnail(
        item: PhoneItem,
        px: Int,
    ): ImageBitmap?
}

/** Home's recent-photos row and the picker sheet share one selection, so a pick in either sends with the other. */

class PickerViewModel(
    private val files: PhoneFiles,
) : ViewModel(),
    PickerActions {
    private val _state = MutableStateFlow(PickerState())
    val state: StateFlow<PickerState> = _state.asStateFlow()

    init {
        refresh()
    }

    /** Rereads what Android grants and the lists behind it; called on resume, since grants change in Settings. */
    override fun refresh() {
        viewModelScope.launch {
            val access = files.mediaAccess()
            val allFiles = files.allFiles()
            val photos = if (access != MediaAccess.None) files.media(MediaKind.Photos) else emptyList()
            val videos = if (access != MediaAccess.None) files.media(MediaKind.Videos) else emptyList()
            _state.update { it.copy(access = access, allFiles = allFiles, photos = photos, videos = videos) }
            if (allFiles) openFolder(_state.value.folder ?: files.root)
        }
    }

    fun open(tab: PickerTab) = _state.update { it.copy(open = true, tab = tab) }

    override fun close() = _state.update { it.copy(open = false) }

    override fun select(tab: PickerTab) = _state.update { it.copy(tab = tab) }

    override fun toggle(item: PhoneItem) =
        _state.update { state ->
            val next = if (item.key in state.selected) state.selected - item.key else state.selected + (item.key to item)
            state.copy(selected = next)
        }

    fun clear() = _state.update { it.copy(selected = emptyMap(), open = false) }

    override fun openFolder(folder: File) {
        viewModelScope.launch {
            val entries = files.list(folder)
            val path =
                folder
                    .relativeToOrNull(files.root)
                    ?.invariantSeparatorsPath
                    .orEmpty()
                    .split('/')
                    .filter(String::isNotEmpty)
            _state.update { it.copy(folder = folder, path = path, entries = entries) }
        }
    }

    /** Up one folder; false at the storage root, so Back closes the sheet instead. */
    override fun up(): Boolean {
        val current = _state.value.folder ?: return false
        if (current == files.root) return false
        openFolder(current.parentFile ?: return false)
        return true
    }

    override suspend fun thumbnail(
        item: PhoneItem,
        px: Int,
    ): ImageBitmap? = files.thumbnail(item, px)
}
