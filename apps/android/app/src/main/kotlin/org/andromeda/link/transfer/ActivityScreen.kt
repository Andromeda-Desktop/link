package org.andromeda.link.transfer

import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.Intent
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.andromeda.link.R
import org.andromeda.link.clipboard.ClipEntry
import org.andromeda.link.core.domain.TransferRecord
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.FilterChips
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.PillKind
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.SectionTitle
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import java.net.URLConnection

private enum class Filter { All, Received, Sent }

/** One row of the history: a transfer or a clip, in either direction. */
private sealed interface Entry {
    val time: Long
    val incoming: Boolean

    data class Move(
        val record: TransferRecord,
    ) : Entry {
        override val time get() = record.time
        override val incoming get() = record.incoming == true
    }

    data class Clip(
        val clip: ClipEntry,
    ) : Entry {
        override val time get() = clip.time
        override val incoming get() = clip.incoming
    }
}

/**
 * The transfer and clip history by day, newest first. [sendTo] names the desktop the empty state offers to send to;
 * [showDesktop] adds the desktop name to each line when more than one desktop is paired.
 */
@Composable
fun ActivityScreen(
    viewModel: ActivityViewModel,
    sendTo: String?,
    showDesktop: Boolean,
    onBack: () -> Unit,
    onSend: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val transfers by viewModel.transfers.collectAsStateWithLifecycle()
    val clips by viewModel.clips.collectAsStateWithLifecycle()
    val busy by viewModel.busy.collectAsStateWithLifecycle()
    val message by viewModel.message.collectAsStateWithLifecycle()
    val context = LocalContext.current
    var filter by rememberSaveable { mutableStateOf(Filter.All) }
    var openFailure by remember { mutableStateOf<String?>(null) }
    val open: (() -> Intent?) -> Unit = { intent ->
        try {
            intent()?.let(context::startActivity) ?: run { openFailure = context.getString(R.string.activity_file_gone) }
        } catch (_: ActivityNotFoundException) {
            openFailure = context.getString(R.string.activity_no_file_app)
        } catch (error: SecurityException) {
            openFailure = context.getString(R.string.failure_other, error.message.orEmpty())
        }
    }
    val toast =
        openFailure ?: when (val shown = message) {
            is ActivityMessage.Failed -> shown.failure.text(context.resources)
            ActivityMessage.Expired -> stringResource(R.string.activity_expired)
            null -> null
        }
    val all =
        remember(transfers, clips) {
            (transfers.map(Entry::Move) + clips.map(Entry::Clip)).sortedByDescending { it.time }
        }
    val days =
        remember(all, filter) {
            all
                .filter {
                    when (filter) {
                        Filter.All -> true
                        Filter.Received -> it.incoming
                        Filter.Sent -> !it.incoming
                    }
                }.groupBy { day(it.time) }
        }
    val filters =
        listOf(
            Filter.All to stringResource(R.string.activity_filter_all),
            Filter.Received to stringResource(R.string.activity_filter_received),
            Filter.Sent to stringResource(R.string.activity_filter_sent),
        )
    val gutter = Modifier.padding(horizontal = Tok.Space.p16.dp)
    val header: @Composable () -> Unit = {
        Column {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = Tok.Space.p12.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                BackKey(LinkTheme.scheme.label1, onBack)
                FolderKey { open { Intent(DownloadManager.ACTION_VIEW_DOWNLOADS) } }
            }
            Text(
                stringResource(R.string.activity_title),
                LinkTheme.phoneType.large,
                modifier = gutter.padding(vertical = Tok.Space.p4.dp),
                maxLines = 1,
            )
            FilterChips(filters, filter, { filter = it }, Modifier.padding(vertical = Tok.Space.p4.dp))
        }
    }
    Box(modifier) {
        Screen(
            toast = toast,
            onToastEnd = {
                openFailure = null
                viewModel.messageShown()
            },
            background = LinkTheme.scheme.solidRegular,
        ) {
            if (all.isEmpty()) {
                header()
                Empty(sendTo, onSend, Modifier.weight(1f))
                return@Screen
            }
            LazyColumn(Modifier.fillMaxSize()) {
                item(key = "header") { header() }
                if (days.isEmpty()) {
                    item(key = "none") {
                        Text(
                            stringResource(if (filter == Filter.Sent) R.string.activity_none_sent else R.string.activity_none_received),
                            LinkTheme.phoneType.footnote,
                            color = LinkTheme.scheme.label2,
                            modifier = gutter.padding(start = Tok.Space.p4.dp, top = Tok.Space.p32.dp),
                        )
                    }
                }
                days.forEach { (day, entries) ->
                    item(key = "day-$day") { SectionTitle(dayLabel(day), gutter) }
                    items(entries, key = { it.key() }) { entry ->
                        Box(gutter.padding(bottom = Tok.Space.p8.dp)) {
                            when (entry) {
                                is Entry.Move ->
                                    MoveCard(
                                        entry.record,
                                        showDesktop,
                                        entry.record.transferId in busy,
                                        { viewModel.accept(entry.record.transferId) },
                                        { viewModel.decline(entry.record.transferId) },
                                        { viewModel.cancel(entry.record.transferId) },
                                    ) { uri -> open { viewIntent(context, uri) } }
                                is Entry.Clip -> ClipCard(entry.clip, showDesktop) { viewModel.copyAgain(entry.clip) }
                            }
                        }
                    }
                }
                item(key = "foot") {
                    Text(
                        stringResource(R.string.activity_kept_hint),
                        LinkTheme.phoneType.footnote,
                        color = LinkTheme.scheme.label2,
                        modifier =
                            gutter
                                .padding(
                                    start = Tok.Space.p4.dp,
                                    top = Tok.Space.p8.dp,
                                ).navigationBarsPadding()
                                .padding(bottom = Tok.Space.p16.dp),
                    )
                }
            }
        }
    }
}

