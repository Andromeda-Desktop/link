package org.andromeda.link.mic

import android.app.ActivityManager
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import org.andromeda.link.R
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.MicCommand
import org.andromeda.link.notifications.Channels
import org.andromeda.link.notifications.NotificationId

/**
 * Desktops' microphone requests. Android starts a microphone service only while the app is visible, so a request
 * opens [MicStartActivity] at once when Link is on screen, else a notification whose Start opens it; Not now refuses.
 */
class PhoneMic(
    private val context: Context,
    private val repository: LinkRepository,
    private val scope: CoroutineScope,
) {
    private val notifications = context.getSystemService(NotificationManager::class.java)

    fun start() {
        val receiver =
            object : BroadcastReceiver() {
                override fun onReceive(
                    context: Context,
                    intent: Intent,
                ) {
                    val desktopId = intent.getStringExtra(EXTRA_DESKTOP) ?: return
                    notifications.cancel(NOTIFICATION)
                    refuse(desktopId)
                }
            }
        ContextCompat.registerReceiver(context, receiver, IntentFilter(ACTION_REFUSE), ContextCompat.RECEIVER_NOT_EXPORTED)
        scope.launch { repository.mic.collect(::on) }
    }

    private fun on(command: MicCommand) {
        when (command) {
            is MicCommand.Requested -> {
                Log.i(TAG, "${command.desktopId} asks for the microphone")
                when {
                    MicService.running -> refuse(command.desktopId)
                    visible() -> context.startActivity(MicStartActivity.intent(context, command.desktopId))
                    else -> ask(command.desktopId)
                }
            }
            is MicCommand.Stopped -> {
                notifications.cancel(NOTIFICATION)
                MicService.stop(context, command.desktopId)
            }
        }
    }

    /** The system's state now: ProcessLifecycleOwner reports a stop 700 ms late, when an activity start is already blocked. */
    private fun visible() =
        ActivityManager.RunningAppProcessInfo().let {
            ActivityManager.getMyMemoryState(it)
            it.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
        }

    private fun ask(desktopId: String) {
        val channel = notifications.getNotificationChannel(Channels.MIC)
        // The request is only a notification; when it cannot show, the desktop would wait in vain.
        if (!notifications.areNotificationsEnabled() || channel?.importance == NotificationManager.IMPORTANCE_NONE) {
            refuse(desktopId)
            return
        }
        val name =
            repository.desktops.value
                .firstOrNull { it.id == desktopId }
                ?.name ?: desktopId
        val start =
            PendingIntent.getActivity(
                context,
                0,
                MicStartActivity.intent(context, desktopId),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val notNow =
            PendingIntent.getBroadcast(
                context,
                0,
                Intent(ACTION_REFUSE).setPackage(context.packageName).putExtra(EXTRA_DESKTOP, desktopId),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val notification =
            NotificationCompat
                .Builder(context, Channels.MIC)
                .setSmallIcon(R.drawable.ic_stat_link)
                .setContentTitle(context.getString(R.string.mic_ask_title, name))
                .setContentText(context.getString(R.string.mic_ask_text))
                .setPriority(NotificationCompat.PRIORITY_HIGH)
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setContentIntent(start)
                .setTimeoutAfter(TIMEOUT_MS)
                .setAutoCancel(true)
                .addAction(0, context.getString(R.string.mic_start), start)
                .addAction(0, context.getString(R.string.mic_not_now), notNow)
                .build()
        notifications.notify(NOTIFICATION, notification)
    }

    private fun refuse(desktopId: String) {
        scope.launch { repository.stopMic(desktopId).onFailure { Log.i(TAG, "refusing the microphone: ${it.message}") } }
    }

    companion object {
        const val EXTRA_DESKTOP = "desktop"
        val NOTIFICATION = NotificationId.MicRequest.id
        private const val TAG = "PhoneMic"
        private const val ACTION_REFUSE = "org.andromeda.link.MIC_REFUSE"

        /** A request older than a minute is stale: the user started the call without the phone. */
        private const val TIMEOUT_MS = 60_000L
    }
}
