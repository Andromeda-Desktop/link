package org.andromeda.link.devices

import android.Manifest
import android.app.Activity
import android.content.ClipboardManager
import android.content.pm.PackageManager
import android.os.Build
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalView
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
import androidx.core.view.WindowCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
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
import org.andromeda.link.devices.galaxy.GalaxyBackdrop
import org.andromeda.link.devices.galaxy.GalaxyControl
import org.andromeda.link.devices.galaxy.galaxyGestures
import org.andromeda.link.devices.galaxy.rememberGalaxy
import org.andromeda.link.files.PickerActions
import org.andromeda.link.files.PickerSheet
import org.andromeda.link.files.PickerTab
import org.andromeda.link.files.PickerViewModel
import org.andromeda.link.media.Artwork
import org.andromeda.link.media.MediaKeys
import org.andromeda.link.media.Progress
import org.andromeda.link.onboarding.Grant
import org.andromeda.link.ui.kit.BottomFade
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.CapsuleSlider
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.OfferRow
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.PillKind
import org.andromeda.link.ui.kit.PillSlider
import org.andromeda.link.ui.kit.PullRefresh
import org.andromeda.link.ui.kit.RoundKey
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.SectionTitle
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.Sheet
import org.andromeda.link.ui.kit.SwitchRow
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.DarkSurface
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
import org.andromeda.link.ui.theme.spring

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
    val resources = LocalResources.current
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
    val galaxy = rememberGalaxy()
    val ids = state.desktops.map { it.id }
    LaunchedEffect(ids, primary?.id) { primary?.let { galaxy.setDesks(ids, it.id) } }
    var overview by remember { mutableStateOf(false) }
    val pickDesk: (Desktop) -> Unit = { desktop ->
        galaxy.enter(desktop.id)
        overview = false
        viewModel.selectDesktop(desktop)
    }
    val leaveOverview = {
        galaxy.leave()
        overview = false
    }
    BackHandler(overview, leaveOverview)
    galaxy.onPick = { id -> state.desktops.firstOrNull { it.id == id }?.let(pickDesk) }
    val list = rememberLazyListState()
    var heroPx by remember { mutableFloatStateOf(0f) }
    val scroll = { if (list.firstVisibleItemIndex == 0) list.firstVisibleItemScrollOffset.toFloat() else heroPx }
    val view = LocalView.current
    val dark = LinkTheme.dark
    // The status bar sits on the night or dusk sky in both themes.
    DisposableEffect(dark, primary != null) {
        val window = (view.context as? Activity)?.window
        val insets = window?.let { WindowCompat.getInsetsController(it, view) }
        if (primary != null) insets?.isAppearanceLightStatusBars = false
        onDispose { insets?.isAppearanceLightStatusBars = !dark }
    }
    val shown by animateFloatAsState(if (overview) 1f else 0f, spring(Tok.Spring.fade), label = "overview")
    Box(modifier) {
        if (primary != null) GalaxyBackdrop(galaxy, scroll, Tok.Phone.heroScene, Modifier.fillMaxSize())
        Screen(
            modifier = Modifier.graphicsLayer { alpha = 1 - shown }.then(if (overview) Modifier.clearAndSetSemantics {} else Modifier),
            background = if (primary != null) Color.Transparent else LinkTheme.scheme.solidRegular,
            toast = toast,
            onToastEnd = viewModel::messageShown,
            toastInset = (Tok.Phone.dockInset + Tok.Phone.hit + Tok.Space.p12).dp,
            overlay = {
                if (primary != null) {
                    // Under the open picker, Home's controls would duplicate the sheet's for TalkBack.
                    val hidden = if (pickOpen) Modifier.clearAndSetSemantics {} else Modifier
                    // Opaque from the dock's top edge down, so no sheet row shows between its keys.
                    val nav =
                        WindowInsets.navigationBars
                            .asPaddingValues()
                            .calculateBottomPadding()
                            .value
                    BottomFade(
                        (Tok.Phone.fadeDock + nav).dp,
                        Modifier.align(Alignment.BottomCenter),
                        solid = 1 - (Tok.Phone.dockInset + Tok.Phone.hit + nav) / (Tok.Phone.fadeDock + nav),
                    )
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
                }
            },
        ) {
            if (primary == null) {
                if (state.nonePaired) Empty(onPair)
                return@Screen
            }
            val desk = state.desk
            val status = desk.statuses[primary.id]
            val live = primary.connected
            val player = desk.player(primary.id)?.takeIf { live }
            val hidden = if (pickOpen) Modifier.clearAndSetSemantics {} else Modifier
            PullRefresh(state.refreshing, viewModel::refresh, Modifier.fillMaxSize().then(hidden)) {
                LazyColumn(Modifier.fillMaxSize(), state = list) {
                    item {
                        Hero(
                            primary,
                            state.refreshing,
                            missing.size,
                            scrim = dark,
                            scroll = scroll,
                            onOpen = {
                                if (state.desktops.size > 1) {
                                    overview = galaxy.overview()
                                } else {
                                    onDesktop(primary)
                                }
                            },
                            onFixes = { sheet = HomeSheet.Fixes },
                            onActivity = onActivity,
                            onSettings = onSettings,
                            modifier =
                                Modifier
                                    .fillParentMaxHeight(Tok.Phone.hero)
                                    .onSizeChanged { heroPx = it.height.toFloat() }
                                    .galaxyGestures(galaxy),
                        )
                    }
                    item { SheetTop() }
                    if (primary.link == DesktopLink.Offline) {
                        item { SheetPart { StatusCard(primary, state.refreshing) { viewModel.connect(primary) } } }
                    } else if (player != null) {
                        item {
                            SheetPart {
                                MediaLevels(
                                    player,
                                    primary,
                                    desk.can(primary.id),
                                    status,
                                    desk.volumes[primary.id],
                                    controls,
                                    onOpen = onMedia,
                                ) { command, value -> viewModel.media(player, command, value) }
                            }
                        }
                    }
                    val offers = desk.transfers.filter { it.incoming == true && it.status in OFFER_STATUSES }
                    if (live && offers.isNotEmpty()) {
                        item {
                            SheetPart {
                                Offers(
                                    offers,
                                    onSave = viewModel::acceptTransfer,
                                    onDecline = viewModel::declineTransfer,
                                    onCancel = viewModel::cancelTransfer,
                                )
                            }
                        }
                    }
                    item {
                        SheetPart {
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
                    // The sheet reaches the bottom edge however short its content is, and leaves room under the dock.
                    item { SheetPart(Modifier.fillParentMaxHeight(1 - Tok.Phone.hero)) {} }
                }
            }
        }
        if (overview) {
            DeskOverview(galaxy, state.desktops, pickDesk, leaveOverview, Modifier.graphicsLayer { alpha = shown })
        }
    }
}

/** Every desktop's galaxy, named under it; tap one to open it, tap twice to go back. */
@Composable
private fun DeskOverview(
    galaxy: GalaxyControl,
    desktops: List<Desktop>,
    onPick: (Desktop) -> Unit,
    onLeave: () -> Unit,
    modifier: Modifier = Modifier,
) = DarkSurface {
    val t = LinkTheme.phoneType
    Box(
        modifier
            .fillMaxSize()
            .pointerInput(galaxy) { detectTapGestures(onTap = { galaxy.tap(it) }, onDoubleTap = { onLeave() }) },
    ) {
        galaxy.spots?.forEach { spot ->
            val desktop = desktops.firstOrNull { it.id == spot.id } ?: return@forEach
            Column(
                Modifier
                    .layout { m, c ->
                        val p = m.measure(c.copy(minWidth = 0, minHeight = 0))
                        layout(p.width, p.height) {
                            val x = (spot.at.x - p.width / 2).toInt().coerceIn(0, maxOf(0, c.maxWidth - p.width))
                            p.place(x, (spot.at.y + spot.radius * LABEL_DROP).toInt())
                        }
                    }.clip(RoundedCornerShape(Tok.Radius.r12.dp))
                    .clickable(role = Role.Button) { onPick(desktop) }
                    .padding(Tok.Space.p8.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(desktop.name, t.body.copy(fontWeight = FontWeight.SemiBold), maxLines = 1)
                Presence(desktop, false)
            }
        }
        Text(
            stringResource(R.string.overview_hint),
            t.footnote,
            Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = Tok.Space.p32.dp),
            color = LinkTheme.scheme.label2,
        )
    }
}

/** The label's top sits this share of the galaxy's radius below its centre. */
private const val LABEL_DROP = .45f

/** Which sheet covers Home; one at a time. */
private enum class HomeSheet { None, Fixes }

/**
 * The desktop over the galaxy: Activity and Settings keys at the top, the name, presence and fix chip at the foot. The
 * name block moves hero_identity of the sheet's scroll; the scene behind it hero_scene.
 */
@Composable
private fun Hero(
    desktop: Desktop,
    refreshing: Boolean,
    fixes: Int,
    scrim: Boolean,
    scroll: () -> Float,
    onOpen: () -> Unit,
    onFixes: () -> Unit,
    onActivity: () -> Unit,
    onSettings: () -> Unit,
    modifier: Modifier = Modifier,
) = DarkSurface {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Box(modifier.fillMaxWidth()) {
        // Night only: the dusk sky is already dark behind the name.
        if (scrim) {
            Box(
                Modifier
                    .align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    .fillMaxHeight(HERO_SCRIM)
                    .background(Brush.verticalGradient(listOf(Color.Transparent, s.scrim))),
            )
        }
        Row(Modifier.align(Alignment.TopEnd).padding(end = Tok.Space.p8.dp)) {
            IconKey(Icons.Clock, stringResource(R.string.activity_title), onActivity)
            IconKey(Icons.Gear, stringResource(R.string.settings_title), onSettings)
        }
        Column(
            Modifier
                .align(Alignment.BottomStart)
                .padding(start = Tok.Space.p24.dp, end = Tok.Space.p24.dp, bottom = Tok.Space.p24.dp)
                .graphicsLayer { translationY = scroll() * (1 - Tok.Phone.heroIdentity) },
        ) {
            Row(
                Modifier
                    .clip(RoundedCornerShape(Tok.Radius.r12.dp))
                    .clickable(role = Role.Button, onClickLabel = stringResource(R.string.desktop_open), onClick = onOpen),
                horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(desktop.name, t.large, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                Symbol(Icons.Chev, s.label2, size = Tok.Phone.symbol.dp, modifier = Modifier.rotate(QUARTER_TURN))
            }
            Row(
                horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp + Tok.Space.p2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Presence(desktop, refreshing, Modifier.weight(1f, fill = false))
                if (fixes > 0) FixChip(fixes, onFixes)
            }
        }
    }
}

/** The media tile's art: a scrim at the top, dimmed to ART_DIM at ART_FADE of its height, solid at the foot. */
private const val ART_FADE = 0.45f
private const val ART_DIM = 0.75f

/** The dark scrim under the name covers this share of the hero, from its foot. */
private const val HERO_SCRIM = 0.45f

/** Peak alpha of the sheet's shadow over the light sky. */
private const val SHEET_SHADOW = 0.25f
private const val QUARTER_TURN = 90f

/**
 * The sheet's rounded top edge with its grab bar; the sheet scrolls over the galaxy. Its top is a step lighter and fades
 * into the sheet; over the light sky a soft shadow rises above it.
 */
@Composable
private fun SheetTop() {
    val s = LinkTheme.scheme
    val r = Tok.Phone.sheetRadius.dp
    val dark = LinkTheme.dark
    val shadow = Tok.Phone.sheetLift.dp
    Box(
        Modifier
            .fillMaxWidth()
            .drawBehind {
                if (dark) return@drawBehind
                val lift = shadow.toPx()
                drawRect(
                    Brush.verticalGradient(listOf(Color.Transparent, s.shadow.copy(alpha = SHEET_SHADOW)), startY = -lift, endY = r.toPx()),
                    topLeft = Offset(0f, -lift),
                    size = Size(size.width, lift + r.toPx()),
                )
            }.clip(RoundedCornerShape(topStart = r, topEnd = r))
            .background(Brush.verticalGradient(listOf(s.solidThick, s.solidRegular)))
            .padding(top = Tok.Space.p12.dp, bottom = Tok.Space.p12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(Tok.Phone.grab.dp, Tok.Space.p4.dp).clip(CircleShape).background(s.label3))
    }
}

/** One slice of the sheet: its colour edge to edge, the content inset like the rest of Home. */
@Composable
private fun SheetPart(
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) = Box(
    modifier
        .fillMaxWidth()
        .background(LinkTheme.scheme.solidRegular)
        .padding(horizontal = Tok.Space.p24.dp),
) { content() }

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

/** The player as a square tile of its art, beside the two level pills (volume, brightness) at the same height. */
@Composable
private fun MediaLevels(
    shown: DesktopPlayer,
    desktop: Desktop,
    capabilities: Set<String>?,
    status: DesktopStatus?,
    volume: DesktopVolume?,
    controls: DeskControls,
    onOpen: () -> Unit,
    onCommand: (MediaCommandKind, Long?) -> Unit,
) {
    val control = desktop.connected && CAN_CONTROL in capabilities.orEmpty()
    // The pills' shadow falls into this part's own bottom room; the next sheet part would paint over it.
    BoxWithConstraints(Modifier.fillMaxWidth().padding(bottom = Tok.Space.p16.dp)) {
        val gap = Tok.Phone.levelGap.dp
        val tile = (maxWidth - gap * 2) / 2
        Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
            MediaTile(shown, tile, onOpen, onCommand)
            PillSlider(
                volume?.percent,
                if (volume?.muted == true) Icons.VolOff else Icons.Vol,
                stringResource(R.string.home_volume),
                control,
                { controls.setVolume(desktop, it, unmute = volume?.muted == true) },
                Modifier.weight(1f).height(tile),
                onIcon =
                    volume?.let {
                        stringResource(if (it.muted) R.string.home_unmute else R.string.home_mute) to
                            { controls.setMuted(desktop, !it.muted) }
                    },
            )
            PillSlider(
                status?.brightness?.takeIf { capabilities?.contains("brightness") != false },
                Icons.Sun,
                stringResource(R.string.home_brightness),
                control,
                { controls.setBrightness(desktop, it) },
                Modifier.weight(1f).height(tile),
            )
        }
    }
}

@Composable
private fun MediaTile(
    shown: DesktopPlayer,
    size: Dp,
    onOpen: () -> Unit,
    onCommand: (MediaCommandKind, Long?) -> Unit,
) = DarkSurface {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val player = shown.player
    val playing = player.state == PlaybackState.Playing
    Box(
        Modifier
            .size(size)
            .clip(RoundedCornerShape(Tok.Radius.r28.dp))
            .clickable(role = Role.Button, onClick = onOpen),
    ) {
        Artwork(player.artwork, size, 0.dp)
        Box(
            Modifier.matchParentSize().background(
                Brush.verticalGradient(
                    0f to s.scrim,
                    ART_FADE to s.solidRegular.copy(alpha = ART_DIM),
                    1f to s.solidRegular,
                ),
            ),
        )
        Column(Modifier.matchParentSize().padding(Tok.Space.p12.dp)) {
            Text(player.name, t.caption, color = s.label2, maxLines = 1)
            Spacer(Modifier.weight(1f))
            Text(player.title.ifEmpty { player.name }, t.headline, maxLines = 1)
            if (player.artist.isNotEmpty()) Text(player.artist, t.footnote, color = s.label2, maxLines = 1)
            Progress(player.positionMs, player.lengthMs, player.receivedAt, playing, MediaCommandKind.Seek in player.can) {
                onCommand(MediaCommandKind.Seek, it)
            }
            MediaKeys(player, onCommand, compact = true)
        }
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

/** On this desktop (Screen, Clipboard sync, Do not disturb, Remote control), Actions (Sleep, Ring), then the levels. */
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
        SectionTitle(stringResource(R.string.home_on_desktop))
        GroupCard {
            if (capabilities?.contains("screen-on") == true) {
                SwitchRow(
                    stringResource(R.string.key_screen),
                    status?.screenOff == false,
                    { controls.setScreenOff(desktop, !it) },
                    icon = Icons.Screen,
                    enabled = control,
                )
                Separator()
            }
            val clipboard = desktop.sharing.clipboard && autoClipboard
            SwitchRow(
                stringResource(R.string.home_clipboard_sync),
                clipboard,
                { on -> if (on && !autoClipboard) onClipboardSetup() else controls.setSharing(desktop, Feature.Clipboard, on) },
                icon = Icons.Clip,
            )
            if (has("dnd")) {
                Separator()
                SwitchRow(
                    stringResource(R.string.key_dnd),
                    status?.dnd == true,
                    { controls.setDnd(desktop, it) },
                    icon = Icons.Dnd,
                    enabled = control,
                )
            }
            if (has(CAN_SCREEN)) {
                Separator()
                ValueRow(
                    stringResource(R.string.viewer_title),
                    icon = Icons.Screen,
                    chevron = Icons.Chev,
                    enabled = live,
                    onClick = onViewer,
                )
            }
        }
        SectionTitle(stringResource(R.string.home_actions))
        GroupCard {
            if (has("suspend")) {
                ValueRow(
                    stringResource(R.string.key_sleep),
                    icon = Icons.Moon,
                    enabled = control,
                    onClick = { controls.sleep(desktop) },
                )
                Separator()
            }
            // A desktop without screen-on state gets the one-way action instead of the Screen switch.
            if (has("screen-off") && capabilities?.contains("screen-on") != true) {
                ValueRow(
                    stringResource(R.string.key_screen_off),
                    icon = Icons.Monoff,
                    enabled = control,
                    onClick = { controls.setScreenOff(desktop, true) },
                )
                Separator()
            }
            // Ringing is a state the desktop reports, so it is a switch row.
            SwitchRow(
                stringResource(R.string.key_ring),
                ringing,
                { controls.ring(desktop, it) },
                icon = Icons.Ring,
                enabled = live && desktop.sharing.ring,
            )
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
                if (volume.muted) Icons.VolOff else Icons.Vol,
                stringResource(R.string.home_volume),
                volumeEnabled,
                { controls.setVolume(desktop, it, unmute = volume.muted) },
                Modifier.padding(top = Tok.Space.p8.dp),
                onIcon =
                    stringResource(if (volume.muted) R.string.home_unmute else R.string.home_mute) to
                        { controls.setMuted(desktop, !volume.muted) },
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
            // These act on the desktop at once, so they need it connected.
            RoundKey(Icons.Lock, stringResource(R.string.key_lock), { confirmLock = true }, enabled = live)
            RoundKey(Icons.Pad, stringResource(R.string.key_touchpad), onRemote, enabled = live)
            RoundKey(Icons.Pres, stringResource(R.string.key_presenter), onPresenter, enabled = live)
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
