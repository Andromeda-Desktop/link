package org.andromeda.link.update

import android.Manifest
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.andromeda.link.MainActivity
import org.andromeda.link.R
import org.andromeda.link.notifications.Channels
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL

/** Updates the app from the newest GitHub release: the APK streams into a PackageInstaller session, Android confirms. */
object AppUpdate {
    sealed interface State {
        data object Idle : State

        data object Checking : State

        data object Current : State

        data class Available(
            val version: String,
            val url: String,
            val bytes: Long,
        ) : State

        /** `total` is 0 while the size is unknown. */
        data class Downloading(
            val update: Available,
            val received: Long,
            val total: Long,
        ) : State {
            val progress get() = if (total > 0) (received.toFloat() / total).coerceIn(0f, 1f) else 0f
        }

        data object Installing : State

        data class Failed(
            val message: String,
        ) : State
    }

    sealed interface Event {
        data object Check : Event

        data class Checked(
            val result: State,
        ) : Event

        data class Start(
            val update: State.Available,
        ) : Event

        data class Progress(
            val received: Long,
            val total: Long,
        ) : Event

        data object Downloaded : Event

        data object Cancelled : Event

        data class Fail(
            val message: String,
        ) : Event

        data object Installed : Event

        data object Aborted : Event
    }

    /** The one transition function: returns null for an event the state does not accept. */
    internal fun reduce(
        state: State,
        event: Event,
    ): State? =
        when (event) {
            Event.Check -> State.Checking.takeIf { state !is State.Downloading && state != State.Installing && state != State.Checking }
            is Event.Checked ->
                event.result.takeIf {
                    state == State.Checking && (it == State.Current || it is State.Available || it is State.Failed)
                }
            is Event.Start -> State.Downloading(event.update, 0, event.update.bytes).takeIf { state == event.update }
            is Event.Progress ->
                (state as? State.Downloading)
                    ?.takeIf { event.received >= it.received }
                    ?.copy(received = event.received, total = event.total)
            Event.Downloaded -> State.Installing.takeIf { state is State.Downloading }
            Event.Cancelled -> (state as? State.Downloading)?.update
            is Event.Fail ->
                State.Failed(event.message).takeIf {
                    state == State.Checking || state is State.Downloading || state == State.Installing
                }
            Event.Installed -> State.Current.takeIf { state == State.Installing }
            Event.Aborted -> State.Idle.takeIf { state == State.Installing }
        }

    private val _state = MutableStateFlow<State>(State.Idle)
    val state: StateFlow<State> = _state

    private fun send(event: Event): Boolean {
        while (true) {
            val current = _state.value
            val next = reduce(current, event) ?: return false
            if (_state.compareAndSet(current, next)) return true
        }
    }

    /** Outlives every screen: leaving Settings or the prompt must not stop a download. */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var job: Job? = null

    @Volatile private var connection: HttpURLConnection? = null

    suspend fun check(context: Context) {
        if (!send(Event.Check)) return
        val result =
            withContext(Dispatchers.IO) {
                runCatching {
                    val release = JSONObject(get(LATEST).inputStream.bufferedReader().use { it.readText() })
                    val version = release.getString("tag_name").removePrefix("v")
                    val assets = release.getJSONArray("assets")
                    val apk =
                        (0 until assets.length())
                            .map { assets.getJSONObject(it) }
                            .firstOrNull { it.getString("name").endsWith(".apk") }
                    val installed =
                        context.packageManager
                            .getPackageInfo(context.packageName, 0)
                            .versionName
                            .orEmpty()
                    if (apk == null || compareVersions(version, installed) <= 0) {
                        State.Current
                    } else {
                        State.Available(version, apk.getString("browser_download_url"), apk.optLong("size"))
                    }
                }.getOrElse { State.Failed(it.message ?: it.javaClass.simpleName) }
            }
        send(Event.Checked(result))
    }

