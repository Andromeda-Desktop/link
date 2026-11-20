package org.andromeda.link.devices

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch
import org.andromeda.link.R
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.update.AppUpdate

/** Settings' update row: checks the newest GitHub release once per app run, then downloads and installs it in place. */
@Suppress("ktlint:standard:function-naming")
@Composable
internal fun SoftwareUpdate() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val update by AppUpdate.state.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { if (update == AppUpdate.State.Idle) AppUpdate.check(context) }
    val check = { scope.launch { AppUpdate.check(context) } }
    val u = update
    val (icon, title) =
        when (u) {
            AppUpdate.State.Idle, AppUpdate.State.Current -> Icons.Check to stringResource(R.string.update_current)
            AppUpdate.State.Checking -> Icons.Refresh to stringResource(R.string.update_checking)
            is AppUpdate.State.Available -> Icons.Down to stringResource(R.string.update_available, u.version)
            is AppUpdate.State.Downloading -> Icons.Down to stringResource(R.string.update_downloading_title)
            AppUpdate.State.Installing -> Icons.Down to stringResource(R.string.update_installing)
            is AppUpdate.State.Failed -> Icons.Alert to stringResource(R.string.update_failed, u.message)
        }
    val (action, onAction) =
        when (u) {
            AppUpdate.State.Idle, AppUpdate.State.Current -> R.string.update_check to { check() }
            is AppUpdate.State.Available -> R.string.update_install to { scope.launch { AppUpdate.install(context, u) } }
            is AppUpdate.State.Failed -> R.string.update_retry to { check() }
            else -> null to null
        }
    val progress = (u as? AppUpdate.State.Downloading)?.progress ?: 0f
    val downloading =
        (u as? AppUpdate.State.Downloading)?.let {
            stringResource(R.string.update_downloading, it.version, (progress * 100).toInt())
        }
    Column {
        ValueRow(
            title,
            icon = icon,
            subtitle = downloading,
        ) {
            if (action != null && onAction != null) Pill(stringResource(action), { onAction() })
        }
        if (u is AppUpdate.State.Downloading) {
            Box(
                Modifier
                    .padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Space.p12.dp)
                    .fillMaxWidth()
                    .height(Tok.Space.p4.dp)
                    .clip(CircleShape)
                    .background(LinkTheme.scheme.fill1),
            ) { Box(Modifier.fillMaxWidth(progress).fillMaxHeight().background(LinkTheme.scheme.accent)) }
        }
    }
}
