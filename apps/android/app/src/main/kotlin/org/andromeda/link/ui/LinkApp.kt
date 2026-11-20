package org.andromeda.link.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.IntOffset
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import kotlinx.coroutines.flow.StateFlow
import org.andromeda.link.AppContainer
import org.andromeda.link.control.RemoteMode
import org.andromeda.link.control.RemoteScreen
import org.andromeda.link.control.RemoteViewModel
import org.andromeda.link.control.ViewerScreen
import org.andromeda.link.control.ViewerViewModel
import org.andromeda.link.devices.DeviceScreen
import org.andromeda.link.devices.DeviceViewModel
import org.andromeda.link.devices.HomeScreen
import org.andromeda.link.devices.HomeViewModel
import org.andromeda.link.devices.SettingsScreen
import org.andromeda.link.files.PickerTab
import org.andromeda.link.files.PickerViewModel
import org.andromeda.link.media.MediaScreen
import org.andromeda.link.media.MediaViewModel
import org.andromeda.link.notifications.AppFilterScreen
import org.andromeda.link.notifications.AppFilterViewModel
import org.andromeda.link.onboarding.Grant
import org.andromeda.link.onboarding.OnboardingScreen
import org.andromeda.link.onboarding.OnboardingViewModel
import org.andromeda.link.pairing.PairingScreen
import org.andromeda.link.pairing.PairingViewModel
import org.andromeda.link.transfer.ActivityScreen
import org.andromeda.link.transfer.ActivityViewModel
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.spring

private enum class Screen { Home, Activity, Settings, Device, Pairing, Setup, Media, MirrorApps, Remote, Presenter, Viewer }

private val rootScreens = listOf(Screen.Home, Screen.Activity, Screen.Settings)

