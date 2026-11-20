package org.andromeda.link.share

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.IntentCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import org.andromeda.link.LinkApplication
import org.andromeda.link.R
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Sheet
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.LinkTheme

/** The share target for text, links, and files from any app. It shows only a chooser or progress, then a toast. */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val payload = payload(intent)
        if (payload == null) {
            finish()
            return
        }
        val repository = (application as LinkApplication).container.repository
        setContent {
            LinkTheme {
                val viewModel = viewModel { ShareViewModel(repository, payload) }
                ShareScreen(viewModel, onDone = { message ->
                    message?.let { Toast.makeText(this, it, Toast.LENGTH_LONG).show() }
                    finish()
                })
            }
        }
    }
}

private fun payload(intent: Intent): SharePayload? {
    val uris =
        when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> return null
        }
    if (uris.isNotEmpty()) return SharePayload.Files(uris.map(Uri::toString))
    val text = intent.getStringExtra(Intent.EXTRA_TEXT)
    return if (intent.action == Intent.ACTION_SEND && !text.isNullOrEmpty()) SharePayload.Text(text) else null
}

@Composable
private fun ShareScreen(
    viewModel: ShareViewModel,
    onDone: (String?) -> Unit,
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val resources = LocalContext.current.resources
    val done by rememberUpdatedState(onDone)
    LaunchedEffect(viewModel) {
        viewModel.outcome.collect { outcome ->
            done(
                when (outcome) {
                    is ShareOutcome.Sent ->
                        resources.getString(
                            when (outcome.how) {
                                SentAs.Delivered -> R.string.share_sent
                                SentAs.Queued -> R.string.share_queued
                                SentAs.Files -> R.string.share_files_sending
                            },
                            outcome.desktop.name,
                        )
                    ShareOutcome.NoDesktop -> resources.getString(R.string.share_no_desktop)
                    ShareOutcome.Cancelled -> null
                    is ShareOutcome.Failed -> resources.getString(R.string.share_failed, outcome.failure.text(resources))
                },
            )
        }
    }
    Box(Modifier.fillMaxSize()) {
        when (val current = state) {
            ShareState.Loading -> Sheet(true, viewModel::cancel, stringResource(R.string.share_preparing)) {}
            is ShareState.Sending -> Sheet(true, viewModel::cancel, stringResource(R.string.share_sending, current.desktop.name)) {}
            is ShareState.Choose ->
                Sheet(true, viewModel::cancel, stringResource(R.string.share_choose)) {
                    current.desktops.forEach { desktop ->
                        Capsule(desktop.name, { viewModel.send(desktop) }, Modifier.fillMaxWidth(), icon = Icons.Laptop, large = true)
                    }
                    Capsule(stringResource(R.string.cancel), viewModel::cancel, Modifier.fillMaxWidth(), large = true)
                }
        }
    }
}
