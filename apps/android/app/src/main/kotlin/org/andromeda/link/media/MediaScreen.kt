package org.andromeda.link.media

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.andromeda.link.R
import org.andromeda.link.core.domain.MediaCommandKind
import org.andromeda.link.core.domain.PlaybackState
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.CapsuleSlider
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation

/** Every connected desktop's players: large art, track, the seek line, the keys, then the desktop and player volume. */
@Composable
fun MediaScreen(
    viewModel: MediaViewModel,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val players by viewModel.players.collectAsStateWithLifecycle()
    val s = LinkTheme.scheme
    Box(modifier) {
        Screen(background = s.solidRegular) {
            BackKey(s.label1, onBack, Modifier.padding(horizontal = Tok.Space.p12.dp))
            if (players.isEmpty()) {
                Empty()
                return@Screen
            }
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Space.p48.dp),
                verticalArrangement = Arrangement.spacedBy(Tok.Space.p48.dp),
            ) {
                items(players, key = { it.desktopId + it.player.player }) { shown ->
                    Player(
                        shown,
                        onCommand = { kind, value -> viewModel.command(shown, kind, value) },
                        onDesktopVolume = { viewModel.setDesktopVolume(shown, it) },
                    )
                }
            }
        }
    }
}

@Composable
private fun Player(
    shown: ShownPlayer,
    onCommand: (MediaCommandKind, Long?) -> Unit,
    onDesktopVolume: (Int) -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val player = shown.player
    Column(Modifier.fillMaxWidth()) {
        Artwork(
            player.artwork,
            Tok.Phone.mediaArt.dp,
            Tok.Radius.r20.dp,
            Modifier
                .padding(top = Tok.Space.p8.dp)
                .align(Alignment.CenterHorizontally)
                .elevation(Tok.Elevation.e2, Tok.Radius.r20.dp),
        )
        Text(
            player.title.ifEmpty { player.name },
            t.title2,
            Modifier.padding(top = Tok.Space.p24.dp),
            maxLines = 2,
        )
        val by = if (player.artist.isEmpty()) player.name else stringResource(R.string.home_on_app, player.artist, player.name)
        Text(by, t.body, Modifier.padding(top = Tok.Space.p2.dp), color = s.label2, maxLines = 1)
        Box(Modifier.padding(top = Tok.Space.p8.dp)) {
            Progress(
                player.positionMs,
                player.lengthMs,
                player.receivedAt,
                player.state == PlaybackState.Playing,
                MediaCommandKind.Seek in player.can,
            ) { onCommand(MediaCommandKind.Seek, it) }
        }
        MediaKeys(player, onCommand, Modifier.padding(top = Tok.Space.p4.dp))
        shown.desktopVolume?.let { volume ->
            Caption(shown.desktopName)
            CapsuleSlider(
                volume.percent,
                Icons.Vol,
                stringResource(R.string.media_desktop_volume),
                true,
                onDesktopVolume,
                Modifier.padding(top = Tok.Space.p4.dp),
            )
        }
        val playerVolume = player.volume
        if (playerVolume != null && MediaCommandKind.Volume in player.can) {
            Caption(player.name)
            CapsuleSlider(
                playerVolume,
                Icons.Music,
                stringResource(R.string.media_player_volume),
                true,
                { onCommand(MediaCommandKind.Volume, it.toLong()) },
                Modifier.padding(top = Tok.Space.p4.dp),
            )
        }
    }
}

@Composable
private fun Caption(text: String) {
    Text(
        text,
        LinkTheme.phoneType.footnote,
        Modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p16.dp),
        color = LinkTheme.scheme.label2,
        maxLines = 1,
    )
}

@Composable
private fun Empty() {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Column(
        Modifier.fillMaxSize().padding(Tok.Space.p32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp, Alignment.CenterVertically),
    ) {
        Symbol(Icons.Music, s.label3, size = Tok.Space.p32.dp)
        Text(stringResource(R.string.media_empty_title), t.headline, align = TextAlign.Center)
        Text(stringResource(R.string.media_empty), t.paragraph, color = s.label2, align = TextAlign.Center)
    }
}