    /** The background check: posts one notification per new version, and leaves a running install alone. */
    suspend fun checkAndNotify(context: Context) {
        check(context)
        val update = _state.value as? State.Available ?: return
        val prefs = prefs(context)
        if (prefs.getString(KEY_NOTIFIED, null) == update.version) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val open = PendingIntent.getActivity(context, 0, Intent(context, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        NotificationManagerCompat.from(context).notify(
            TAG,
            0,
            NotificationCompat
                .Builder(context, Channels.UPDATES)
                .setSmallIcon(R.drawable.ic_stat_link)
                .setContentTitle(context.getString(R.string.update_prompt_title, update.version))
                .setContentText(context.getString(R.string.update_notify_text))
                .setContentIntent(open)
                .setAutoCancel(true)
                .build(),
        )
        prefs.edit { putString(KEY_NOTIFIED, update.version) }
    }

    /** The version whose prompt the user put off; the app asks again only for a newer one. */
    fun dismissed(context: Context): String? = prefs(context).getString(KEY_DISMISSED, null)

    fun dismiss(
        context: Context,
        version: String,
    ) = prefs(context).edit { putString(KEY_DISMISSED, version) }

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun install(
        context: Context,
        update: State.Available,
    ) {
        if (!send(Event.Start(update))) return
        val app = context.applicationContext
        job = scope.launch { download(app, update) }
    }

    /** Stops a download; the installer session is abandoned and the update shows as available again. */
    fun cancel() {
        job?.cancel()
        // A blocked read only returns when the socket closes.
        connection?.disconnect()
    }

    private suspend fun download(
        context: Context,
        update: State.Available,
    ) {
        try {
            val installer = context.packageManager.packageInstaller
            val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
            params.setAppPackageName(context.packageName)
            val id = installer.createSession(params)
            installer.openSession(id).use { session ->
                try {
                    val http = open(update.url)
                    connection = http
                    http.ok()
                    val total = http.contentLengthLong.takeIf { it > 0 } ?: update.bytes
                    http.inputStream.use { input ->
                        session.openWrite("link.apk", 0, total.takeIf { it > 0 } ?: -1).use { output ->
                            val buffer = ByteArray(64 * 1024)
                            var received = 0L
                            while (true) {
                                currentCoroutineContext().ensureActive()
                                val read = input.read(buffer)
                                if (read < 0) break
                                output.write(buffer, 0, read)
                                received += read
                                send(Event.Progress(received, total))
                            }
                            session.fsync(output)
                        }
                    }
                    currentCoroutineContext().ensureActive()
                    send(Event.Downloaded)
                    val result =
                        PendingIntent.getBroadcast(
                            context,
                            id,
                            Intent(context, UpdateReceiver::class.java),
                            // Mutable: the installer adds the status extras.
                            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
                        )
                    session.commit(result.intentSender)
                } catch (e: Exception) {
                    session.abandon()
                    throw e
                }
            }
        } catch (e: Exception) {
            when {
                !currentCoroutineContext().isActive -> send(Event.Cancelled)
                e is SocketTimeoutException -> send(Event.Fail(context.getString(R.string.update_stalled)))
                else -> send(Event.Fail(e.message ?: e.javaClass.simpleName))
            }
        } finally {
            connection = null
        }
    }

    internal fun finished(
        context: Context,
        intent: Intent,
    ) {
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                @Suppress("DEPRECATION")
                val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT) ?: return
                context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            PackageInstaller.STATUS_SUCCESS -> send(Event.Installed)
            PackageInstaller.STATUS_FAILURE_ABORTED -> send(Event.Aborted)
            else -> send(Event.Fail(intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "Install failed"))
        }
    }

    private fun get(url: String): HttpURLConnection = open(url).ok()

    private fun open(url: String): HttpURLConnection =
        (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 15_000
            readTimeout = 30_000
            setRequestProperty("Accept", "application/vnd.github+json")
            setRequestProperty("User-Agent", "AndromedaLink")
        }

    private fun HttpURLConnection.ok() =
        apply {
            if (responseCode !in
                200..299
            ) {
                throw IllegalStateException("GitHub answered $responseCode")
            }
        }

    /** Compares dotted numeric versions ("0.2.10" > "0.2.9"); a missing part counts as 0. */
    internal fun compareVersions(
        a: String,
        b: String,
    ): Int {
        val x = a.split('.', '-').map { it.toIntOrNull() ?: 0 }
        val y = b.split('.', '-').map { it.toIntOrNull() ?: 0 }
        for (i in 0 until maxOf(x.size, y.size)) {
            val c = x.getOrElse(i) { 0 }.compareTo(y.getOrElse(i) { 0 })
            if (c != 0) return c
        }
        return 0
    }

    private const val PREFS = "update"
    private const val KEY_DISMISSED = "dismissed"
    private const val KEY_NOTIFIED = "notified"
    private const val TAG = "update"
    private const val LATEST = "https://api.github.com/repos/Andromeda-Desktop/link/releases/latest"
}

class UpdateReceiver : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent,
    ) = AppUpdate.finished(context, intent)
}
