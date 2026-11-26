package org.andromeda.link.notifications

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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.andromeda.link.R
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.SwitchRow
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.groupSlice
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

/** Every launcher app with a switch; all are mirrored until the user turns one off. */
@Composable
fun AppFilterScreen(
    viewModel: AppFilterViewModel,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val apps by viewModel.state.collectAsStateWithLifecycle()
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Box(modifier) {
        Screen(title = stringResource(R.string.mirror_apps_title), onBack = onBack, background = s.solidRegular) {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Space.p48.dp),
            ) {
                item {
                    Text(
                        stringResource(R.string.mirror_apps_hint),
                        t.paragraph,
                        Modifier.padding(start = Tok.Space.p4.dp, top = Tok.Space.p8.dp, bottom = Tok.Space.p16.dp),
                        color = s.label2,
                    )
                }
                itemsIndexed(apps, key = { _, app -> app.packageName }) { i, app ->
                    Column(Modifier.groupSlice(i, apps.size)) {
                        if (i > 0) Separator()
                        SwitchRow(app.label, app.mirrored, { viewModel.setMirrored(app.packageName, it) })
                    }
                }
            }
        }
    }
}
