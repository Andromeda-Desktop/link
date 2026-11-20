package org.andromeda.link.devices

import android.Manifest
import android.content.ClipboardManager
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.os.Build
import android.os.SystemClock
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.andromeda.link.AppContainer
import org.andromeda.link.R
import org.andromeda.link.core.domain.Desktop
import org.andromeda.link.core.domain.DesktopLink
import org.andromeda.link.core.domain.DesktopPlayer
import org.andromeda.link.core.domain.DesktopStatus
import org.andromeda.link.core.domain.DesktopVolume
import org.andromeda.link.core.domain.Feature
import org.andromeda.link.core.domain.MediaCommandKind
import org.andromeda.link.core.domain.PlaybackState
import org.andromeda.link.core.domain.TransferRecord
import org.andromeda.link.files.PickerActions
import org.andromeda.link.files.PickerSheet
import org.andromeda.link.files.PickerTab
import org.andromeda.link.files.PickerViewModel
import org.andromeda.link.onboarding.Grant
import org.andromeda.link.ui.components.PullRefresh
import org.andromeda.link.ui.components.Screen
import org.andromeda.link.ui.kit.BottomFade
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.CapsuleSlider
import org.andromeda.link.ui.kit.DragTrack
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.OfferRow
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.PillKind
import org.andromeda.link.ui.kit.RoundKey
import org.andromeda.link.ui.kit.SectionTitle
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.Sheet
import org.andromeda.link.ui.kit.SwitchRow
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation

/**
 * Home is the one root (handoff `screens.home`, concept-c.html): the desktop's identity row, its state card or media
 * card, the controls, and the floating dock with Lock, Touchpad, Presenter and Send.
 */
