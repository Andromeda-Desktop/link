package org.andromeda.link.mic

import android.Manifest
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import org.andromeda.link.LinkApplication

/** Starts [MicService] while visible, as Android requires for a microphone service; asks for the permission first. */
class MicStartActivity : ComponentActivity() {
    private val permission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted -> answer(granted) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        getSystemService(NotificationManager::class.java).cancel(PhoneMic.NOTIFICATION)
        if (savedInstanceState != null) return
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            answer(true)
        } else {
            permission.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    private fun answer(granted: Boolean) {
        val desktop = intent.getStringExtra(PhoneMic.EXTRA_DESKTOP)
        if (desktop == null) {
            finish()
            return
        }
        if (granted) {
            MicService.start(this, desktop)
            finish()
            return
        }
        val repository = (application as LinkApplication).container.repository
        lifecycleScope.launch {
            repository.stopMic(desktop)
            finish()
        }
    }

    companion object {
        fun intent(
            context: Context,
            desktopId: String,
        ): Intent =
            Intent(context, MicStartActivity::class.java)
                .putExtra(PhoneMic.EXTRA_DESKTOP, desktopId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
