package org.andromeda.link.update

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch
import org.andromeda.link.R
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation

/** Checks for a release when the app opens and asks once per new version; Settings' update row shows the progress. */
@Composable
fun UpdatePrompt() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val update by AppUpdate.state.collectAsStateWithLifecycle()
    var dismissed by remember { mutableStateOf(AppUpdate.dismissed(context)) }
    LaunchedEffect(Unit) { if (AppUpdate.state.value == AppUpdate.State.Idle) AppUpdate.check(context) }
    val available = update as? AppUpdate.State.Available ?: return
    if (dismissed == available.version) return
    val later = {
        AppUpdate.dismiss(context, available.version)
        dismissed = available.version
    }
    val s = LinkTheme.scheme
    Dialog(onDismissRequest = later) {
        Column(
            Modifier
                .fillMaxWidth()
                .elevation(Tok.Elevation.e4, Tok.Radius.r28.dp)
                .clip(RoundedCornerShape(Tok.Radius.r28.dp))
                .background(s.view)
                .padding(Tok.Space.p20.dp),
            verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
        ) {
            Text(stringResource(R.string.update_prompt_title, available.version), LinkTheme.phoneType.title3)
            Text(
                stringResource(R.string.update_prompt_text),
                LinkTheme.phoneType.paragraph,
                Modifier.padding(bottom = Tok.Space.p8.dp),
                color = s.label2,
            )
            Capsule(
                stringResource(R.string.update_install),
                { scope.launch { AppUpdate.install(context, available) } },
                Modifier.fillMaxWidth(),
                kind = CapsuleKind.Primary,
            )
            Capsule(stringResource(R.string.onboarding_not_now), later, Modifier.fillMaxWidth())
        }
    }
}
