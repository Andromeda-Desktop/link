package org.andromeda.link.transfer

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.yield
import org.andromeda.link.LinkApplication
import org.andromeda.link.R
import org.andromeda.link.notifications.TransferNotifier
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.Sheet
import org.andromeda.link.ui.theme.LinkTheme

/** A desktop's offer, opened from its notification: the same Accept and Decline, for when Android hides the buttons. */
class OfferActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val transferId = intent.getStringExtra(EXTRA_TRANSFER) ?: return finish()
        val title = intent.getStringExtra(EXTRA_TITLE).orEmpty()
        val repository = (application as LinkApplication).container.repository
        val answer = { accept: Boolean ->
            NotificationManagerCompat.from(this).cancel(TransferNotifier.TAG, transferId.hashCode())
            // Finished only after the answer, so the transfer service starts while this is still in the foreground.
            lifecycleScope.launch {
                if (accept) repository.acceptTransfer(transferId) else repository.declineTransfer(transferId)
                yield()
                finish()
            }
        }
        setContent {
            LinkTheme {
                Box(Modifier.fillMaxSize()) {
                    Sheet(true, ::finish, title) {
                        Capsule(
                            stringResource(R.string.transfer_accept),
                            { answer(true) },
                            Modifier.fillMaxWidth(),
                            kind = CapsuleKind.Primary,
                            large = true,
                        )
                        Capsule(stringResource(R.string.transfer_decline), { answer(false) }, Modifier.fillMaxWidth(), large = true)
                    }
                }
            }
        }
    }

    companion object {
        const val EXTRA_TRANSFER = "transfer"
        const val EXTRA_TITLE = "title"
    }
}
