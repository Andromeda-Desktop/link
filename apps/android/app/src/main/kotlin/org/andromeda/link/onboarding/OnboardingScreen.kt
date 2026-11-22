package org.andromeda.link.onboarding

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.andromeda.link.AppContainer
import org.andromeda.link.R
import org.andromeda.link.clipboard.LogAccess
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

private data class Page(
    val grant: Grant,
    val title: Int,
    val body: Int,
    val icon: ImageVector,
    val points: List<Int>,
    /** What Android shows when the user allows it. */
    val foot: Int,
)

private val PAGES =
    listOf(
        Page(
            Grant.NotificationAccess,
            R.string.onboarding_notifications_title,
            R.string.onboarding_notifications_body,
            Icons.Bell,
            listOf(R.string.onboarding_notifications_p1, R.string.onboarding_notifications_p2),
            R.string.onboarding_foot_notifications,
        ),
        Page(
            Grant.DoNotDisturb,
            R.string.onboarding_dnd_title,
            R.string.onboarding_dnd_body,
            Icons.Dnd,
            listOf(R.string.onboarding_dnd_p1, R.string.onboarding_dnd_p2),
            R.string.onboarding_foot_dnd,
        ),
        Page(
            Grant.PostNotifications,
            R.string.onboarding_post_title,
            R.string.onboarding_post_body,
            Icons.Bell,
            emptyList(),
            R.string.onboarding_foot_ask,
        ),
        Page(
            Grant.Battery,
            R.string.onboarding_battery_title,
            R.string.onboarding_battery_body,
            Icons.Battery,
            listOf(R.string.onboarding_battery_p1),
            R.string.onboarding_foot_battery,
        ),
        Page(
            Grant.Calls,
            R.string.onboarding_calls_title,
            R.string.onboarding_calls_body,
            Icons.Call,
            listOf(R.string.onboarding_calls_p1, R.string.onboarding_calls_p2),
            R.string.onboarding_foot_ask,
        ),
        Page(
            Grant.Bluetooth,
            R.string.onboarding_bluetooth_title,
            R.string.onboarding_bluetooth_body,
            Icons.Bt,
            listOf(R.string.onboarding_bluetooth_p1, R.string.onboarding_bluetooth_p2),
            R.string.onboarding_foot_ask,
        ),
        Page(
            Grant.Texts,
            R.string.onboarding_texts_title,
            R.string.onboarding_texts_body,
            Icons.Msg,
            listOf(R.string.onboarding_texts_p1, R.string.onboarding_texts_p2),
            R.string.onboarding_foot_ask,
        ),
        Page(
            Grant.Photos,
            R.string.onboarding_photos_title,
            R.string.onboarding_photos_body,
            Icons.Photo,
            listOf(R.string.onboarding_photos_p1, R.string.onboarding_photos_p2),
            R.string.onboarding_foot_ask,
        ),
        Page(
            Grant.AutoClipboard,
            R.string.onboarding_clipboard_title,
            R.string.onboarding_clipboard_body,
            Icons.Clip,
            emptyList(),
            R.string.onboarding_foot_clipboard,
        ),
    )

