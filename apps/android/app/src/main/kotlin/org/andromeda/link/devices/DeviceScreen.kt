package org.andromeda.link.devices

import android.content.Context
import android.content.Intent
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.andromeda.link.R
import org.andromeda.link.clipboard.LogAccess
import org.andromeda.link.core.domain.Desktop
import org.andromeda.link.core.domain.Feature
import org.andromeda.link.files.PhoneFiles
import org.andromeda.link.screen.ScreenInput
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.BottomFade
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.SectionTitle
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.Sheet
import org.andromeda.link.ui.kit.SwitchRow
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

/** One desktop: what it may do on this phone, what this phone may open or control on it, and unpairing. */
@Composable
fun DeviceScreen(
    viewModel: DeviceViewModel,
    onBack: () -> Unit,
    onMirrorApps: () -> Unit,
    onRemote: () -> Unit,
    onPresenter: () -> Unit,
    onViewer: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val unpaired by viewModel.unpaired.collectAsStateWithLifecycle()
    var confirmUnpair by rememberSaveable { mutableStateOf(false) }
    var controlEnabled by rememberSaveable { mutableStateOf(ScreenInput.enabled || LogAccess.helperPossible()) }
    val context = LocalContext.current
    var allFiles by rememberSaveable { mutableStateOf(PhoneFiles.allFiles(context)) }
    LifecycleResumeEffect(Unit) {
        controlEnabled = ScreenInput.enabled || LogAccess.helperPossible()
        allFiles = PhoneFiles.allFiles(context)
        onPauseOrDispose {}
    }
    val back by rememberUpdatedState(onBack)
    LaunchedEffect(unpaired) { if (unpaired) back() }
    val resources = LocalResources.current
    val desktop = state.desktop
    Box(modifier) {
        Screen(
            toast = state.failure?.text(resources),
            onToastEnd = viewModel::failureShown,
            background = LinkTheme.scheme.solidRegular,
            overlay = {
                BottomFade(Tok.Space.p48.dp, Modifier.align(Alignment.BottomCenter))
                if (desktop != null) {
                    Sheet(
                        confirmUnpair,
                        { if (!state.busy) confirmUnpair = false },
                        stringResource(R.string.device_unpair_title),
                        subtitle = stringResource(R.string.device_unpair_body, desktop.name),
                    ) {
                        Capsule(
                            stringResource(R.string.unpair),
                            { if (!state.busy) viewModel.unpair() },
                            Modifier.fillMaxWidth(),
                            kind = CapsuleKind.Danger,
                            large = true,
                        )
                        Capsule(
                            stringResource(R.string.cancel),
                            { if (!state.busy) confirmUnpair = false },
                            Modifier.fillMaxWidth(),
                            large = true,
                        )
                    }
                }
            },
        ) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                BackKey(LinkTheme.scheme.label1, onBack, Modifier.padding(horizontal = Tok.Space.p12.dp))
                if (desktop == null) return@Column
                Column(Modifier.padding(horizontal = Tok.Space.p16.dp)) {
                    Identity(desktop, state.busy, viewModel::connect)
                    Grants(
                        desktop,
                        state,
                        allFiles,
                        controlEnabled,
                        viewModel::setSharing,
                        onMirrorApps,
                        { context.startActivity(allFilesSettings(context)) },
                        { context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) },
                    )
                    Control(desktop, state, onRemote, onPresenter, onViewer, viewModel::ping)
                    if (desktop.control && state.commands.isNotEmpty()) {
                        SectionTitle(stringResource(R.string.device_run_on, desktop.name))
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
                            state.commands.forEach { command ->
                                Pill(command.name, { if (!state.busy) viewModel.run(command) })
                            }
                        }
                    }
                    Capsule(
                        stringResource(R.string.device_unpair, desktop.name),
                        { if (!state.busy) confirmUnpair = true },
                        Modifier
                            .fillMaxWidth()
                            .padding(top = Tok.Space.p32.dp)
                            .navigationBarsPadding()
                            .padding(bottom = Tok.Space.p48.dp),
                        kind = CapsuleKind.Danger,
                        large = true,
                    )
                }
            }
        }
    }
}

