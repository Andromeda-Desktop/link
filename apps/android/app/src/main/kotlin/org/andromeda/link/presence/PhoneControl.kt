package org.andromeda.link.presence

import android.app.KeyguardManager
import android.app.NotificationManager
import android.bluetooth.BluetoothManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.database.ContentObserver
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.media.AudioManager
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.IncomingSystemCommand
import org.andromeda.link.core.domain.PhoneControls
import org.andromeda.link.screen.InputCommandProvider

/**
 * The phone's media volume, ringer, Do Not Disturb, lock, Wi-Fi, Bluetooth and torch, as desktops see them, and the
 * desktop commands that set them. Volume changes arrive through the system settings observer, the torch through its
 * callback, the rest as broadcasts, so nothing polls. Wi-Fi and Bluetooth go through the shell helper: Android lets no
 * app switch them since 10 and 13.
 */
class PhoneControl(
    private val context: Context,
    private val repository: LinkRepository,
    private val scope: CoroutineScope,
) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val notifications = context.getSystemService(NotificationManager::class.java)
    private val keyguard = context.getSystemService(KeyguardManager::class.java)
    private val wifi = context.getSystemService(WifiManager::class.java)
    private val bluetooth = context.getSystemService(BluetoothManager::class.java)?.adapter
    private val camera = context.getSystemService(CameraManager::class.java)
    private val torchCamera =
        runCatching {
            camera.cameraIdList.firstOrNull {
                camera.getCameraCharacteristics(it).get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
            }
        }.getOrNull()

    @Volatile
    private var torch: Boolean? = null
    private val _controls = MutableStateFlow(read())
    val controls: StateFlow<PhoneControls> = _controls.asStateFlow()

    fun start() {
        val refresh = { _controls.value = read() }
        context.contentResolver.registerContentObserver(
            Settings.System.CONTENT_URI,
            true,
            object : ContentObserver(Handler(Looper.getMainLooper())) {
                override fun onChange(selfChange: Boolean) = refresh()
            },
        )
        val filter =
            IntentFilter().apply {
                // Volume keys reach the settings observer only once the volume is saved, seconds later.
                addAction("android.media.VOLUME_CHANGED_ACTION")
                addAction(AudioManager.RINGER_MODE_CHANGED_ACTION)
                addAction(NotificationManager.ACTION_INTERRUPTION_FILTER_CHANGED)
                addAction(Intent.ACTION_SCREEN_OFF)
                addAction(Intent.ACTION_SCREEN_ON)
                addAction(Intent.ACTION_USER_PRESENT)
                addAction(WifiManager.WIFI_STATE_CHANGED_ACTION)
                addAction(android.bluetooth.BluetoothAdapter.ACTION_STATE_CHANGED)
            }
        // Exported: the volume broadcast is not a protected one, and comes from the system.
        ContextCompat.registerReceiver(
            context,
            object : BroadcastReceiver() {
                override fun onReceive(
                    context: Context,
                    intent: Intent,
                ) = refresh()
            },
            filter,
            ContextCompat.RECEIVER_EXPORTED,
        )
        if (torchCamera != null) {
            camera.registerTorchCallback(
                object : CameraManager.TorchCallback() {
                    override fun onTorchModeChanged(
                        cameraId: String,
                        enabled: Boolean,
                    ) {
                        if (cameraId != torchCamera) return
                        torch = enabled
                        refresh()
                    }
                },
                Handler(Looper.getMainLooper()),
            )
        }
        scope.launch {
            repository.systemCommands.collect { command ->
                val error = runCatching { run(command) }.exceptionOrNull()?.let { it.message ?: "It failed on the phone" }
                _controls.value = read()
                repository.systemResult(command.desktopId, command.id, error)
            }
        }
    }

    private fun read(): PhoneControls {
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).coerceAtLeast(1)
        val ringer =
            when (audio.ringerMode) {
                AudioManager.RINGER_MODE_SILENT -> "silent"
                AudioManager.RINGER_MODE_VIBRATE -> "vibrate"
                else -> "normal"
            }
        return PhoneControls(
            mediaVolume = audio.getStreamVolume(AudioManager.STREAM_MUSIC) * 100 / max,
            ringer = ringer,
            dnd = notifications.currentInterruptionFilter > NotificationManager.INTERRUPTION_FILTER_ALL,
            locked = keyguard.isDeviceLocked,
            wifi = wifi?.isWifiEnabled,
            bluetooth = bluetooth?.isEnabled,
            torch = torch,
        )
    }

    /** Throws with a message for the desktop's user when the command cannot be done. */
    private suspend fun run(command: IncomingSystemCommand) {
        val value = command.value ?: error("${command.command} needs a value")
        when (command.command) {
            "set-media-volume" -> {
                val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC)
                audio.setStreamVolume(AudioManager.STREAM_MUSIC, (value.coerceIn(0, 100) * max + 50) / 100, 0)
            }
            "set-ringer" -> {
                // Silent and leaving it change Do Not Disturb, which needs its access.
                needPolicyAccess()
                audio.ringerMode =
                    when (value) {
                        0 -> AudioManager.RINGER_MODE_SILENT
                        1 -> AudioManager.RINGER_MODE_VIBRATE
                        else -> AudioManager.RINGER_MODE_NORMAL
                    }
            }
            "set-dnd" -> {
                needPolicyAccess()
                notifications.setInterruptionFilter(
                    if (value == 1) NotificationManager.INTERRUPTION_FILTER_PRIORITY else NotificationManager.INTERRUPTION_FILTER_ALL,
                )
            }
            "set-torch" -> camera.setTorchMode(torchCamera ?: error("This phone has no torch"), value == 1)
            "set-wifi", "set-bluetooth" -> {
                // A helper started a moment ago, or a Link process Android just restarted, connects within 3 s.
                check(InputCommandProvider.awaitConnected(HELPER_WAIT_MS) && InputCommandProvider.send(command.command, code = value)) {
                    "Link's helper is not running on the phone: start it with root, Shizuku, or Link Apps over adb"
                }
            }
            else -> error("${command.command} is not a phone command")
        }
    }

    private fun needPolicyAccess() {
        check(notifications.isNotificationPolicyAccessGranted) { "Allow Do Not Disturb access for Link on the phone" }
    }
}

/** One helper look for Link (3 s) and its connect; linkd waits 5 s for the answer. */
private const val HELPER_WAIT_MS = 3_500L