/** A handful of screens, so navigation is one piece of state rather than a navigation library. */
@Composable
fun LinkApp(
    container: AppContainer,
    pairingLink: StateFlow<String?>,
    onLinkHandle: () -> Unit,
    modifier: Modifier = Modifier,
    home: HomeViewModel = viewModel { HomeViewModel(container.repository, container.presence, container.mirror, container.clipboard) },
    pairing: PairingViewModel = viewModel { PairingViewModel(container.repository) },
    picker: PickerViewModel = viewModel { PickerViewModel(container.files) },
    onboarding: OnboardingViewModel = viewModel { OnboardingViewModel(container.application, container.mirror, container.ringer) },
) {
    val linkHandle by rememberUpdatedState(onLinkHandle)
    var screen by rememberSaveable { mutableStateOf(Screen.Home) }
    var methodSheet by rememberSaveable { mutableStateOf(false) }
    var returnScreen by rememberSaveable { mutableStateOf(Screen.Home) }

    /** Where Remote's back goes: the screen that opened it. */
    var remoteFrom by rememberSaveable { mutableStateOf(Screen.Home) }
    var desktopId by rememberSaveable { mutableStateOf<String?>(null) }
    var setupStart by rememberSaveable { mutableStateOf<Grant?>(null) }
    val link by pairingLink.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val setup by onboarding.state.collectAsStateWithLifecycle()
    // Grants change in Settings and over adb, so they are rechecked whenever the app returns.
    LifecycleResumeEffect(Unit) {
        onboarding.refresh()
        onPauseOrDispose {}
    }

    LaunchedEffect(link) {
        link?.let {
            pairing.offerLink(it)
            linkHandle()
            returnScreen = if (screen in rootScreens) screen else Screen.Home
            screen = Screen.Pairing
        }
    }
    if (screen != Screen.Home) {
        BackHandler {
            screen =
                when {
                    screen in rootScreens -> Screen.Home
                    screen == Screen.MirrorApps && desktopId != null -> Screen.Device
                    screen == Screen.Remote || screen == Screen.Presenter -> remoteFrom
                    else -> returnScreen
                }
        }
    }
    val openDesktop: (org.andromeda.link.core.domain.Desktop) -> Unit = {
        desktopId = it.id
        returnScreen = screen
        screen = Screen.Device
    }
    val openPair: () -> Unit = {
        pairing.reset()
        returnScreen = screen
        screen = Screen.Pairing
    }
    val openSetup: () -> Unit = {
        returnScreen = screen
        setupStart = null
        screen = Screen.Setup
    }
    val openClipboardSetup: () -> Unit = {
        returnScreen = screen
        setupStart = Grant.AutoClipboard
        screen = Screen.Setup
    }
    val openNotifications: () -> Unit = {
        returnScreen = screen
        desktopId = null
        if (Grant.NotificationAccess in setup.granted) {
            screen = Screen.MirrorApps
        } else {
            setupStart = null
            screen = Screen.Setup
        }
    }
    Column(modifier.fillMaxSize()) {
        Box(Modifier.weight(1f)) {
            AnimatedContent(
                targetState = screen,
                transitionSpec = {
                    val forward = targetState.ordinal > initialState.ordinal
                    val enter =
                        slideInHorizontally(spring<IntOffset>(Tok.Spring.page)) {
                            if (forward) it / 6 else -it / 6
                        } + fadeIn(spring(Tok.Spring.fade))
                    enter togetherWith fadeOut(spring(Tok.Spring.fadeOut)) +
                        slideOutHorizontally(spring<IntOffset>(Tok.Spring.page)) { if (forward) -it / 8 else it / 8 }
                },
                label = "screen",
            ) { current ->
                when (current) {
                    Screen.Home ->
                        HomeScreen(
                            container = container,
                            missing = setup.missing,
                            autoClipboard = Grant.AutoClipboard in setup.granted,
                            onPair = openPair,
                            onDesktop = openDesktop,
                            onMedia = {
                                returnScreen = screen
                                screen = Screen.Media
                            },
                            onFix = {
                                returnScreen = screen
                                setupStart = it
                                screen = Screen.Setup
                            },
                            onClipboardSetup = openClipboardSetup,
                            onActivity = { screen = Screen.Activity },
                            onSettings = { screen = Screen.Settings },
                            onRemote = {
                                desktopId = it.id
                                remoteFrom = Screen.Home
                                screen = Screen.Remote
                            },
                            onPresenter = {
                                desktopId = it.id
                                remoteFrom = Screen.Home
                                screen = Screen.Presenter
                            },
                            onViewer = {
                                desktopId = it.id
                                remoteFrom = Screen.Home
                                screen = Screen.Viewer
                            },
                        )
                    Screen.Activity -> {
                        val desks by home.state.collectAsStateWithLifecycle()
                        ActivityScreen(
                            viewModel { ActivityViewModel(container.repository, container.clipboard, container.clipHistory) },
                            sendTo = desks.primary?.name,
                            showDesktop = desks.desktops.size > 1,
                            onBack = { screen = Screen.Home },
                            onSend = {
                                picker.open(PickerTab.Files)
                                screen = Screen.Home
                            },
                        )
                    }
                    Screen.Settings ->
                        SettingsScreen(
                            container,
                            Grant.AutoClipboard in setup.granted,
                            setup.missing,
                            { screen = Screen.Home },
                            openDesktop,
                            openPair,
                            openNotifications,
                            openSetup,
                            openClipboardSetup,
                            methodSheet,
                            { methodSheet = it },
                        )
                    Screen.Device ->
                        desktopId?.let { id ->
                            DeviceScreen(
                                viewModel(key = "device-$id") { DeviceViewModel(container.repository, id) },
                                onBack = { screen = returnScreen },
                                onMirrorApps = { screen = Screen.MirrorApps },
                                onRemote = {
                                    remoteFrom = Screen.Device
                                    screen = Screen.Remote
                                },
                                onPresenter = {
                                    remoteFrom = Screen.Device
                                    screen = Screen.Presenter
                                },
                                onViewer = {
                                    remoteFrom = Screen.Device
                                    screen = Screen.Viewer
                                },
                            )
                        }
                    Screen.Remote, Screen.Presenter ->
                        desktopId?.let { id ->
                            RemoteScreen(
                                viewModel(key = "remote-$id") { RemoteViewModel(container.repository, id) },
                                start = if (current == Screen.Remote) RemoteMode.Pad else RemoteMode.Present,
                                onBack = { screen = remoteFrom },
                                onScreen = { screen = Screen.Viewer },
                            )
                        }
                    Screen.Viewer ->
                        desktopId?.let { id ->
                            ViewerScreen(
                                viewModel(key = "viewer-$id") { ViewerViewModel(container.repository, id) },
                                viewModel(key = "remote-$id") { RemoteViewModel(container.repository, id) },
                                onBack = { screen = Screen.Remote },
                            )
                        }
                    Screen.Pairing ->
                        PairingScreen(
                            container = container,
                            onBack = { screen = returnScreen },
                            onPair = { desktop ->
                                container.presence.paired()
                                home.announce(desktop)
                                if (setup.complete) {
                                    screen = returnScreen
                                } else {
                                    setupStart = null
                                    screen = Screen.Setup
                                }
                            },
                        )
                    Screen.Setup ->
                        OnboardingScreen(
                            container,
                            container.mirror.accessSettings(),
                            start = setupStart,
                            onDone = { screen = returnScreen },
                        )
                    Screen.Media ->
                        MediaScreen(
                            viewModel { MediaViewModel(container.repository) },
                            onBack = { screen = returnScreen },
                        )
                    Screen.MirrorApps ->
                        AppFilterScreen(
                            viewModel { AppFilterViewModel(container.mirror, context.packageManager) },
                            onBack = { screen = if (desktopId != null) Screen.Device else returnScreen },
                        )
                }
            }
        }
    }
}