@Composable
fun HomeScreen(
    container: AppContainer,
    /** Required grants not given yet, in setup order. */
    missing: List<Grant>,
    /** Both grants automatic clipboard needs are given. */
    autoClipboard: Boolean,
    onPair: () -> Unit,
    onDesktop: (Desktop) -> Unit,
    onMedia: () -> Unit,
    /** Opens setup at one grant, from the fixes sheet. */
    onFix: (Grant) -> Unit,
    onClipboardSetup: () -> Unit,
    onActivity: () -> Unit,
    onSettings: () -> Unit,
    onRemote: (Desktop) -> Unit,
    onPresenter: (Desktop) -> Unit,
    onViewer: (Desktop) -> Unit,
    modifier: Modifier = Modifier,
    viewModel: HomeViewModel = viewModel { HomeViewModel(container.repository, container.presence, container.mirror, container.clipboard) },
    picker: PickerViewModel = viewModel { PickerViewModel(container.files) },
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val message by viewModel.message.collectAsStateWithLifecycle()
    val pick = picker.state.collectAsStateWithLifecycle()
    val pickerActions: PickerActions = picker
    val controls: DeskControls = viewModel
    // Only open and close recompose Home; the sheet reads the rest itself.
    val pickOpen by remember { derivedStateOf { pick.value.open } }
    val context = LocalContext.current
    val resources = context.resources
    val notificationPermission =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) viewModel.notificationsAllowed()
        }
    val paired = state.desktops.isNotEmpty()
    // Asked once a desktop is paired, the first moment one can send this phone anything to notify about.
    LaunchedEffect(paired) {
        val needed =
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
                PackageManager.PERMISSION_GRANTED
        if (paired && needed) notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
    LifecycleResumeEffect(Unit) {
        picker.refresh()
        onPauseOrDispose {}
    }
    val toast =
        when (val shown = message) {
            is HomeMessage.Paired -> resources.getString(R.string.paired_with, shown.name)
            is HomeMessage.Sent -> resources.getString(R.string.share_sent, shown.name)
            is HomeMessage.FilesSending -> resources.getString(R.string.share_files_sending, shown.name)
            HomeMessage.ClipboardEmpty -> resources.getString(R.string.clipboard_empty)
            is HomeMessage.ClipboardSent -> resources.getString(R.string.clipboard_sent, shown.name)
            is HomeMessage.ClipboardQueued -> resources.getString(R.string.clipboard_queued, shown.name)
            is HomeMessage.Failed -> shown.failure.text(resources)
            null -> null
        }
    val primary = state.primary
    val send = {
        val selected = pick.value.selected
        primary?.let { viewModel.sendFiles(it, selected.keys.toList()) }
        picker.clear()
    }
    val sendText = {
        primary?.let { viewModel.sendClipboard(it, context.getSystemService(ClipboardManager::class.java).primaryClip) }
        Unit
    }
    var fan by rememberSaveable { mutableStateOf(false) }
    var sheet by rememberSaveable { mutableStateOf(HomeSheet.None) }
    Box(modifier) {
        Screen(
            title = null,
            toast = toast,
            onToastShown = viewModel::messageShown,
            toastInset = (Tok.Phone.dockInset + Tok.Phone.hit + Tok.Space.p12).dp,
            overlay = {
                if (primary != null) {
                    // Under the open picker, Home's controls would duplicate the sheet's for TalkBack.
                    val hidden = if (pickOpen) Modifier.clearAndSetSemantics {} else Modifier
                    BottomFade(Tok.Phone.fadeDock.dp, Modifier.align(Alignment.BottomCenter))
                    Dock(
                        primary,
                        fan,
                        { fan = it },
                        onLock = { viewModel.lock(primary) },
                        // Touchpad and Presenter need input on the desktop; without it they open the Device page to allow it.
                        onRemote = { if (input(state, primary)) onRemote(primary) else onDesktop(primary) },
                        onPresenter = { if (input(state, primary)) onPresenter(primary) else onDesktop(primary) },
                        onPhotos = { picker.open(PickerTab.Photos) },
                        onFiles = { picker.open(PickerTab.Files) },
                        onText = sendText,
                        modifier = Modifier.align(Alignment.BottomCenter).then(hidden),
                    )
                    PickerSheet(pickerActions, pick.value, primary.name, send)
                    Sheet(
                        sheet == HomeSheet.Fixes,
                        { sheet = HomeSheet.None },
                        pluralStringResource(R.plurals.home_fixes_title, missing.size, missing.size),
                        subtitle = stringResource(R.string.home_fixes_body),
                    ) {
                        missing.forEachIndexed { index, grant ->
                            if (index > 0) Separator()
                            Row(
                                Modifier.fillMaxWidth().padding(vertical = Tok.Space.p4.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(stringResource(grant.title()), LinkTheme.phoneType.headline, modifier = Modifier.weight(1f))
                                Pill(stringResource(R.string.missing_turn_on), {
                                    sheet = HomeSheet.None
                                    onFix(grant)
                                })
                            }
                        }
                    }
                    Sheet(sheet == HomeSheet.Desks, { sheet = HomeSheet.None }, stringResource(R.string.desktops_title)) {
                        state.desktops.forEach { desktop ->
                            DeskRow(desktop, desktop.id == primary.id) {
                                sheet = HomeSheet.None
                                viewModel.selectDesktop(desktop)
                            }
                        }
                        DeskRow(null, false) {
                            sheet = HomeSheet.None
                            onPair()
                        }
                    }
                }
            },
        ) {
            if (primary == null) {
                Empty(onPair)
                return@Screen
            }
            val desk = state.desk
            val status = desk.statuses[primary.id]
            val live = primary.connected
            val player = desk.player(primary.id)?.takeIf { live }
            val hidden = if (pickOpen) Modifier.clearAndSetSemantics {} else Modifier
            PullRefresh(state.refreshing, viewModel::refresh, Modifier.fillMaxSize().then(hidden)) {
                LazyColumn(
                    Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Phone.fadeDock.dp),
                ) {
                    item {
                        IdentityRow(
                            primary,
                            state.refreshing,
                            missing.size,
                            onOpen = { if (state.desktops.size > 1) sheet = HomeSheet.Desks else onDesktop(primary) },
                            onFixes = { sheet = HomeSheet.Fixes },
                            onActivity = onActivity,
                            onSettings = onSettings,
                        )
                    }
                    if (primary.link == DesktopLink.Offline) {
                        item { StatusCard(primary, state.refreshing) { viewModel.connect(primary) } }
                    } else if (player != null) {
                        item {
                            Column {
                                MediaCard(player, onOpen = onMedia) { command, value -> viewModel.media(player, command, value) }
                                Levels(primary, desk.can(primary.id), status, desk.volumes[primary.id], controls, volumeEnabled = true)
                            }
                        }
                    }
                    val offers = desk.transfers.filter { it.incoming == true && it.status in OFFER_STATUSES }
                    if (live && offers.isNotEmpty()) {
                        item {
                            Offers(
                                offers,
                                onSave = viewModel::acceptTransfer,
                                onDecline = viewModel::declineTransfer,
                                onCancel = viewModel::cancelTransfer,
                            )
                        }
                    }
                    item {
                        Controls(
                            primary,
                            desk.can(primary.id),
                            status,
                            desk.volumes[primary.id],
                            levels = player == null,
                            primary.id in state.ringing,
                            autoClipboard,
                            controls,
                            onViewer = { onViewer(primary) },
                            onClipboardSetup = onClipboardSetup,
                        )
                    }
                }
            }
        }
    }
}