private fun Entry.key(): String =
    when (this) {
        is Entry.Move -> record.transferId
        is Entry.Clip -> "clip-${clip.time}"
    }

@Composable
private fun FolderKey(onClick: () -> Unit) {
    val label = stringResource(R.string.activity_downloads)
    Box(
        Modifier
            .size(Tok.Phone.hit.dp)
            .clip(CircleShape)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Symbol(Icons.Folder, LinkTheme.scheme.label1, size = Tok.Phone.symbol.dp) }
}

@Composable
private fun Empty(
    sendTo: String?,
    onSend: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    Column(
        modifier.fillMaxWidth().padding(horizontal = Tok.Space.p16.dp, vertical = Tok.Space.p32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp, Alignment.CenterVertically),
    ) {
        Box(
            Modifier
                .padding(bottom = Tok.Space.p8.dp)
                .size(Tok.Space.p64.dp)
                .clip(CircleShape)
                .background(s.fill1),
            contentAlignment = Alignment.Center,
        ) { Symbol(Icons.Swap, s.label2, size = Tok.Space.p32.dp) }
        Text(stringResource(R.string.activity_empty_title), LinkTheme.phoneType.title3, align = TextAlign.Center)
        Text(
            if (sendTo != null) stringResource(R.string.activity_empty_hint, sendTo) else stringResource(R.string.activity_empty_hint_none),
            LinkTheme.phoneType.paragraph,
            color = s.label2,
            align = TextAlign.Center,
        )
        if (sendTo != null) {
            Capsule(
                stringResource(R.string.activity_send_to, sendTo),
                onSend,
                Modifier.padding(top = Tok.Space.p12.dp),
                icon = Icons.Send,
                kind = CapsuleKind.Primary,
            )
        }
    }
}

/** A view card r16 with a stroke ring; the direction symbol sits on the first line, actions at the trailing edge. */
@Composable
private fun EntryCard(
    symbol: ImageVector,
    trailing: @Composable () -> Unit,
    below: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    val s = LinkTheme.scheme
    val shape = RoundedCornerShape(Tok.Radius.r16.dp)
    Column(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(s.view)
            .border(Dp.Hairline, s.stroke, shape)
            .padding(start = Tok.Space.p12.dp, end = Tok.Space.p4.dp, top = Tok.Space.p12.dp, bottom = Tok.Space.p12.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp)) {
            Symbol(
                symbol,
                s.label2,
                size = Tok.Size.symbolL.dp,
                modifier = Modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p2.dp),
            )
            Column(Modifier.weight(1f)) { content() }
            Row(Modifier.offset(y = -Tok.Space.p4.dp), verticalAlignment = Alignment.CenterVertically) { trailing() }
        }
        Column(Modifier.padding(start = INDENT, end = Tok.Space.p12.dp)) { below() }
    }
}

