package org.andromeda.link.capture

import android.app.ActivityManager
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.SystemClock
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.annotation.StringRes
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.andromeda.link.R
import org.andromeda.link.core.data.LinkRepository

/** What one capture (microphone, webcam) shows when it asks; [CaptureRequests] runs the rest. */
class CaptureAsk(
    val tag: String,
    val channel: String,
    val notification: Int,
    val refuseAction: String,
    @StringRes val title: Int,
    @StringRes val text: Int,
    @StringRes val start: Int,
    @StringRes val notNow: Int,
)

/**
 * Desktops' requests for one capture. Android starts a capture service only while the app is visible, so a request
 * opens the start activity at once when Link is on screen, else a notification whose Start opens it; Not now refuses.
 * One request waits at a time: another desktop's request meanwhile, or one while the capture runs, is refused busy.
 */
class CaptureRequests(
    private val context: Context,
    private val repository: LinkRepository,
    private val scope: CoroutineScope,
    private val ask: CaptureAsk,
    private val running: () -> Boolean,
    private val sendRefusal: suspend (desktopId: String, reason: String) -> Result<Unit>,
) {
    private class Waiting(
        val desktopId: String,
        val since: Long,
    ) {
        fun live(now: Long) = now - since < TIMEOUT_MS
    }

    private val waiting = MutableStateFlow<Waiting?>(null)
    private val notifications = context.getSystemService(NotificationManager::class.java)

    fun start() {
        val receiver =
            object : BroadcastReceiver() {
                override fun onReceive(
                    context: Context,
                    intent: Intent,
                ) {
                    refuse(intent.getStringExtra(EXTRA_DESKTOP) ?: return, "declined")
                }
            }
        ContextCompat.registerReceiver(context, receiver, IntentFilter(ask.refuseAction), ContextCompat.RECEIVER_NOT_EXPORTED)
    }

    /** [activity] gives the start activity's intent; its argument is true when the notification's Start already asked. */
    fun request(
        desktopId: String,
        activity: (asked: Boolean) -> Intent,
    ) {
        val now = SystemClock.elapsedRealtime()
        var busy = false
        waiting.update { current ->
            busy = running() || (current != null && current.desktopId != desktopId && current.live(now))
            if (busy) current else Waiting(desktopId, now)
        }
        when {
            busy -> send(desktopId, "busy")
            visible() -> context.startActivity(activity(false))
            else -> notify(desktopId, activity(true))
        }
    }

    /** The desktop ended its request, or the capture: the notification and any open start activity go. */
    fun stopped(desktopId: String) {
        clear(desktopId)
    }

    /** The user or the phone refused [desktopId]'s request. */
    fun refuse(
        desktopId: String,
        reason: String,
    ) {
        clear(desktopId)
        send(desktopId, reason)
    }

    /** The start activity calls this right before it starts the service; false when the request ended meanwhile. */
    fun take(desktopId: String): Boolean = clear(desktopId)

    /** Hides the request's notification and finishes [activity] once [desktopId]'s request no longer waits. */
    fun attach(
        activity: ComponentActivity,
        desktopId: String,
    ) {
        notifications.cancel(ask.notification)
        activity.lifecycleScope.launch {
            waiting.first { it?.desktopId != desktopId }
            activity.finish()
        }
    }

    private fun clear(desktopId: String): Boolean {
        val now = SystemClock.elapsedRealtime()
        var was = false
        waiting.update { current ->
            was = current?.desktopId == desktopId && current.live(now)
            if (current?.desktopId == desktopId) null else current
        }
        notifications.cancel(ask.notification)
        return was
    }

    private fun send(
        desktopId: String,
        reason: String,
    ) {
        scope.launch { sendRefusal(desktopId, reason).onFailure { Log.i(ask.tag, "refusing $desktopId: ${it.message}") } }
    }

    /** The system's state now: ProcessLifecycleOwner reports a stop 700 ms late, when an activity start is already blocked. */
    private fun visible() =
        ActivityManager.RunningAppProcessInfo().let {
            ActivityManager.getMyMemoryState(it)
            it.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
        }

    private fun notify(
        desktopId: String,
        activity: Intent,
    ) {
        val channel = notifications.getNotificationChannel(ask.channel)
        // The request is only a notification; when it cannot show, the desktop would wait in vain.
        if (!notifications.areNotificationsEnabled() || channel?.importance == NotificationManager.IMPORTANCE_NONE) {
            refuse(desktopId, "declined")
            return
        }
        val start = PendingIntent.getActivity(context, 0, activity, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val notNow =
            PendingIntent.getBroadcast(
                context,
                0,
                Intent(ask.refuseAction).setPackage(context.packageName).putExtra(EXTRA_DESKTOP, desktopId),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val notification =
            NotificationCompat
                .Builder(context, ask.channel)
                .setSmallIcon(R.drawable.ic_stat_link)
                .setContentTitle(context.getString(ask.title, desktopName(repository, desktopId)))
                .setContentText(context.getString(ask.text))
                .setPriority(NotificationCompat.PRIORITY_HIGH)
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setContentIntent(start)
                .setTimeoutAfter(TIMEOUT_MS)
                .setAutoCancel(true)
                .addAction(0, context.getString(ask.start), start)
                .addAction(0, context.getString(ask.notNow), notNow)
                .build()
        notifications.notify(ask.notification, notification)
    }

    companion object {
        const val EXTRA_DESKTOP = "desktop"

        /** A request older than a minute is stale: the user started the call without the phone. */
        private const val TIMEOUT_MS = 60_000L
    }
}

/** The desktop's paired name, never text from its request. */
internal fun desktopName(
    repository: LinkRepository,
    desktopId: String,
) = repository.desktops.value
    .firstOrNull { it.id == desktopId }
    ?.name ?: desktopId