/** The desktop glyph, its name and where it is; an away desktop offers Connect. */
@Composable
private fun Identity(
    desktop: Desktop,
    busy: Boolean,
    onConnect: () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val on = desktop.connected
    Column {
        Row(
            Modifier.padding(top = Tok.Space.p8.dp),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier
                    .size(Tok.Phone.glyph.dp)
                    .clip(RoundedCornerShape(Tok.Radius.r16.dp))
                    .background(if (on) s.accent.copy(alpha = Tok.Phone.glyphTint) else s.fill1),
                contentAlignment = Alignment.Center,
            ) { Symbol(Icons.Laptop, if (on) s.accent else s.label2, size = Tok.Size.controlS.dp) }
            Column(Modifier.weight(1f)) {
                Text(desktop.name, t.title1, maxLines = 2)
                Row(horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(Tok.Space.p8.dp).clip(CircleShape).background(if (on) s.success else s.label3))
                    val via = stringResource(if (desktop.bluetooth) R.string.home_via_bluetooth else R.string.home_via_wifi)
                    Text(
                        if (on) {
                            stringResource(R.string.home_connected_via, via)
                        } else {
                            disconnectedLine(desktop)
                        },
                        t.footnote,
                        color = s.label2,
                        maxLines = 1,
                    )
                }
            }
        }
        if (!on) {
            Capsule(
                stringResource(R.string.connect),
                { if (!busy) onConnect() },
                Modifier.fillMaxWidth().padding(top = Tok.Space.p16.dp),
                icon = Icons.Refresh,
                kind = CapsuleKind.Primary,
                large = true,
            )
        }
    }
}

/** What the desktop may do on this phone, in three groups: share, this phone, and what it can open. */
@Composable
private fun Grants(
    desktop: Desktop,
    state: DeviceState,
    allFiles: Boolean,
    controlEnabled: Boolean,
    onShare: (Feature, Boolean) -> Unit,
    onMirrorApps: () -> Unit,
    onAllFiles: () -> Unit,
    onAccessibility: () -> Unit,
) {
    val sharing = desktop.sharing
    val enabled = !state.busy

    @Composable
    fun Toggle(
        feature: Feature,
        label: Int,
        icon: ImageVector,
    ) = SwitchRow(stringResource(label), sharing.allows(feature), { onShare(feature, it) }, icon = icon, enabled = enabled)

    SectionTitle(stringResource(R.string.device_share))
    GroupCard {
        val rows =
            buildList<@Composable () -> Unit> {
                if (state.desktopCan(CAN_CLIPBOARD)) add { Toggle(Feature.Clipboard, R.string.feature_clipboard, Icons.Clip) }
                add { Toggle(Feature.Files, R.string.feature_files, Icons.File) }
                add { Toggle(Feature.Media, R.string.feature_media, Icons.Music) }
                if (state.desktopCan(CAN_NOTIFICATIONS)) {
                    add { Toggle(Feature.Notifications, R.string.feature_notifications, Icons.Bell) }
                    if (sharing.allows(Feature.Notifications)) {
                        add {
                            ValueRow(
                                stringResource(R.string.mirror_apps_choose),
                                Modifier.padding(start = SUB_INDENT),
                                chevron = Icons.Chev,
                                accent = true,
                                onClick = onMirrorApps,
                            )
                        }
                    }
                }
            }
        Rows(rows)
    }
    SectionTitle(stringResource(R.string.device_this_phone))
    GroupCard {
        Rows(
            listOf(
                { Toggle(Feature.Ring, R.string.feature_ring, Icons.Bell) },
                { Toggle(Feature.Calls, R.string.feature_calls, Icons.Call) },
                { Toggle(Feature.Messages, R.string.feature_messages, Icons.Msg) },
            ),
        )
    }
    SectionTitle(stringResource(R.string.device_can_open, desktop.name))
    GroupCard {
        val rows =
            buildList<@Composable () -> Unit> {
                add { Toggle(Feature.Browse, R.string.device_open_files, Icons.Folder) }
                if (sharing.browse && !allFiles) {
                    add {
                        ValueRow(
                            stringResource(R.string.device_browse_photos_only, desktop.name),
                            Modifier.padding(start = SUB_INDENT),
                        ) { Pill(stringResource(R.string.settings_turn_on), onAllFiles) }
                    }
                }
                add { Toggle(Feature.Screen, R.string.device_open_screen, Icons.Screen) }
                if (sharing.screen && !controlEnabled) {
                    add {
                        ValueRow(
                            stringResource(R.string.screen_control_off),
                            Modifier.padding(start = SUB_INDENT),
                            chevron = Icons.Chev,
                            accent = true,
                            onClick = onAccessibility,
                        )
                    }
                }
            }
        Rows(rows)
    }
}