/** A short paged flow through the grants, each explained before Android's own screen asks. */
@Composable
fun OnboardingScreen(
    container: AppContainer,
    mirrorSettings: Intent,
    start: Grant?,
    onDone: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: OnboardingViewModel =
        viewModel { OnboardingViewModel(container.application, container.mirror, container.ringer, container.files) },
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val pager = rememberPagerState(PAGES.indexOfFirst { it.grant == start }.coerceAtLeast(0)) { PAGES.size }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) viewModel.refresh() }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { viewModel.refresh() }
    val last = pager.currentPage == PAGES.lastIndex
    // The scroll runs in the screen's scope: an effect keyed on currentPage would cancel it halfway, where currentPage
    // changes, and leave the pager between two pages. It counts from the settled page, so a tap during a scroll does not
    // skip one.
    val next = {
        val from = pager.settledPage
        if (from == PAGES.lastIndex) onDone() else scope.launch { pager.animateScrollToPage(from + 1) }
    }
    // A grant given on its page moves on by itself, once the user has seen it turn granted.
    val currentGranted = PAGES[pager.currentPage].grant in state.granted
    var seenUngranted by remember(pager.currentPage) { mutableStateOf(!currentGranted) }
    LaunchedEffect(pager.currentPage, currentGranted) {
        if (!currentGranted) {
            seenUngranted = true
        } else if (seenUngranted && !last) {
            delay(ADVANCE_MS)
            if (!pager.isScrollInProgress) next()
        }
    }
    Box(modifier) {
        Screen(background = LinkTheme.scheme.solidRegular) {
            Row(
                Modifier.fillMaxWidth().padding(start = Tok.Space.p16.dp, end = Tok.Space.p4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Steps(pager.currentPage, Modifier.weight(1f))
                Box(
                    Modifier
                        .height(Tok.Phone.hit.dp)
                        .clip(CircleShape)
                        .clickable(role = Role.Button) { next() }
                        .padding(horizontal = Tok.Space.p12.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        stringResource(R.string.onboarding_not_now),
                        LinkTheme.phoneType.body.copy(fontWeight = FontWeight.SemiBold),
                        color = LinkTheme.scheme.accentText,
                        maxLines = 1,
                    )
                }
            }
            HorizontalPager(pager, Modifier.weight(1f).fillMaxWidth()) { index ->
                val page = PAGES[index]
                val granted = page.grant in state.granted
                val clipboardSteps = page.grant == Grant.AutoClipboard && !granted
                PageContent(
                    page,
                    granted,
                    steps =
                        if (clipboardSteps) {
                            {
                                ClipboardSteps(viewModel.adbCommand(), viewModel::overlayAllowed, viewModel::logsAllowed) {
                                    viewModel.settings(Grant.AutoClipboard, mirrorSettings)?.let(context::startActivity)
                                }
                            }
                        } else {
                            null
                        },
                ) {
                    when {
                        clipboardSteps -> {}
                        granted ->
                            Capsule(
                                stringResource(if (index == PAGES.lastIndex) R.string.onboarding_done else R.string.onboarding_next),
                                { next() },
                                Modifier.fillMaxWidth(),
                                kind = CapsuleKind.Primary,
                                large = true,
                            )
                        else ->
                            Capsule(
                                stringResource(R.string.onboarding_allow),
                                {
                                    val permissions = viewModel.permissions(page.grant)
                                    if (permissions.isNotEmpty()) {
                                        request.launch(permissions)
                                    } else {
                                        viewModel.settings(page.grant, mirrorSettings)?.let(context::startActivity)
                                    }
                                },
                                Modifier.fillMaxWidth(),
                                kind = CapsuleKind.Primary,
                                large = true,
                            )
                    }
                }
            }
        }
    }
}

/** The step track: one bar per page, the current one accent and wider. */
@Composable
private fun Steps(
    current: Int,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val description = stringResource(R.string.onboarding_step, current + 1, PAGES.size)
    Row(modifier.semantics { contentDescription = description }, horizontalArrangement = Arrangement.spacedBy(Tok.Space.p4.dp)) {
        repeat(PAGES.size) {
            Box(
                Modifier
                    .width(if (it == current) Tok.Space.p24.dp else Tok.Space.p16.dp)
                    .height(Tok.Space.p4.dp)
                    .background(if (it == current) s.accent else s.fill2, CircleShape),
            )
        }
    }
}

