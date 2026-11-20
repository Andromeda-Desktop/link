package org.andromeda.link.files

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import org.andromeda.link.R
import org.andromeda.link.media.clock
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.Segmented
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
import org.andromeda.link.ui.theme.spring

/**
 * The in-app picker over one selection: Photos, Videos and Files in a view sheet (r28, inset 8, e4) that hugs its
 * content up to [Tok.Phone.pickMaxTop] under the status bar, and one send capsule that is off until something is picked.
 */
@Composable
fun BoxScope.PickerSheet(
    viewModel: PickerActions,
    state: PickerState,
    desktopName: String,
    onSend: () -> Unit,
    modifier: Modifier = Modifier,
) {
    BackHandler(state.open) { if (state.tab != PickerTab.Files || !viewModel.up()) viewModel.close() }
    val s = LinkTheme.scheme
    AnimatedVisibility(state.open, enter = fadeIn(spring(Tok.Spring.fade)), exit = fadeOut(spring(Tok.Spring.fadeOut))) {
        Box(
            Modifier
                .fillMaxSize()
                .background(s.scrim)
                .clickable(remember { MutableInteractionSource() }, indication = null, onClick = viewModel::close),
        )
    }
    AnimatedVisibility(
        state.open,
        modifier
            .align(Alignment.BottomCenter)
            .statusBarsPadding()
            .padding(top = Tok.Phone.pickMaxTop.dp),
        enter = slideInVertically(spring<IntOffset>(Tok.Spring.open)) { it } + fadeIn(spring(Tok.Spring.fade)),
        exit = slideOutVertically(spring<IntOffset>(Tok.Spring.close)) { it } + fadeOut(spring(Tok.Spring.fadeOut)),
    ) {
        val shape = RoundedCornerShape(Tok.Radius.r28.dp)
        Column(
            Modifier
                .navigationBarsPadding()
                .padding(Tok.Space.p8.dp)
                .fillMaxWidth()
                .elevation(Tok.Elevation.e4, Tok.Radius.r28.dp)
                .clip(shape)
                .background(s.view)
                .clickable(remember { MutableInteractionSource() }, indication = null) {}
                .padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, top = Tok.Space.p8.dp, bottom = Tok.Space.p16.dp),
            verticalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
        ) {
            Box(
                Modifier
                    .align(Alignment.CenterHorizontally)
                    .size(Tok.Phone.grab.dp, Tok.Space.p4.dp)
                    .clip(CircleShape)
                    .background(s.fill3),
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(R.string.picker_title, desktopName),
                    LinkTheme.phoneType.title3,
                    Modifier.weight(1f),
                    maxLines = 1,
                )
                CloseKey(viewModel::close)
            }
            Segmented(
                listOf(
                    PickerTab.Photos to stringResource(R.string.picker_photos),
                    PickerTab.Videos to stringResource(R.string.picker_videos),
                    PickerTab.Files to stringResource(R.string.picker_files),
                ),
                state.tab,
                viewModel::select,
            )
            Box(Modifier.weight(1f, fill = false).fillMaxWidth()) {
                when (state.tab) {
                    PickerTab.Photos -> MediaGrid(viewModel, state, state.photos, Icons.Photo)
                    PickerTab.Videos -> MediaGrid(viewModel, state, state.videos, Icons.Video)
                    PickerTab.Files -> FileList(viewModel, state)
                }
            }
            Capsule(
                if (state.selected.isEmpty()) stringResource(R.string.picker_send_none) else sendLabel(state.selected),
                onSend,
                Modifier.fillMaxWidth(),
                icon = Icons.Up,
                kind = CapsuleKind.Primary,
                large = true,
                enabled = state.selected.isNotEmpty(),
            )
        }
    }
}