/** Which sheet covers Home; one at a time. */
private enum class HomeSheet { None, Fixes, Desks }

/** One desktop in the Desktops sheet (current = accent tint + check), or the Pair row when [desktop] is null. */
@Composable
private fun DeskRow(
    desktop: Desktop?,
    current: Boolean,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = Tok.Phone.identity.dp)
            .clip(RoundedCornerShape(Tok.Radius.r16.dp))
            .then(if (current) Modifier.background(s.accent.copy(alpha = Tok.Phone.chipTint)) else Modifier)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = if (current) Tok.Space.p12.dp else Tok.Space.p4.dp, vertical = Tok.Space.p8.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .size(Tok.Phone.deskGlyph.dp)
                .clip(RoundedCornerShape(Tok.Radius.r12.dp))
                .background(if (current) s.accent.copy(alpha = Tok.Phone.glyphTint) else s.fill1),
            contentAlignment = Alignment.Center,
        ) {
            Symbol(if (desktop == null) Icons.Plus else Icons.Laptop, if (current || desktop == null) s.accentText else s.label2)
        }
        if (desktop == null) {
            Text(stringResource(R.string.home_pair_desktop), t.headline, color = s.accentText, modifier = Modifier.weight(1f), maxLines = 1)
            return@Row
        }
        Column(Modifier.weight(1f)) {
            Text(desktop.name, t.headline, maxLines = 2)
            Presence(desktop, refreshing = false, Modifier)
        }
        if (current) Symbol(Icons.Check, s.accentText)
    }
}

/** The desktop: glyph, name (2 lines at most), presence and the fix chip; Activity and Settings keys at the end. */
@Composable
private fun IdentityRow(
    desktop: Desktop,
    refreshing: Boolean,
    fixes: Int,
    onOpen: () -> Unit,
    onFixes: () -> Unit,
    onActivity: () -> Unit,
    onSettings: () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val live = desktop.connected
    Row(Modifier.fillMaxWidth().padding(top = Tok.Space.p8.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(
            Modifier
                .weight(1f)
                .heightIn(min = Tok.Phone.identity.dp)
                .clip(RoundedCornerShape(Tok.Radius.r16.dp))
                .clickable(role = Role.Button, onClickLabel = stringResource(R.string.desktop_open), onClick = onOpen),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier
                    .size(Tok.Phone.glyph.dp)
                    .clip(RoundedCornerShape(Tok.Radius.r16.dp))
                    .background(if (live) s.accent.copy(alpha = Tok.Phone.glyphTint) else s.fill1),
                contentAlignment = Alignment.Center,
            ) { Symbol(Icons.Laptop, if (live) s.accentText else s.label2, size = Tok.Size.controlS.dp + Tok.Space.p2.dp) }
            Column(Modifier.weight(1f)) {
                Text(desktop.name, t.title1, maxLines = 2)
                Row(
                    horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp + Tok.Space.p2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Presence(desktop, refreshing, Modifier.weight(1f, fill = false))
                    if (fixes > 0) FixChip(fixes, onFixes)
                }
            }
        }
        IconKey(Icons.Clock, stringResource(R.string.activity_title), onActivity)
        IconKey(Icons.Gear, stringResource(R.string.settings_title), onSettings)
    }
}

