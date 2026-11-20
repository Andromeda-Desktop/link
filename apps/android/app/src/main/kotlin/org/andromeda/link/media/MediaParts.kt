package org.andromeda.link.media

import android.graphics.BitmapFactory
import android.os.SystemClock
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.andromeda.link.R
import org.andromeda.link.core.domain.MediaCommandKind
import org.andromeda.link.core.domain.MediaPlayer
import org.andromeda.link.core.domain.PlaybackState
import org.andromeda.link.ui.kit.DragTrack
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

/** Previous, play / pause in a label_1 disc, and next, centred. */
@Composable
internal fun MediaKeys(
    player: MediaPlayer,
    onCommand: (MediaCommandKind, Long?) -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val playing = player.state == PlaybackState.Playing
    Row(
        modifier.fillMaxWidth(),
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

/** The track's art, cropped to a square of [size]; a gradient without art. */
@Composable
internal fun Artwork(
    bytes: ByteArray?,
    size: Dp,
    radius: Dp,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    // The last image stays up while the next one decodes.
    val image =
        produceState<ImageBitmap?>(null, bytes) {
            value = withContext(Dispatchers.Default) { bytes?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }?.asImageBitmap() }
        }.value
    Box(
        modifier
            .size(size)
            .clip(RoundedCornerShape(radius))
            .background(Brush.linearGradient(listOf(s.accent, s.fill2))),
    ) {
        if (image != null) {
            Image(image, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        }
    }
}

/** A line that shows how far the track is; a tap or drag seeks, once the finger lifts. */
@Composable
internal fun Progress(
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
internal fun clock(ms: Long): String {
    val sec = ms.coerceAtLeast(0) / 1_000
    return if (sec >= 3_600) {
        "%d:%02d:%02d".format(sec / 3_600, sec / 60 % 60, sec % 60)
    } else {
        "%d:%02d".format(sec / 60, sec % 60)
    }
}
