package org.andromeda.link.devices

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Process
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.andromeda.link.AppContainer
import org.andromeda.link.R
import org.andromeda.link.clipboard.LogAccess
import org.andromeda.link.clipboard.LogAccess.Method
import org.andromeda.link.core.domain.Desktop
import org.andromeda.link.onboarding.Grant
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.BottomFade
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.SectionTitle
import org.andromeda.link.ui.kit.Segmented
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.SwitchRow
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.theme.AppTheme
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.rememberAppTheme
import org.andromeda.link.ui.theme.setAppTheme

private enum class SettingsPage { Main, Advanced }

/** The app's own settings: desktops, permissions, general, appearance, about; Advanced holds the rarely changed rest. */
@Composable
fun SettingsScreen(
    container: AppContainer,
    autoClipboard: Boolean,
    /** Required grants not given yet. */
    missing: List<Grant>,
    onBack: () -> Unit,
    onDesktop: (Desktop) -> Unit,
    onPair: () -> Unit,
    onMirrorApps: () -> Unit,
    onSetup: () -> Unit,
    onClipboardSetup: () -> Unit,
    /** The log access method sheet; open, the app hides its tab bar as for Home's picker. */
    methodSheet: Boolean,
    onMethodSheet: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    viewModel: HomeViewModel = viewModel { HomeViewModel(container.repository, container.presence, container.mirror, container.clipboard) },
) {
    val context = LocalContext.current
    val state by viewModel.state.collectAsStateWithLifecycle()
    val prefs by viewModel.notificationPrefs.collectAsStateWithLifecycle()
    val clearsSynced by viewModel.clearsSynced.collectAsStateWithLifecycle()
    val sendsScreenshots by viewModel.sendsScreenshots.collectAsStateWithLifecycle()
    var page by rememberSaveable { mutableStateOf(SettingsPage.Main) }
    BackHandler(page == SettingsPage.Advanced) { page = SettingsPage.Main }
    val logMethod by LogAccess.method.collectAsStateWithLifecycle()
    var methods by remember { mutableStateOf(logMethods(context)) }
    var logs by remember { mutableStateOf(false) }
    // Read again on resume: the grant comes from adb, Shizuku or root while Link is in the background.
    LifecycleResumeEffect(logMethod) {
        logs = context.checkSelfPermission(Manifest.permission.READ_LOGS) == PackageManager.PERMISSION_GRANTED
        methods = logMethods(context)
        onPauseOrDispose {}
    }
    Box(modifier) {
        Screen(
            background = LinkTheme.scheme.solidRegular,
            overlay = {
                BottomFade(Tok.Space.p48.dp, Modifier.align(Alignment.BottomCenter))
                LogAccessSheet(
                    methodSheet,
                    logMethod,
                    methods,
                    { LogAccess.choose(context, it) },
                    onClipboardSetup,
                    { onMethodSheet(false) },
                )
            },
        ) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                val advanced = page == SettingsPage.Advanced
                BackKey(
                    LinkTheme.scheme.label1,
                    if (advanced) ({ page = SettingsPage.Main }) else onBack,
                    Modifier.padding(horizontal = Tok.Space.p12.dp),
                )
                Column(Modifier.padding(horizontal = Tok.Space.p16.dp)) {
                    Text(
                        stringResource(if (advanced) R.string.settings_advanced else R.string.settings_title),
                        LinkTheme.phoneType.large,
                        modifier = Modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p8.dp),
                        maxLines = 1,
                    )
                    if (advanced) {
                        Advanced(
                            prefs,
                            clearsSynced,
                            sendsScreenshots,
                            logs,
                            methods.inUse,
                            { onMethodSheet(true) },
                            viewModel::setFullContentWhileLocked,
                            viewModel::setSendsScreenshots,
                            viewModel::setClearsSynced,
                        )
                    } else {
                        Main(
                            state,
                            prefs,
                            autoClipboard,
                            missing,
                            viewModel::setStayConnected,
                            onDesktop,
                            onPair,
                            onMirrorApps,
                            onSetup,
                            onClipboardSetup,
                        ) { page = SettingsPage.Advanced }
                    }
                    Box(Modifier.navigationBarsPadding().padding(bottom = Tok.Space.p48.dp))
                }
            }
        }
    }
}