@Composable
private fun Presence(
    desktop: Desktop,
    refreshing: Boolean,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val dot = Tok.Space.p8.dp
    val (mark, text) =
        when {
            desktop.connected ->
                Modifier.background(s.success, CircleShape) to
                    stringResource(
                        R.string.home_connected_via,
                        stringResource(if (desktop.bluetooth) R.string.home_via_bluetooth else R.string.home_via_wifi),
                    )
            desktop.link == DesktopLink.Reconnecting ->
                Modifier.border(Tok.Space.p2.dp, s.warning, CircleShape) to stringResource(R.string.home_reconnecting)
            refreshing -> Modifier.border(Tok.Space.p2.dp, s.warning, CircleShape) to stringResource(R.string.home_connecting)
            desktop.paused -> null to stringResource(R.string.home_paused_short, desktop.name)
            else -> Modifier.background(s.label3, CircleShape) to stringResource(R.string.home_last_seen, lastSeen(desktop.lastSeen))
        }
    Row(
        modifier,
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp + Tok.Space.p2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (mark != null) Box(Modifier.size(dot).then(mark)) else Symbol(Icons.Pause, s.label2, size = Tok.Size.symbol.dp - Tok.Space.p2.dp)
        Text(text, LinkTheme.phoneType.footnote, color = s.label2, maxLines = 1)
    }
}

@Composable
private fun FixChip(
    count: Int,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    val label = pluralStringResource(R.plurals.home_fixes, count, count)
    Row(
        Modifier
            .height(Tok.Phone.fixChip.dp)
            .clip(CircleShape)
            .background(s.warning.copy(alpha = Tok.Phone.glyphTint))
            .clickable(role = Role.Button, onClickLabel = label, onClick = onClick)
            .padding(horizontal = Tok.Space.p8.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Symbol(Icons.Shield, s.warning, size = Tok.Size.symbol.dp - Tok.Space.p2.dp)
        Text("$count", LinkTheme.phoneType.caption.copy(fontWeight = FontWeight.SemiBold), maxLines = 1)
    }
}

@Composable
private fun IconKey(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
) {
    Box(
        Modifier
            .size(Tok.Phone.hit.dp)
            .clip(CircleShape)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Symbol(icon, LinkTheme.scheme.label1) }
}

/** Away or paused: what happened and, when away, Connect now. */
@Composable
private fun StatusCard(
    desktop: Desktop,
    refreshing: Boolean,
    onConnect: () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val shape = RoundedCornerShape(Tok.Radius.r16.dp)
    Column(
        Modifier
            .padding(top = Tok.Space.p12.dp)
            .fillMaxWidth()
            .clip(shape)
            .background(s.view)
            .border(Dp.Hairline, s.stroke, shape)
            .padding(Tok.Space.p16.dp),
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp),
    ) {
        if (desktop.paused) {
            Text(stringResource(R.string.home_paused_title, desktop.name), t.headline)
            Text(stringResource(R.string.home_paused_body), t.footnote, color = s.label2)
        } else {
            Text(stringResource(R.string.home_away_title, desktop.name), t.headline)
            Text(stringResource(R.string.home_away_body), t.footnote, color = s.label2)
            Capsule(
                stringResource(R.string.home_connect_now),
                onConnect,
                Modifier.padding(top = Tok.Space.p12.dp).alpha(if (refreshing) Tok.Phone.offAlpha else 1f),
                kind = CapsuleKind.Primary,
            )
        }
    }
}

/** Only while a player plays: art with title and artist, the seek line with times, then previous / play / next. */
@Composable
private fun MediaCard(
    shown: DesktopPlayer,
    onOpen: () -> Unit,
    onCommand: (MediaCommandKind, Long?) -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val player = shown.player
    val playing = player.state == PlaybackState.Playing
    val shape = RoundedCornerShape(Tok.Radius.r16.dp)
    Column(
        Modifier
            .padding(top = Tok.Space.p12.dp)
            .fillMaxWidth()
            .clip(shape)
            .background(s.view)
            .border(Dp.Hairline, s.stroke, shape)
            .padding(Tok.Space.p12.dp),
    ) {
        Row(
            Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onOpen),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Artwork(player.artwork)
            Column(Modifier.weight(1f)) {
                Text(player.title.ifEmpty { player.name }, t.headline, maxLines = 1)
                val by = if (player.artist.isEmpty()) player.name else stringResource(R.string.home_on_app, player.artist, player.name)
                Text(by, t.footnote, color = s.label2, maxLines = 1)
            }
        }
        Progress(player.positionMs, player.lengthMs, player.receivedAt, playing, MediaCommandKind.Seek in player.can) {
            onCommand(MediaCommandKind.Seek, it)
        }
        Row(
            Modifier.fillMaxWidth().padding(top = Tok.Space.p4.dp),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p24.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            MediaKey(Icons.Prev, stringResource(R.string.media_previous), MediaCommandKind.Previous in player.can) {
                onCommand(MediaCommandKind.Previous, null)
            }
            val playLabel = stringResource(if (playing) R.string.home_pause else R.string.home_play)
            Box(
                Modifier
                    .size(Tok.Phone.roundKeyL.dp)
                    .clip(CircleShape)
                    .background(s.label1)
                    .clickable(enabled = MediaCommandKind.PlayPause in player.can, role = Role.Button) {
                        onCommand(MediaCommandKind.PlayPause, null)
                    }.semantics { contentDescription = playLabel },
                contentAlignment = Alignment.Center,
            ) { Symbol(if (playing) Icons.Pause else Icons.Play, s.view, size = Tok.Phone.symbol.dp) }
            MediaKey(Icons.Next, stringResource(R.string.media_next), MediaCommandKind.Next in player.can) {
                onCommand(MediaCommandKind.Next, null)
            }
        }
    }
}