@Composable
private fun MediaGrid(
    viewModel: PickerActions,
    state: PickerState,
    items: List<PhoneItem>,
    icon: ImageVector,
) {
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { viewModel.refresh() }
    if (state.access == MediaAccess.None) {
        Notice(
            icon,
            stringResource(R.string.picker_media_title),
            stringResource(R.string.picker_media_hint),
            stringResource(R.string.picker_media_allow),
        ) { ask.launch(PhoneFiles.mediaPermissions) }
        return
    }
    Column {
        if (state.access == MediaAccess.Partial) {
            Row(
                Modifier.padding(bottom = Tok.Space.p8.dp),
                horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    stringResource(R.string.picker_partial),
                    LinkTheme.phoneType.footnote,
                    Modifier.weight(1f),
                    color = LinkTheme.scheme.label2,
                )
                Pill(stringResource(R.string.picker_partial_more), { ask.launch(PhoneFiles.mediaPermissions) })
            }
        }
        if (items.isEmpty()) {
            Notice(icon, stringResource(R.string.picker_empty))
            return
        }
        LazyVerticalGrid(
            GridCells.Fixed(GRID_COLUMNS),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp),
            verticalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp),
        ) {
            items(items, key = { it.key }) { item ->
                MediaTile(item, state.selected.keys.indexOf(item.key), viewModel::thumbnail) { viewModel.toggle(item) }
            }
        }
    }
}

@Composable
private fun FileList(
    viewModel: PickerActions,
    state: PickerState,
) {
    val context = LocalContext.current
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { viewModel.refresh() }
    if (!state.allFiles) {
        Notice(
            Icons.Folder,
            stringResource(R.string.picker_files_title),
            stringResource(R.string.picker_files_hint),
            stringResource(R.string.picker_files_allow),
        ) {
            if (Build.VERSION.SDK_INT >= 30) {
                context.startActivity(
                    Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${context.packageName}")),
                )
            } else {
                ask.launch(PhoneFiles.mediaPermissions)
            }
        }
        return
    }
    val s = LinkTheme.scheme
    Column {
        Row(Modifier.padding(end = Tok.Space.p4.dp), verticalAlignment = Alignment.CenterVertically) {
            BackKey(s.label2, { viewModel.up() })
            val path = (listOf(stringResource(R.string.picker_storage)) + state.path).joinToString(" › ")
            Text(path, LinkTheme.phoneType.footnote, Modifier.weight(1f), color = s.label2, maxLines = 1)
        }
        val entries = state.entries
        if (entries.isNullOrEmpty()) {
            Notice(Icons.Folder, stringResource(if (entries == null) R.string.picker_folder_unreadable else R.string.picker_folder_empty))
            return
        }
        LazyColumn {
            items(entries, key = { it.key }) { item ->
                val order = state.selected.keys.indexOf(item.key)
                Row(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = Tok.Size.rowTwo.dp)
                        .clickable { item.folder?.let(viewModel::openFolder) ?: viewModel.toggle(item) }
                        .semantics { if (item.folder == null) selected = order >= 0 }
                        .padding(vertical = Tok.Space.p8.dp),
                    horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    FileIcon(item, viewModel::thumbnail)
                    Column(Modifier.weight(1f)) {
                        Text(item.name, LinkTheme.phoneType.body)
                        if (item.folder == null) {
                            val size = Formatter.formatShortFileSize(context, item.size)
                            val date = DateUtils.formatDateTime(context, item.modified * 1_000, DATE_FLAGS)
                            Text("$size · $date", LinkTheme.phoneType.footnote, color = s.label2)
                        }
                    }
                    if (item.folder == null) {
                        OrderBadge(order, onImage = false)
                    } else {
                        Symbol(Icons.Chev, s.label3, size = Tok.Size.symbolL.dp)
                    }
                }
            }
        }
    }
}

@Composable
private fun FileIcon(
    item: PhoneItem,
    thumbnail: suspend (PhoneItem, Int) -> ImageBitmap?,
) {
    val s = LinkTheme.scheme
    val box =
        Modifier
            .size(Tok.Size.controlL.dp)
            .clip(RoundedCornerShape(Tok.Radius.r12.dp))
            .background(s.fill1)
    if (item.visual) {
        Thumbnail(item, Tok.Size.controlL.dp, thumbnail, box)
    } else {
        Box(box, contentAlignment = Alignment.Center) {
            Symbol(if (item.folder != null) Icons.Folder else Icons.File, s.label2)
        }
    }
}