/** What this phone may control on the desktop; the desktop decides, so rows are off when it does not allow input. */
@Composable
private fun Control(
    desktop: Desktop,
    state: DeviceState,
    onRemote: () -> Unit,
    onPresenter: () -> Unit,
    onViewer: () -> Unit,
    onPing: () -> Unit,
) {
    val allowed = state.capabilities?.contains(CAN_CONTROL) == true
    val input = state.desktopCan(CAN_INPUT)
    val off = stringResource(R.string.device_not_allowed)
    SectionTitle(stringResource(R.string.device_control))
    GroupCard {
        val rows =
            buildList<@Composable () -> Unit> {
                add {
                    ValueRow(
                        stringResource(R.string.device_control_system),
                        icon = Icons.Sliders,
                        subtitle = stringResource(R.string.device_control_system_hint),
                        value = stringResource(if (allowed) R.string.device_allowed else R.string.device_not_allowed),
                    )
                }
                if (desktop.control) {
                    add { Open(R.string.remote_title, R.string.device_touchpad_hint, Icons.Pad, input, off, onRemote) }
                    add { Open(R.string.presenter_title, R.string.device_presenter_hint, Icons.Pres, input, off, onPresenter) }
                    if (state.capabilities?.contains(CAN_SCREEN) == true) {
                        add { Open(R.string.viewer_title, R.string.device_viewer_hint, Icons.Monitor, input, off, onViewer) }
                    }
                    add { Open(R.string.ping, R.string.ping_hint, Icons.Ping, true, off, onPing) }
                }
            }
        Rows(rows)
    }
    Text(
        stringResource(R.string.device_desktop_decides, desktop.name),
        LinkTheme.phoneType.footnote,
        Modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p8.dp),
        color = LinkTheme.scheme.label2,
    )
}

@Composable
private fun Open(
    title: Int,
    subtitle: Int,
    icon: ImageVector,
    enabled: Boolean,
    off: String,
    onClick: () -> Unit,
) {
    ValueRow(
        stringResource(title),
        icon = icon,
        subtitle = stringResource(subtitle),
        value = if (enabled) null else off,
        chevron = if (enabled) Icons.Chev else null,
        onClick = if (enabled) onClick else null,
    )
}

/** Rows of one group card, with full-width separators between them. */
@Composable
private fun Rows(rows: List<@Composable () -> Unit>) {
    rows.forEachIndexed { index, row ->
        if (index > 0) Separator()
        row()
    }
}

/** A sub-row starts under its parent's title: the row's symbol and the gap. */
private val SUB_INDENT = (Tok.Phone.symbol + Tok.Space.p12).dp

/** Android 11+ grants All files access on its own page; Android 10 has only the storage permission in App info. */
private fun allFilesSettings(context: Context): Intent =
    Intent(
        if (Build.VERSION.SDK_INT >= 30) {
            Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION
        } else {
            Settings.ACTION_APPLICATION_DETAILS_SETTINGS
        },
        "package:${context.packageName}".toUri(),
    )