@Composable
private fun MediaKey(
    icon: ImageVector,
    label: String,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    Box(
        Modifier
            .size(Tok.Phone.hit.dp)
            .clip(CircleShape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Symbol(icon, if (enabled) s.label1 else s.label3, size = Tok.Phone.symbol.dp) }
}

@Composable
private fun Artwork(bytes: ByteArray?) {
    val s = LinkTheme.scheme
    // The last image stays up while the next one decodes.
    val image =
        produceState<ImageBitmap?>(null, bytes) {
            value = withContext(Dispatchers.Default) { bytes?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }?.asImageBitmap() }
        }.value
    Box(
        Modifier
            .size(Tok.Size.rowTwo.dp)
            .clip(RoundedCornerShape(Tok.Radius.r12.dp))
            .background(Brush.linearGradient(listOf(s.accent, s.fill2))),
    ) {
        if (image != null) {
            Image(image, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        }
    }
}

/** A hairline that shows how far the track is; a tap or drag seeks, once the finger lifts. */
@Composable
private fun Progress(
    positionMs: Long,
    lengthMs: Long?,
    receivedAt: Long,
    playing: Boolean,
    seekable: Boolean,
    onSeek: (Long) -> Unit,
) {
    val s = LinkTheme.scheme
    var now by remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }
    LaunchedEffect(playing, receivedAt) {
        // Playback has no change event on this side; the line moves once a second while it plays.
        while (playing) {
            now = SystemClock.elapsedRealtime()
            delay(1_000)
        }
    }
    // A player without a length (a live stream, or media still loading) shows no line.
    if (lengthMs == null) return
    val position = if (playing) positionMs + (now - receivedAt) else positionMs
    val fraction = (position.toFloat() / lengthMs.coerceAtLeast(1)).coerceIn(0f, 1f)
    val t = LinkTheme.phoneType
    val line = Tok.Space.p4.dp
    val thumb = Tok.Space.p12.dp
    Column(Modifier.fillMaxWidth()) {
        DragTrack(
            fraction,
            stringResource(R.string.media_position),
            seekable,
            Modifier.fillMaxWidth().height(Tok.Size.controlS.dp),
            onDone = { onSeek((it * lengthMs).toLong()) },
        ) { value, _ ->
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(line)
                    .clip(CircleShape)
                    .background(s.fill2),
            )
            Box(
                Modifier
                    .fillMaxWidth(value)
                    .height(line)
                    .clip(CircleShape)
                    .background(s.label1),
            )
            BoxWithConstraints(Modifier.fillMaxWidth()) {
                Box(
                    Modifier
                        .offset(x = (maxWidth - thumb) * value)
                        .size(thumb)
                        .clip(CircleShape)
                        .background(s.label1),
                )
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            val times = t.caption.copy(fontFeatureSettings = "tnum")
            Text(clock(position.coerceAtMost(lengthMs)), times, color = s.label2)
            Text(clock(lengthMs), times, color = s.label2)
        }
    }
}

/** m:ss, or h:mm:ss from an hour on. */
private fun clock(ms: Long): String {
    val sec = ms.coerceAtLeast(0) / 1_000
    return if (sec >= 3_600) {
        "%d:%02d:%02d".format(sec / 3_600, sec / 60 % 60, sec % 60)
    } else {
        "%d:%02d".format(sec / 60, sec % 60)
    }
}

