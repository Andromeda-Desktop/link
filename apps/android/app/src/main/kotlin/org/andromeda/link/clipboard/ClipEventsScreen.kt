package org.andromeda.link.clipboard

import android.text.format.DateUtils
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.andromeda.link.R
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.SectionTitle
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.kit.groupSlice
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

@StringRes
fun CaptureMode.title(): Int =
    when (this) {
        is CaptureMode.Direct -> if (asSystemUi) R.string.clip_mode_direct_system else R.string.clip_mode_direct_shell
        CaptureMode.Denials -> R.string.clip_mode_denials
        is CaptureMode.Off ->
            when (why) {
                OffReason.Stopped -> R.string.clip_mode_off_stopped
                OffReason.NoLogAccess -> R.string.clip_mode_off_no_log
                OffReason.NoOverlay -> R.string.clip_mode_off_no_overlay
                OffReason.Background -> R.string.clip_mode_off_background
                OffReason.ReaderEnded -> R.string.clip_mode_off_reader_ended
            }
    }

@Composable
private fun ClipOutcome.text(): String =
    when (this) {
        is ClipOutcome.Sent -> pluralStringResource(R.plurals.clip_sent, desktops, desktops)
        is ClipOutcome.Dropped -> {
            val why =
                stringResource(
                    when (why) {
                        ClipDrop.NoDesktop -> R.string.clip_drop_no_desktop
                        ClipDrop.SendFailed -> R.string.clip_drop_send_failed
                        ClipDrop.Empty -> R.string.clip_drop_empty
                        ClipDrop.OverlayBusy -> R.string.clip_drop_overlay_busy
                        ClipDrop.OverlayFailed -> R.string.clip_drop_overlay_failed
                        ClipDrop.NoFocus -> R.string.clip_drop_no_focus
                    },
                )
            if (detail.isNullOrEmpty()) why else "$why ($detail)"
        }
    }

/** The capture mode and the last copy outcomes from [ClipEvents], newest first. */
@Composable
fun ClipEventsScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val mode by ClipEvents.mode.collectAsStateWithLifecycle()
    val events by ClipEvents.events.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Box(modifier) {
        Screen(background = s.solidRegular) {
            BackKey(s.label1, onBack, Modifier.padding(horizontal = Tok.Space.p12.dp))
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Space.p48.dp),
            ) {
                item {
                    Text(
                        stringResource(R.string.clip_log),
                        t.large,
                        Modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p8.dp, bottom = Tok.Space.p16.dp),
                        maxLines = 2,
                    )
                    GroupCard {
                        ValueRow(stringResource(R.string.clip_log_mode), icon = Icons.Clip, subtitle = stringResource(mode.title()))
                    }
                    SectionTitle(stringResource(R.string.clip_log_events))
                    if (events.isEmpty()) {
                        GroupCard { ValueRow(stringResource(R.string.clip_log_empty)) }
                    }
                }
                itemsIndexed(events) { i, event ->
                    Column(Modifier.groupSlice(i, events.size)) {
                        if (i > 0) Separator()
                        val sent = event.outcome is ClipOutcome.Sent
                        ValueRow(
                            event.preview ?: stringResource(R.string.clip_log_no_text),
                            icon = if (sent) Icons.Check else Icons.Alert,
                            subtitle = event.outcome.text(),
                            value = DateUtils.formatDateTime(context, event.at, DateUtils.FORMAT_SHOW_TIME),
                        )
                    }
                }
            }
        }
    }
}