/** A square photo or video: an inset accent ring and its pick order when picked, the length on a video. */
@Composable
private fun MediaTile(
    item: PhoneItem,
    order: Int,
    thumbnail: suspend (PhoneItem, Int) -> ImageBitmap?,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    val shape = RoundedCornerShape(Tok.Radius.r8.dp)
    Box(
        Modifier
            .fillMaxWidth()
            .aspectRatio(1f)
            .clip(shape)
            .background(s.fill2)
            .clickable(onClick = onClick)
            .semantics { selected = order >= 0 },
    ) {
        Thumbnail(item, THUMB_DP, thumbnail, Modifier.fillMaxSize())
        if (order >= 0) Box(Modifier.matchParentSize().border(Tok.Phone.pickRing.dp, s.accent, shape))
        if (item.durationMs > 0) {
            val lift =
                with(LocalDensity.current) {
                    Tok.Space.p2.dp
                        .toPx()
                }
            Text(
                clock(item.durationMs),
                LinkTheme.phoneType.caption.copy(fontWeight = FontWeight.SemiBold, shadow = Shadow(s.scrim, Offset(0f, lift), lift)),
                Modifier.align(Alignment.BottomStart).padding(start = Tok.Space.p8.dp, bottom = Tok.Space.p4.dp),
                color = s.onAccent,
            )
        }
        OrderBadge(order, onImage = true, Modifier.align(Alignment.TopEnd).padding(Tok.Space.p8.dp))
    }
}

/** The pick-order badge: the order on accent once picked, an empty ring before (light on a photo, label_3 in a list). */
@Composable
private fun OrderBadge(
    order: Int,
    onImage: Boolean,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val base = modifier.size(Tok.Phone.pickBadge.dp).clip(CircleShape)
    if (order >= 0) {
        Box(base.background(s.accent), contentAlignment = Alignment.Center) {
            Text((order + 1).toString(), LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold), color = s.onAccent)
        }
    } else {
        val ring = if (onImage) s.onAccent else s.label3
        Box(
            (if (onImage) base.background(s.scrim) else base)
                .border(Tok.Phone.pickBadgeRing.dp, ring, CircleShape),
        )
    }
}

@Composable
private fun Thumbnail(
    item: PhoneItem,
    size: Dp,
    thumbnail: suspend (PhoneItem, Int) -> ImageBitmap?,
    modifier: Modifier = Modifier,
) {
    val px = with(LocalDensity.current) { size.roundToPx() }
    val load by rememberUpdatedState(thumbnail)
    val bitmap by produceState<ImageBitmap?>(null, item.key, px) { value = load(item, px) }
    Box(modifier) {
        bitmap?.let { Image(it, contentDescription = item.name, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize()) }
    }
}

/** "Send 3 items · 12 MB". */
@Composable
fun sendLabel(selected: Map<String, PhoneItem>): String {
    val context = LocalContext.current
    val bytes = Formatter.formatShortFileSize(context, selected.values.sumOf { it.size })
    return LocalResources.current.getQuantityString(R.plurals.picker_send, selected.size, selected.size, bytes)
}

/** An empty or locked tab: a symbol, a headline, and optionally a hint and one action. */
@Composable
private fun Notice(
    icon: ImageVector,
    title: String,
    hint: String? = null,
    action: String? = null,
    onAction: () -> Unit = {},
) {
    val s = LinkTheme.scheme
    Column(
        Modifier.fillMaxWidth().padding(horizontal = Tok.Space.p16.dp, vertical = Tok.Space.p48.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
    ) {
        Symbol(icon, s.label2, size = Tok.Phone.heroSymbol.dp)
        Text(title, LinkTheme.phoneType.headline, align = TextAlign.Center)
        if (hint != null) Text(hint, LinkTheme.phoneType.footnote, color = s.label2, align = TextAlign.Center)
        if (action != null) Capsule(action, onAction, Modifier.padding(top = Tok.Space.p8.dp), kind = CapsuleKind.Primary)
    }
}

@Composable
private fun CloseKey(onClose: () -> Unit) {
    val close = stringResource(R.string.close)
    Box(
        Modifier
            .size(Tok.Phone.hit.dp)
            .clip(CircleShape)
            .clickable(role = Role.Button, onClick = onClose)
            .semantics { contentDescription = close },
        contentAlignment = Alignment.Center,
    ) { Symbol(Icons.X, LinkTheme.scheme.label2) }
}

private const val GRID_COLUMNS = 3
private const val DATE_FLAGS = DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_ABBREV_MONTH
private val THUMB_DP = 128.dp