/** Offered and moving files in one view card: Save + decline while offered, progress + cancel while moving. */
@Composable
private fun Offers(
    offers: List<TransferRecord>,
    onSave: (String) -> Unit,
    onDecline: (String) -> Unit,
    onCancel: (String) -> Unit,
) {
    val context = LocalContext.current
    GroupCard(Modifier.padding(top = Tok.Space.p12.dp)) {
        offers.forEachIndexed { index, offer ->
            if (index > 0) Separator()
            val first =
                offer.files
                    .firstOrNull()
                    ?.name
                    .orEmpty()
            val name = pluralStringResource(R.plurals.offer_files, offer.files.size, first, offer.files.size - 1)
            val size = Formatter.formatShortFileSize(context, offer.total.takeIf { it > 0 } ?: offer.files.sumOf { it.size })
            val moving = offer.status == STATUS_MOVING
            if (moving) {
                val fraction = if (offer.total > 0) offer.bytes.toFloat() / offer.total else 0f
                OfferRow(
                    Icons.File,
                    name,
                    stringResource(R.string.offer_progress, (fraction * 100).toInt(), size),
                    nameLines = if (offer.files.size > 1) 2 else 1,
                    progress = fraction,
                    dismissLabel = stringResource(R.string.cancel),
                    onDismiss = { onCancel(offer.transferId) },
                )
            } else {
                OfferRow(
                    Icons.File,
                    name,
                    stringResource(R.string.offer_from, offer.desktopName, size),
                    nameLines = if (offer.files.size > 1) 2 else 1,
                    pill = { Pill(stringResource(R.string.offer_save), { onSave(offer.transferId) }, kind = PillKind.Primary) },
                    dismissLabel = stringResource(R.string.offer_decline),
                    onDismiss = { onDecline(offer.transferId) },
                )
            }
        }
    }
}

/** Do not disturb and Clipboard sync, Sleep | Screen off, the Screen mode tile, then the volume and brightness. */
@Composable
private fun Controls(
    desktop: Desktop,
    /** Null when the desktop does not list its capabilities: it then hides nothing. */
    capabilities: Set<String>?,
    status: DesktopStatus?,
    volume: DesktopVolume?,
    /** False while the media card holds the volume and brightness. */
    levels: Boolean,
    ringing: Boolean,
    autoClipboard: Boolean,
    controls: DeskControls,
    onViewer: () -> Unit,
    onClipboardSetup: () -> Unit,
) {
    val live = desktop.connected
    val has = { capability: String -> capabilities?.contains(capability) ?: true }
    val granted = CAN_CONTROL in capabilities.orEmpty()
    val control = live && granted
    Column(Modifier.alpha(if (live) 1f else Tok.Phone.offAlpha)) {
        SectionTitle(stringResource(R.string.home_controls))
        GroupCard {
            if (has("dnd")) {
                SwitchRow(
                    stringResource(R.string.key_dnd),
                    status?.dnd == true,
                    { controls.setDnd(desktop, it) },
                    icon = Icons.Dnd,
                    enabled = control,
                )
                Separator()
            }
            val clipboard = desktop.sharing.clipboard && autoClipboard
            SwitchRow(stringResource(R.string.home_clipboard_sync), clipboard, { on ->
                when {
                    on && !autoClipboard -> onClipboardSetup()
                    else -> controls.setSharing(desktop, Feature.Clipboard, on)
                }
            }, icon = Icons.Clip)
            Separator()
            // Not in the mockup (deviations.md): ringing is a state the desktop reports, so it is a switch row.
            SwitchRow(
                stringResource(R.string.key_ring),
                ringing,
                { controls.ring(desktop, it) },
                icon = Icons.Ring,
                enabled =
                    live && desktop.sharing.ring,
            )
        }
        if (has("suspend") || has("screen-off")) {
            SplitCapsule(
                listOfNotNull(
                    if (has("suspend")) Triple(Icons.Moon, stringResource(R.string.key_sleep)) { controls.sleep(desktop) } else null,
                    if (has(
                            "screen-off",
                        )
                    ) {
                        Triple(Icons.Monoff, stringResource(R.string.key_screen_off)) { controls.screenOff(desktop) }
                    } else {
                        null
                    },
                ),
                enabled = control,
            )
        }
        if (has(
                CAN_SCREEN,
            )
        ) {
            ModeTile(Icons.Screen, stringResource(R.string.key_screen), stringResource(R.string.home_screen_hint), live, onViewer)
        }
        if (levels) Levels(desktop, capabilities, status, volume, controls, volumeEnabled = control)
    }
}