@Composable
private fun ColumnScope.Main(
    state: HomeState,
    prefs: NotificationPrefs,
    autoClipboard: Boolean,
    missing: List<Grant>,
    onStay: (Boolean) -> Unit,
    onDesktop: (Desktop) -> Unit,
    onPair: () -> Unit,
    onMirrorApps: () -> Unit,
    onSetup: () -> Unit,
    onClipboardSetup: () -> Unit,
    onMore: () -> Unit,
) {
    val theme by rememberAppTheme()
    val context = LocalContext.current
    val s = LinkTheme.scheme
    val version =
        remember(context) {
            context.packageManager
                .getPackageInfo(context.packageName, 0)
                .versionName
                .orEmpty()
        }

    SectionTitle(stringResource(R.string.settings_desktops))
    GroupCard {
        for (desktop in state.desktops) {
            val via = stringResource(if (desktop.bluetooth) R.string.home_via_bluetooth else R.string.home_via_wifi)
            ValueRow(
                desktop.name,
                subtitle = if (desktop.connected) stringResource(R.string.home_connected_via, via) else disconnectedLine(desktop),
                chevron = Icons.Chev,
                onClick = { onDesktop(desktop) },
                leading = {
                    Box(
                        Modifier
                            .size(Tok.Space.p8.dp)
                            .clip(CircleShape)
                            .background(if (desktop.connected) s.success else s.label3),
                    )
                },
            )
            Separator()
        }
        if (state.desktops.isEmpty()) {
            ValueRow(stringResource(R.string.settings_no_desktop), subtitle = stringResource(R.string.settings_no_desktop_hint))
            Separator()
        }
        ValueRow(stringResource(R.string.pair_action), icon = Icons.Plus, accent = true, onClick = onPair)
    }

    val required = Grant.entries.filter { it != Grant.Texts && it != Grant.AutoClipboard }
    SectionTitle(
        stringResource(R.string.setup_title),
        detail = stringResource(R.string.settings_grants_on, required.size - missing.size, required.size),
    )
    GroupCard {
        for (grant in required.filter { it in missing }) {
            ValueRow(stringResource(grant.title()), icon = grant.icon()) {
                Pill(stringResource(R.string.settings_turn_on), onSetup)
            }
            Separator()
        }
        ValueRow(stringResource(R.string.settings_all_permissions), chevron = Icons.Chev, onClick = onSetup)
    }

    SectionTitle(stringResource(R.string.settings_general))
    GroupCard {
        SwitchRow(stringResource(R.string.settings_keep_open), state.stayConnected, onStay, icon = Icons.Wifi)
        Separator()
        ValueRow(
            stringResource(R.string.settings_notifications),
            icon = Icons.Bell,
            value =
                if (prefs.excluded == 0) {
                    stringResource(R.string.settings_apps_all)
                } else {
                    pluralStringResource(R.plurals.settings_apps_except, prefs.excluded, prefs.excluded)
                },
            chevron = Icons.Chev,
            onClick = onMirrorApps,
        )
        Separator()
        ValueRow(
            stringResource(R.string.settings_clipboard),
            icon = Icons.Clip,
            value = stringResource(if (autoClipboard) R.string.settings_clipboard_auto else R.string.settings_set_up),
            chevron = Icons.Chev,
            onClick = onClipboardSetup,
        )
    }

    SectionTitle(stringResource(R.string.settings_appearance))
    Segmented(
        listOf(
            AppTheme.System to stringResource(R.string.theme_system),
            AppTheme.Light to stringResource(R.string.theme_light),
            AppTheme.Dark to stringResource(R.string.theme_dark),
        ),
        theme,
        { setAppTheme(context, it) },
        tall = true,
    )

    SectionTitle(stringResource(R.string.settings_about))
    GroupCard {
        SoftwareUpdate()
        Separator()
        ValueRow(stringResource(R.string.settings_advanced), icon = Icons.Log, chevron = Icons.Chev, onClick = onMore)
    }
    Text(
        stringResource(R.string.settings_footer, stringResource(R.string.app_name), version),
        LinkTheme.phoneType.footnote,
        modifier = Modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p16.dp),
        color = s.label2,
    )
}

