package org.andromeda.link.mic

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import org.andromeda.link.LinkApplication
import org.andromeda.link.capture.CaptureRequests

/** Starts [MicService] while visible, as Android requires for a microphone service; asks for the permission first. */
class MicStartActivity : ComponentActivity() {
    private val requests get() = (application as LinkApplication).container.mic.requests

    private val permission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted -> answer(granted) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val desktop = intent.getStringExtra(CaptureRequests.EXTRA_DESKTOP)
        if (desktop == null) {
            finish()
            return
        }
        requests.attach(this, desktop)
        if (savedInstanceState != null) return
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            answer(true)
        } else {
            permission.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    private fun answer(granted: Boolean) {
        val desktop = intent.getStringExtra(CaptureRequests.EXTRA_DESKTOP) ?: return finish()
        when {
            !granted -> requests.refuse(desktop, "not-allowed")
            requests.take(desktop) -> MicService.start(this, desktop)
        }
        finish()
    }

    companion object {
        fun intent(
            context: Context,
            desktopId: String,
        ): Intent =
            Intent(context, MicStartActivity::class.java)
                .putExtra(CaptureRequests.EXTRA_DESKTOP, desktopId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