/** The indent of lines under an entry's text: the symbol's inset, the symbol and the gap. */
private val INDENT = (Tok.Space.p4 + Tok.Size.symbolL + Tok.Space.p12).dp

@Composable
private fun Dismiss(
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
    ) { Symbol(Icons.X, LinkTheme.scheme.label2, size = Tok.Size.symbolL.dp) }
}

private data class FileRow(
    val name: String,
    val size: Long?,
    val uri: String?,
)

@Composable
private fun MoveCard(
    transfer: TransferRecord,
    showDesktop: Boolean,
    busy: Boolean,
    onAccept: () -> Unit,
    onDecline: () -> Unit,
    onCancel: () -> Unit,
    onOpen: (String) -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val context = LocalContext.current
    val incoming = transfer.incoming == true
    var expanded by rememberSaveable(transfer.transferId) { mutableStateOf(false) }
    // A received file opens once it is published; a sent one from where it was read.
    val rows =
        if (incoming && transfer.saved.isNotEmpty()) {
            transfer.saved.map { saved -> FileRow(saved.name, transfer.files.firstOrNull { it.name == saved.name }?.size, saved.uri) }
        } else {
            transfer.files.map { FileRow(it.name, it.size, if (incoming) null else it.uri) }
        }
    val single = rows.size == 1
    val photos = rows.isNotEmpty() && rows.all { URLConnection.guessContentTypeFromName(it.name)?.startsWith("image/") == true }
    val size = transfer.total.takeIf { it > 0 }?.let { Formatter.formatShortFileSize(context, it) }
    val fraction = if (transfer.total <= 0) 0f else (transfer.bytes.toDouble() / transfer.total).toFloat().coerceIn(0f, 1f)
    val meta =
        listOfNotNull(
            if (transfer.status == "transferring" && size != null) {
                stringResource(R.string.activity_moving, (fraction * 100).toInt(), size)
            } else {
                size
            },
            DateUtils.formatDateTime(context, transfer.time, DateUtils.FORMAT_SHOW_TIME),
        ).joinToString(" · ")
    val desktop = transfer.desktopName.ifEmpty { stringResource(R.string.app_name) }
    val openUri = rows.singleOrNull()?.uri?.takeIf { transfer.status == "done" }
    EntryCard(
        if (incoming) Icons.Down else Icons.Up,
        trailing = {
            if (!busy) {
                when (transfer.status) {
                    "offered" -> {
                        Pill(stringResource(R.string.activity_save), onAccept, kind = PillKind.Primary)
                        Dismiss(stringResource(R.string.transfer_decline), onDecline)
                    }
                    "transferring" -> Dismiss(stringResource(R.string.cancel), onCancel)
                    else -> if (openUri != null) Pill(stringResource(R.string.activity_open), { onOpen(openUri) })
                }
            }
        },
        below = {
            when (transfer.status) {
                "transferring" ->
                    Box(
                        Modifier
                            .padding(vertical = Tok.Space.p4.dp)
                            .fillMaxWidth()
                            .height(Tok.Space.p4.dp)
                            .clip(CircleShape)
                            .background(s.fill2)
                            .semantics { progressBarRangeInfo = ProgressBarRangeInfo(fraction, 0f..1f) },
                    ) { Box(Modifier.fillMaxWidth(fraction).height(Tok.Space.p4.dp).background(s.accent)) }
                "declined", "cancelled" ->
                    Text(
                        stringResource(if (transfer.status == "declined") R.string.activity_declined else R.string.activity_cancelled),
                        t.footnote,
                        color = s.label2,
                    )
                "offered", "done" -> {}
                else -> Failure(transfer.status)
            }
            if (!single) {
                if (expanded) {
                    rows.forEach { row -> FileLine(row, onOpen) }
                } else {
                    Row(Modifier.padding(top = Tok.Space.p4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(summary(rows), t.footnote, color = s.label2, modifier = Modifier.weight(1f), maxLines = 2)
                        if (transfer.status == "done") Pill(stringResource(R.string.activity_show_all), { expanded = true })
                    }
                }
            }
        },
    ) {
        val name =
            when {
                single -> rows[0].name
                photos -> pluralStringResource(R.plurals.activity_photos, rows.size, rows.size)
                else -> pluralStringResource(R.plurals.activity_files, rows.size, rows.size)
            }
        Text(name, t.headline, maxLines = 2)
        Line(meta, desktop, incoming, showDesktop)
    }
}

/** The names of a multi-file transfer on two lines; the rest as a count. */
@Composable
private fun summary(rows: List<FileRow>): String {
    val shown = rows.take(SUMMARY_NAMES).joinToString(", ") { it.name }
    val rest = rows.size - SUMMARY_NAMES
    return if (rest > 0) pluralStringResource(R.plurals.activity_and_more, rest, shown, rest) else shown
}

@Composable
private fun Line(
    meta: String,
    desktop: String,
    incoming: Boolean,
    showDesktop: Boolean,
) {
    val text =
        if (showDesktop) {
            stringResource(if (incoming) R.string.activity_from else R.string.activity_to, desktop) + " · " + meta
        } else {
            meta
        }
    Text(text, LinkTheme.phoneType.footnote, color = LinkTheme.scheme.label2, maxLines = 1)
}

@Composable
private fun Failure(status: String) {
    val s = LinkTheme.scheme
    val why =
        when (status) {
            "too-large" -> R.string.activity_too_large
            "no-space" -> R.string.activity_no_space
            "busy" -> R.string.activity_busy
            else -> R.string.activity_failed
        }
    Row(Modifier.padding(top = Tok.Space.p4.dp), horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
        Symbol(Icons.Alert, s.dangerText, size = Tok.Size.symbol.dp, modifier = Modifier.padding(top = Tok.Space.p2.dp))
        Text(stringResource(why), LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold), color = s.dangerText)
    }
}

