package org.andromeda.link.webcam

import android.Manifest
import android.app.AlertDialog
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import org.andromeda.link.LinkApplication
import org.andromeda.link.R
import org.andromeda.link.capture.CaptureRequests
import org.andromeda.link.capture.desktopName
import org.andromeda.link.core.domain.CameraCommand

/**
 * Starts [CameraService] while visible, as Android requires for a camera service. Asks the user first unless the
 * notification's Start already did, then asks for the camera permission.
 */
class CameraStartActivity : ComponentActivity() {
    private val container get() = (application as LinkApplication).container
    private val requests get() = container.webcam.requests

    private val permission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) start() else refuse("not-allowed")
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val desktop = intent.getStringExtra(CaptureRequests.EXTRA_DESKTOP)
        if (desktop == null) {
            finish()
            return
        }
        requests.attach(this, desktop)
        if (savedInstanceState != null) return
        if (intent.getBooleanExtra(EXTRA_ASKED, false)) {
            allowed()
            return
        }
        AlertDialog
            .Builder(this)
            .setTitle(getString(R.string.webcam_ask_question, desktopName(container.repository, desktop)))
            .setMessage(R.string.webcam_ask_text)
            .setPositiveButton(R.string.webcam_start) { _, _ -> allowed() }
            .setNegativeButton(R.string.webcam_not_now) { _, _ -> refuse("declined") }
            .setOnCancelListener { refuse("declined") }
            .show()
    }

    private fun allowed() {
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            start()
        } else {
            permission.launch(Manifest.permission.CAMERA)
        }
    }

    private fun start() {
        val desktop = intent.getStringExtra(CaptureRequests.EXTRA_DESKTOP) ?: return finish()
        if (requests.take(desktop)) CameraService.start(this, intent)
        finish()
    }

    private fun refuse(reason: String) {
        val desktop = intent.getStringExtra(CaptureRequests.EXTRA_DESKTOP) ?: return finish()
        requests.refuse(desktop, reason)
        finish()
    }

    companion object {
        private const val EXTRA_ASKED = "asked"

        /** [asked] is true when the user already chose Start in the request's notification. */
        fun intent(
            context: Context,
            request: CameraCommand.Requested,
            asked: Boolean,
        ): Intent =
            CameraService
                .request(Intent(context, CameraStartActivity::class.java), request)
                .putExtra(EXTRA_ASKED, asked)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
