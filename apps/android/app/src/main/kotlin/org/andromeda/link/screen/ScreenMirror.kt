package org.andromeda.link.screen

import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.provider.Settings
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import org.andromeda.link.LinkApplication
import org.andromeda.link.R
import org.andromeda.link.clipboard.LogAccess
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.MirrorCommand
import org.andromeda.link.core.domain.SoundCommand
import org.andromeda.link.notifications.Channels

/**
 * Mirroring this phone's screen to a desktop. A request becomes a notification whose Allow opens Android's capture
 * prompt (Android asks every session, and an app may not skip it); input goes to [ScreenInput], which the user turns
 * on once in Accessibility settings. A desktop may ask for the phone's sound alone ([CaptureMode.Sound]) through the
 * same consent; the phone has one projection, so a mirror request ends a sound-only capture first.
 */
class ScreenMirror(
    private val context: Context,
    private val repository: LinkRepository,
    private val scope: CoroutineScope,
) {
    private val notifications = context.getSystemService(NotificationManager::class.java)

    fun start() {
        scope.launch { repository.mirrorCommands.collect(::on) }
        scope.launch { repository.sound.collect(::on) }
    }

    private fun on(command: SoundCommand) {
        when (command) {
            is SoundCommand.Requested -> request(command.desktopId, CaptureMode.Sound)
            is SoundCommand.Stopped -> {
                notifications.cancel(requestNotification(CaptureMode.Sound))
                ScreenCapture.stop(context, command.desktopId, CaptureMode.Sound)
            }
        }
    }

    private fun request(
        desktopId: String,
        mode: CaptureMode,
    ) {
        scope.launch(Dispatchers.IO) {
            val start = System.currentTimeMillis()
            Log.i(TAG, "$desktopId requests the capture: $mode")
            LogAccess.startHelper(context)
            if (mode == CaptureMode.Mirror && ScreenCapture.endSound(context)) {
                withTimeoutOrNull(HELPER_WAIT_MS) { while (ScreenCapture.running) delay(POLL_MS) }
            }
            val silent = captureSilently(desktopId, mode)
            Log.i(TAG, "silent capture: $silent after ${System.currentTimeMillis() - start} ms")
            if (!silent) ask(desktopId, mode)
        }
    }

    private fun on(command: MirrorCommand) {
        when (command) {
            is MirrorCommand.Requested -> request(command.desktopId, CaptureMode.Mirror)
            is MirrorCommand.Input ->
                when {
                    ScreenInput.enabled || InputCommandProvider.connected -> ScreenInput.perform(command)
                    // A helper lost mid-mirror comes back within its 3 s poll; this input is dropped meanwhile.
                    LogAccess.helperPossible() -> scope.launch(Dispatchers.IO) { LogAccess.startHelper(context) }
                    else -> askForControl()
                }
            is MirrorCommand.Keyframe -> ScreenCapture.requestKeyframe()
            is MirrorCommand.Lag -> ScreenCapture.reportLag(command.lagMs)
            is MirrorCommand.Stopped -> {
                notifications.cancel(requestNotification(CaptureMode.Mirror))
                notifications.cancel(CONTROL_NOTIFICATION)
                ScreenCapture.stop(context, command.desktopId, CaptureMode.Mirror)
            }
        }
    }

    /** Through the shell helper, which starts the capture with no prompt; false when it cannot or did not. */
    private suspend fun captureSilently(
        desktopId: String,
        mode: CaptureMode,
    ): Boolean {
        if (ScreenCapture.running || !LogAccess.helperPossible()) return false
        withTimeoutOrNull(HELPER_WAIT_MS) { while (!InputCommandProvider.connected) delay(POLL_MS) } ?: return false
        if (!InputCommandProvider.send("capture", code = mode.ordinal, text = desktopId)) return false
        // A helper from an older Link ignores the command; the notification asks then.
        return withTimeoutOrNull(HELPER_WAIT_MS) { while (!ScreenCapture.running) delay(POLL_MS) } != null
    }

    private fun ask(
        desktopId: String,
        mode: CaptureMode,
    ) {
        Log.i(TAG, "$desktopId asks for the capture: $mode")
        if (ScreenCapture.running) {
            refuse(desktopId, "the phone is already mirroring", mode)
            return
        }
        // The request is only a notification; when it cannot show, the desktop would wait in vain.
        val channel = notifications.getNotificationChannel(Channels.SCREEN)
        if (!notifications.areNotificationsEnabled() || channel?.importance == NotificationManager.IMPORTANCE_NONE) {
            refuse(desktopId, "Link's notifications are off on the phone, so it cannot ask to share the screen", mode)
            return
        }
        val name =
            repository.desktops.value
                .firstOrNull { it.id == desktopId }
                ?.name ?: desktopId
        val allow =
            PendingIntent.getActivity(
                context,
                mode.ordinal,
                Intent(context, CaptureConsentActivity::class.java)
                    .putExtra(EXTRA_DESKTOP, desktopId)
                    .putExtra(ScreenCapture.EXTRA_MODE, mode.name)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val decline =
            PendingIntent.getBroadcast(
                context,
                mode.ordinal,
                Intent(context, DeclineReceiver::class.java)
                    .putExtra(EXTRA_DESKTOP, desktopId)
                    .putExtra(ScreenCapture.EXTRA_MODE, mode.name),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val notification =
            NotificationCompat
                .Builder(context, Channels.SCREEN)
                .setSmallIcon(R.drawable.ic_stat_link)
                .setContentTitle(
                    context.getString(
                        when (mode) {
                            CaptureMode.Mirror -> R.string.screen_request_title
                            CaptureMode.Sound -> R.string.sound_request_title
                        },
                        name,
                    ),
                ).setContentText(context.getString(R.string.screen_request_text))
                .setPriority(NotificationCompat.PRIORITY_HIGH)
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setContentIntent(allow)
                .setTimeoutAfter(REQUEST_TIMEOUT_MS)
                .setAutoCancel(true)
                .addAction(0, context.getString(R.string.screen_allow), allow)
                .addAction(0, context.getString(R.string.screen_decline), decline)
                .build()
        notifications.notify(requestNotification(mode), notification)
    }

    /** Input arrived while the accessibility service is off: Android drops it, so ask the user to turn it on. */
    private fun askForControl() {
        if (notifications.activeNotifications.any { it.id == CONTROL_NOTIFICATION }) return
        Log.i(TAG, "input dropped: the accessibility service is off")
        val settings =
            PendingIntent.getActivity(
                context,
                0,
                Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                PendingIntent.FLAG_IMMUTABLE,
            )
        val notification =
            NotificationCompat
                .Builder(context, Channels.SCREEN)
                .setSmallIcon(R.drawable.ic_stat_link)
                .setContentTitle(context.getString(R.string.screen_control_off))
                .setContentText(context.getString(R.string.screen_input_description))
                .setStyle(NotificationCompat.BigTextStyle().bigText(context.getString(R.string.screen_input_description)))
                .setPriority(NotificationCompat.PRIORITY_HIGH)
                .setContentIntent(settings)
                .setAutoCancel(true)
                .build()
        notifications.notify(CONTROL_NOTIFICATION, notification)
    }

    fun refuse(
        desktopId: String,
        reason: String,
        mode: CaptureMode,
    ) {
        notifications.cancel(requestNotification(mode))
        scope.launch {
            when (mode) {
                CaptureMode.Mirror -> repository.stopMirror(desktopId, reason)
                CaptureMode.Sound -> repository.stopSound(desktopId)
            }.onFailure { Log.i(TAG, "refusing the capture: ${it.message}") }
        }
    }

    class DeclineReceiver : BroadcastReceiver() {
        override fun onReceive(
            context: Context,
            intent: Intent,
        ) {
            val desktop = intent.getStringExtra(EXTRA_DESKTOP) ?: return
            val mode = CaptureMode.of(intent.getStringExtra(ScreenCapture.EXTRA_MODE))
            (context.applicationContext as LinkApplication).container.screen.refuse(desktop, "declined on the phone", mode)
        }
    }

    private fun requestNotification(mode: CaptureMode) =
        when (mode) {
            CaptureMode.Mirror -> REQUEST_NOTIFICATION
            CaptureMode.Sound -> SOUND_REQUEST_NOTIFICATION
        }

    companion object {
        const val EXTRA_DESKTOP = "desktop"
        private const val REQUEST_NOTIFICATION = 40
        private const val SOUND_REQUEST_NOTIFICATION = 46
        private const val CONTROL_NOTIFICATION = 42

        // The desktop gives up after 60 s.
        const val REQUEST_TIMEOUT_MS = 60_000L
        private const val TAG = "ScreenMirror"

        // The helper polls for Link every 3 s.
        private const val HELPER_WAIT_MS = 5_000L
        private const val POLL_MS = 100L
    }
}