@Composable
private fun PageContent(
    page: Page,
    granted: Boolean,
    steps: (@Composable () -> Unit)?,
    action: @Composable () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    // The hero sits in the middle when the page fits; a page with more to say, like the clipboard one, scrolls.
    BoxWithConstraints(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .heightIn(min = maxHeight)
                .navigationBarsPadding()
                .padding(horizontal = Tok.Space.p16.dp)
                .padding(bottom = Tok.Space.p12.dp),
            verticalArrangement = Arrangement.SpaceBetween,
        ) {
            Box(Modifier)
            Column(verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
                val tint = if (granted) s.success else s.accent
                Box(
                    Modifier
                        .padding(bottom = Tok.Space.p16.dp)
                        .size(Tok.Phone.heroGlyph.dp)
                        .clip(RoundedCornerShape(Tok.Radius.r24.dp))
                        .background(tint.copy(alpha = Tok.Phone.glyphTint)),
                    contentAlignment = Alignment.Center,
                ) {
                    Symbol(
                        if (granted) Icons.Check else page.icon,
                        if (granted) s.success else s.accentText,
                        size = Tok.Phone.heroSymbol.dp,
                    )
                }
                Text(stringResource(page.title), t.large)
                Text(stringResource(page.body), t.paragraph, color = s.label2)
                Column(Modifier.padding(top = Tok.Space.p8.dp), verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
                    for (point in page.points) {
                        Row(horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp)) {
                            Symbol(Icons.Check, s.success, Modifier.padding(top = Tok.Space.p2.dp), size = Tok.Size.symbol.dp)
                            Text(stringResource(point), t.body.copy(lineHeight = t.paragraph.lineHeight))
                        }
                    }
                }
                if (steps != null) Box(Modifier.padding(top = Tok.Space.p16.dp)) { steps() }
            }
            Column(verticalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp)) {
                Text(stringResource(page.foot), t.footnote, modifier = Modifier.fillMaxWidth(), color = s.label2, align = TextAlign.Center)
                action()
            }
        }
    }
}

private const val ADVANCE_MS = 700L

/**
 * Automatic clipboard's two grants as two steps, each with its own way to give it: the overlay in Settings, and log
 * access from a computer. Rechecked on every return to the app, since both are given outside it.
 */
@Composable
private fun ClipboardSteps(
    command: String,
    overlayAllowed: () -> Boolean,
    logsAllowed: () -> Boolean,
    onOverlay: () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val context = LocalContext.current
    var overlay by remember { mutableStateOf(overlayAllowed()) }
    var logs by remember { mutableStateOf(logsAllowed()) }
    val overlayNow by rememberUpdatedState(overlayAllowed)
    val logsNow by rememberUpdatedState(logsAllowed)
    LifecycleResumeEffect(Unit) {
        overlay = overlayNow()
        logs = logsNow()
        onPauseOrDispose {}
    }
    val inset = Modifier.padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Space.p12.dp)
    GroupCard {
        Step(stringResource(R.string.onboarding_clipboard_step_overlay), overlay) {
            Pill(stringResource(R.string.onboarding_clipboard_open), onOverlay)
        }
        if (!overlay) {
            Column(inset, verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
                Text(stringResource(R.string.onboarding_clipboard_restricted), t.footnote, color = s.label2)
                Pill(stringResource(R.string.onboarding_clipboard_app_info), {
                    context.startActivity(
                        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    )
                })
            }
        }
        Separator()
        Step(stringResource(R.string.onboarding_clipboard_step_logs), logs) {
            if (LogAccess.shizukuAsks()) Pill(stringResource(R.string.onboarding_clipboard_shizuku), LogAccess::askShizuku)
        }
        if (!logs) {
            Column(inset, verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
                Text(
                    command,
                    t.mono,
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(Tok.Radius.r12.dp))
                        .background(s.fill1)
                        .padding(Tok.Space.p12.dp),
                )
                Pill(stringResource(R.string.onboarding_clipboard_copy), {
                    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("adb", command))
                })
                Text(stringResource(R.string.onboarding_clipboard_note), t.footnote, color = s.label2)
            }
        }
    }
}

/** One grant: done or not, and while not, the [action] that gives it. */
@Composable
private fun Step(
    label: String,
    done: Boolean,
    action: @Composable () -> Unit,
) {
    ValueRow(label, icon = if (done) Icons.Check else Icons.Alert) { if (!done) action() }
}