/** Rarely changed: notification detail, how Link sees copies, and the log for a bug report. */
@Composable
private fun Advanced(
    prefs: NotificationPrefs,
    clearsSynced: Boolean,
    sendsScreenshots: Boolean,
    logs: Boolean,
    method: Method?,
    onMethod: () -> Unit,
    onFullContent: (Boolean) -> Unit,
    onSendScreenshots: (Boolean) -> Unit,
    onClearSync: (Boolean) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()

    SectionTitle(stringResource(R.string.settings_notifications))
    GroupCard {
        ValueRow(
            stringResource(R.string.settings_quiet_apps),
            icon = Icons.Bell,
            subtitle =
                if (prefs.lowImportance == 0) {
                    stringResource(R.string.settings_quiet_apps_none)
                } else {
                    pluralStringResource(R.plurals.settings_quiet_apps_count, prefs.lowImportance, prefs.lowImportance)
                },
        )
        Separator()
        SwitchRow(
            stringResource(R.string.settings_full_locked),
            prefs.fullWhileLocked,
            onFullContent,
            icon = Icons.Lock,
            subtitle =
                stringResource(
                    if (prefs.fullWhileLocked) R.string.settings_full_locked_on else R.string.settings_full_locked_off,
                ),
        )
    }

    SectionTitle(stringResource(R.string.settings_clipboard))
    GroupCard {
        ValueRow(
            stringResource(R.string.log_method),
            icon = Icons.Clip,
            subtitle = stringResource(R.string.log_method_hint),
            value = stringResource(method?.title() ?: R.string.settings_set_up),
            chevron = Icons.Chev,
            onClick = onMethod,
        )
        Separator()
        ValueRow(
            stringResource(R.string.settings_log_access),
            icon = Icons.Lock,
            subtitle = stringResource(R.string.settings_log_access_hint),
            value = stringResource(if (logs) R.string.settings_on else R.string.settings_off),
        )
        Separator()
        SwitchRow(
            stringResource(R.string.settings_screenshots),
            sendsScreenshots,
            onSendScreenshots,
            icon = Icons.Photo,
            subtitle = stringResource(R.string.settings_screenshots_hint),
        )
        Separator()
        SwitchRow(
            stringResource(R.string.settings_clear_clips),
            clearsSynced,
            onClearSync,
            icon = Icons.Clip,
            subtitle = stringResource(R.string.settings_clear_clips_hint),
        )
    }

    SectionTitle(stringResource(R.string.settings_diagnostics))
    GroupCard {
        ValueRow(stringResource(R.string.settings_log), icon = Icons.Log, subtitle = stringResource(R.string.settings_log_hint)) {
            Pill(stringResource(R.string.settings_export), { scope.launch { shareLog(context) } })
        }
    }
}

internal fun Grant.title(): Int =
    when (this) {
        Grant.NotificationAccess -> R.string.settings_grant_notifications
        Grant.DoNotDisturb -> R.string.settings_grant_dnd
        Grant.PostNotifications -> R.string.settings_grant_post
        Grant.Battery -> R.string.settings_grant_battery
        Grant.Calls -> R.string.settings_grant_calls
        Grant.Bluetooth -> R.string.settings_grant_bluetooth
        Grant.Texts -> R.string.onboarding_texts_title
        Grant.AutoClipboard -> R.string.clipboard_auto_row
    }

private fun Grant.icon(): ImageVector =
    when (this) {
        Grant.NotificationAccess, Grant.PostNotifications -> Icons.Bell
        Grant.DoNotDisturb -> Icons.Dnd
        Grant.Battery -> Icons.Battery
        Grant.Calls, Grant.Texts -> Icons.Call
        Grant.Bluetooth -> Icons.Bt
        Grant.AutoClipboard -> Icons.Clip
    }

/** The last 500 lines this app logged, through the share sheet; an app may read its own log without READ_LOGS. */
private suspend fun shareLog(context: Context) {
    val log =
        withContext(Dispatchers.IO) {
            runCatching {
                val logcat =
                    ProcessBuilder("logcat", "-d", "-v", "time", "-t", "$LOG_LINES", "--pid", "${Process.myPid()}")
                        .redirectErrorStream(true)
                        .start()
                try {
                    logcat.inputStream.bufferedReader().use { it.readText() }
                } finally {
                    logcat.destroy()
                }
            }.getOrElse { context.getString(R.string.settings_log_failed, it.message.orEmpty()) }
                // A share intent travels through Binder, which fails past about 1 MB.
                .takeLast(LOG_CHARS)
        }
    val send =
        Intent(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_SUBJECT, context.getString(R.string.settings_log_subject))
            .putExtra(Intent.EXTRA_TEXT, log)
    context.startActivity(Intent.createChooser(send, context.getString(R.string.settings_log)))
}

private const val LOG_LINES = 500
private const val LOG_CHARS = 200_000