/** Volume and brightness as one group: under the media card while a player plays, else at the end of the controls. */
@Composable
private fun Levels(
    desktop: Desktop,
    capabilities: Set<String>?,
    status: DesktopStatus?,
    volume: DesktopVolume?,
    controls: DeskControls,
    volumeEnabled: Boolean,
) {
    val control = desktop.connected && CAN_CONTROL in capabilities.orEmpty()
    Column {
        if (volume != null) {
            CapsuleSlider(
                volume.percent,
                Icons.Vol,
                stringResource(R.string.home_volume),
                volumeEnabled,
                { controls.setVolume(desktop, it, unmute = volume.muted) },
                Modifier.padding(top = Tok.Space.p8.dp),
            )
        }
        val brightness = status?.brightness
        if (brightness != null && capabilities?.contains("brightness") != false) {
            CapsuleSlider(
                brightness,
                Icons.Sun,
                stringResource(R.string.home_brightness),
                control,
                { controls.setBrightness(desktop, it) },
                Modifier.padding(top = Tok.Space.p8.dp),
            )
        }
    }
}

/** One-shot actions that belong together in one fill_1 capsule, split by a separator. */
@Composable
private fun SplitCapsule(
    actions: List<Triple<ImageVector, String, () -> Unit>>,
    enabled: Boolean,
) {
    val s = LinkTheme.scheme
    Row(
        Modifier
            .padding(top = Tok.Space.p8.dp)
            .fillMaxWidth()
            .height(Tok.Phone.hit.dp)
            .clip(CircleShape)
            .background(s.fill1),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        actions.forEachIndexed { index, (icon, label, onClick) ->
            if (index > 0) Box(Modifier.width(Dp.Hairline).height(Tok.Size.controlS.dp).background(s.separator))
            Row(
                Modifier.weight(1f).fillMaxHeight().clickable(enabled = enabled, role = Role.Button, onClick = onClick),
                horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Symbol(icon, s.label1, size = Tok.Size.symbolL.dp)
                Text(label, LinkTheme.phoneType.body.copy(fontWeight = FontWeight.Medium), maxLines = 1)
            }
        }
    }
}

/** A tile that opens a full screen: symbol without a circle, name, hint, chevron. */
@Composable
private fun ModeTile(
    icon: ImageVector,
    name: String,
    hint: String,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Row(
        Modifier
            .padding(top = Tok.Space.p8.dp)
            .fillMaxWidth()
            .heightIn(min = Tok.Phone.identity.dp)
            .clip(RoundedCornerShape(Tok.Radius.r16.dp))
            .background(s.fill1)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(Tok.Space.p12.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Symbol(icon, s.label1, modifier = Modifier.padding(horizontal = Tok.Space.p4.dp))
        Column(Modifier.weight(1f)) {
            Text(name, t.headline, maxLines = 1)
            Text(hint, t.footnote, color = s.label2, maxLines = 1)
        }
        Symbol(Icons.Chev, s.label3, size = Tok.Size.symbolL.dp)
    }
}

/** Floating over the page: Lock, Touchpad, Presenter, and Send, which opens Photos | Files | Text above it. */
@Composable
private fun Dock(
    desktop: Desktop,
    fan: Boolean,
    onFan: (Boolean) -> Unit,
    onLock: () -> Unit,
    onRemote: () -> Unit,
    onPresenter: () -> Unit,
    onPhotos: () -> Unit,
    onFiles: () -> Unit,
    onText: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val live = desktop.connected
    var confirmLock by remember { mutableStateOf(false) }
    Column(
        modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Phone.dockInset.dp),
        horizontalAlignment = Alignment.End,
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
    ) {
        if (fan) {
            val pick = { action: () -> Unit ->
                onFan(false)
                action()
            }
            FanCapsule(Icons.Photo, stringResource(R.string.picker_photos)) { pick(onPhotos) }
            FanCapsule(Icons.File, stringResource(R.string.picker_files)) { pick(onFiles) }
            FanCapsule(Icons.Msg, stringResource(R.string.home_send_text)) { pick(onText) }
            Spacer(Modifier.height(Tok.Space.p4.dp))
        }
        Row(
            Modifier.fillMaxWidth().alpha(if (live) 1f else Tok.Phone.offAlpha),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp + Tok.Space.p2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RoundKey(Icons.Lock, stringResource(R.string.key_lock), { confirmLock = true })
            RoundKey(Icons.Pad, stringResource(R.string.key_touchpad), onRemote)
            RoundKey(Icons.Pres, stringResource(R.string.key_presenter), onPresenter)
            Capsule(
                stringResource(if (fan) R.string.home_send_close else R.string.home_send),
                { onFan(!fan) },
                Modifier.weight(1f).elevation(Tok.Elevation.e3, Tok.Size.rowTwo.dp / 2),
                icon = if (fan) Icons.X else Icons.Send,
                kind = if (fan) CapsuleKind.Dark else CapsuleKind.Primary,
                large = true,
            )
        }
    }
    if (confirmLock) {
        LockDialog(desktop.name, { confirmLock = false }) {
            confirmLock = false
            onLock()
        }
    }
}