@Composable
private fun FileLine(
    row: FileRow,
    onOpen: (String) -> Unit,
) {
    val context = LocalContext.current
    val uri = row.uri
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(row.name, LinkTheme.phoneType.body, maxLines = 1)
            row.size?.let {
                Text(
                    Formatter.formatShortFileSize(context, it),
                    LinkTheme.phoneType.footnote,
                    color = LinkTheme.scheme.label2,
                )
            }
        }
        if (uri != null) Pill(stringResource(R.string.activity_open), { onOpen(uri) })
    }
}

@Composable
private fun ClipCard(
    clip: ClipEntry,
    showDesktop: Boolean,
    onCopy: () -> Unit,
) {
    val context = LocalContext.current
    EntryCard(
        Icons.Clip,
        trailing = { if (clip.text != null) Pill(stringResource(R.string.activity_copy), onCopy) },
        below = {},
    ) {
        Text(clip.text ?: stringResource(R.string.activity_clip_other, clip.mime), LinkTheme.phoneType.paragraph, maxLines = 3)
        Line(
            DateUtils.formatDateTime(context, clip.time, DateUtils.FORMAT_SHOW_TIME),
            clip.desktopName ?: stringResource(R.string.app_name),
            clip.incoming,
            showDesktop,
        )
    }
}

@Composable
private fun dayLabel(day: Long): String =
    when (day) {
        day(System.currentTimeMillis()) -> stringResource(R.string.activity_today)
        day(System.currentTimeMillis() - DateUtils.DAY_IN_MILLIS) -> stringResource(R.string.activity_yesterday)
        else -> DateUtils.formatDateTime(LocalContext.current, day, DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_WEEKDAY)
    }

/** Local midnight of [time], so a day's entries group together. */
private fun day(time: Long): Long =
    java.util.Calendar.getInstance().run {
        timeInMillis = time
        set(java.util.Calendar.HOUR_OF_DAY, 0)
        set(java.util.Calendar.MINUTE, 0)
        set(java.util.Calendar.SECOND, 0)
        set(java.util.Calendar.MILLISECOND, 0)
        timeInMillis
    }

private const val SUMMARY_NAMES = 12