@Composable
private fun FanCapsule(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    Row(
        Modifier
            .height(Tok.Phone.hit.dp)
            .elevation(Tok.Elevation.e3, Tok.Phone.hit.dp / 2)
            .clip(CircleShape)
            .background(s.view)
            .border(Dp.Hairline, s.stroke, CircleShape)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = Tok.Space.p16.dp + Tok.Space.p2.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Symbol(icon, s.label1, size = Tok.Size.symbolL.dp)
        Text(label, LinkTheme.phoneType.headline, maxLines = 1)
    }
}

@Composable
private fun LockDialog(
    name: String,
    onDismiss: () -> Unit,
    onLock: () -> Unit,
) {
    val s = LinkTheme.scheme
    Dialog(onDismissRequest = onDismiss) {
        Column(
            Modifier
                .fillMaxWidth()
                .elevation(Tok.Elevation.e4, Tok.Radius.r28.dp)
                .clip(RoundedCornerShape(Tok.Radius.r28.dp))
                .background(s.view)
                .padding(Tok.Space.p20.dp),
            verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
        ) {
            Text(
                stringResource(R.string.home_lock_title, name),
                LinkTheme.phoneType.title3,
                modifier = Modifier.padding(bottom = Tok.Space.p8.dp),
            )
            Capsule(stringResource(R.string.key_lock), onLock, Modifier.fillMaxWidth(), kind = CapsuleKind.Primary)
            Capsule(stringResource(R.string.cancel), onDismiss, Modifier.fillMaxWidth())
        }
    }
}

/** Nothing paired: the empty glyph, what Link does, and Scan. */
@Composable
private fun Empty(onPair: () -> Unit) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Column(Modifier.fillMaxSize().padding(horizontal = Tok.Space.p16.dp), verticalArrangement = Arrangement.Center) {
        Box(
            Modifier.size(Tok.Phone.glyph.dp).clip(RoundedCornerShape(Tok.Radius.r16.dp)).background(s.fill1),
            contentAlignment = Alignment.Center,
        ) { Symbol(Icons.Laptop, s.label2, size = Tok.Size.controlS.dp + Tok.Space.p2.dp) }
        Text(stringResource(R.string.empty_scan_title), t.large, modifier = Modifier.padding(top = Tok.Space.p16.dp))
        Text(stringResource(R.string.empty_scan_body), t.paragraph, color = s.label2, modifier = Modifier.padding(top = Tok.Space.p8.dp))
        Capsule(
            stringResource(R.string.scan_action),
            onPair,
            Modifier.padding(top = Tok.Space.p24.dp).fillMaxWidth(),
            kind = CapsuleKind.Primary,
            large = true,
        )
        Text(stringResource(R.string.home_private_hint), t.footnote, color = s.label2, modifier = Modifier.padding(top = Tok.Space.p16.dp))
    }
}

private const val STATUS_MOVING = "transferring"
private val OFFER_STATUSES = setOf("offered", STATUS_MOVING)

private fun input(
    state: HomeState,
    desktop: Desktop,
) = desktop.control && (state.desk.can(desktop.id)?.contains(CAN_INPUT) ?: true)

@Composable
internal fun lastSeen(unixSeconds: Long): String =
    if (unixSeconds <= 0) stringResource(R.string.last_seen_never) else DateUtils.getRelativeTimeSpanString(unixSeconds * 1000).toString()
