package org.andromeda.link.clipboard

import android.net.LocalServerSocket
import android.os.SystemClock
import org.andromeda.link.screen.InputRelay
import java.io.IOException
import kotlin.concurrent.thread
import kotlin.system.exitProcess

/**
 * The log helper the desktop starts over adb, the way Shizuku starts its server: `app_process` runs this class from
 * Link's APK as the shell uid, which reads other apps' log lines without Android's dialog. It lives until the phone
 * restarts. SELinux keeps apps from connecting to a shell's socket, so the helper writes to Link instead: a
 * `content write` into [LogLineProvider], which only the shell or root may open.
 *
 * Its limits: it writes only to Link's own provider; it sends only copies ([watch]), or where that fails Link's
 * clipboard-denial lines ([follow]); its one
 * input is mirror input from Link's own provider, which [org.andromeda.link.screen.InputRelay] turns into touch, key
 * and panel-power events and nothing else, so it is no shell; it exits when Link is uninstalled or has not run for
 * [IDLE_MS] ([main]).
 *
 * Started as `CLASSPATH=<Link's APK> app_process /system/bin org.andromeda.link.clipboard.LogHelper <package>`.
 */
object LogHelper {
    private const val LOCK = "org.andromeda.link.loghelper"
    private const val IDLE_MS = 30 * 60 * 1000L
    private const val BEAT_MS = 60 * 1000L
    private const val TICK_MS = 3000L
    private const val LOCK_TRIES = 5

    /** Bound while this helper lives, so a second start is a no-op. */
    private var lock: LocalServerSocket? = null

    /** `content write` into Link's provider; its stdin reaches Link. */
    @Volatile private var link: Process? = null

    /** [ClipLine.ready] once the reader runs; sent first on every connect. */
    @Volatile private var ready: String? = null

    /**
     * Lines that found no Link connected, with their uptime: a connect within [PENDING_MS] sends them (a watcher restart
     * or a Link restart); older ones are dropped, since a late copy would replace a newer desktop clip.
     */
    private val pending = ArrayDeque<Pair<Long, String>>()

    @JvmStatic
    fun main(args: Array<String>) {
        val pkg = args.firstOrNull() ?: exit("usage: LogHelper <package>")
        val appId = appId(pkg) ?: exit("$pkg is not installed")
        // A helper from a replaced APK holds the lock until its next tick notices the update.
        lock =
            generateSequence(0) { it + 1 }
                .take(LOCK_TRIES)
                .firstNotNullOfOrNull {
                    if (it > 0) Thread.sleep(TICK_MS / 2)
                    runCatching { LocalServerSocket(LOCK) }.getOrNull()
                } ?: exit("already running")
        println("helper for $pkg (uid $appId)")
        thread(isDaemon = true, name = "clipboard") { if (!watch()) follow(pkg) }
        thread(isDaemon = true, name = "input") {
            while (true) {
                if (running(pkg)) InputRelay.follow(pkg)
                Thread.sleep(TICK_MS)
            }
        }
        var lastRun = System.currentTimeMillis()
        var lastBeat = 0L
        var lastCheck = lastRun
        var linkPid: String? = null
        while (true) {
            Thread.sleep(TICK_MS)
            // An update deletes the APK this code came from; the updated Link must not reach this helper first.
            if (!java.io.File(System.getProperty("java.class.path").orEmpty()).exists()) exit("$pkg was updated")
            val now = System.currentTimeMillis()
            if (now - lastCheck > BEAT_MS) {
                lastCheck = now
                if (appId(pkg) != appId) exit("$pkg was uninstalled")
            }
            val pid = pid(pkg)
            when {
                // A heartbeat lets a closed reader (Link stopped watching) end `content write`.
                link?.isAlive == true && pid == linkPid -> {
                    lastRun = now
                    if (now - lastBeat > BEAT_MS) {
                        lastBeat = now
                        send("")
                    }
                }
                // Connects only to a running Link: opening its provider would start Link after a force stop. A new pid
                // is a restarted Link, whose old pipe would take lines into a dead process.
                pid != null -> {
                    lastRun = now
                    reconnect(pkg)
                    linkPid = pid
                }
                // Link died: lines wait in pending, not in a pipe nobody reads.
                link != null -> reconnect(null)
                now - lastRun > IDLE_MS -> exit("$pkg has not run for ${IDLE_MS / 60000} min")
            }
        }
    }

    /** Sends each copy from a [ClipWatch] child; false when the child cannot start, so [follow] stands in. */
    private fun watch(): Boolean {
        val root = android.os.Process.myUid() == android.os.Process.ROOT_UID
        val builder =
            ProcessBuilder("app_process", "/system/bin", ClipWatch::class.java.name, if (root) SYSTEM_UI else SHELL)
                .redirectError(ProcessBuilder.Redirect.INHERIT)
        if (root) builder.environment()[ClipWatch.AS_UID] = (appId(SYSTEM_UI) ?: return false).toString()
        val child = runCatching { builder.start() }.getOrElse { return false }
        val lines = child.inputStream.bufferedReader()
        if (runCatching { lines.readLine() }.getOrNull() != ClipLine.READY) {
            child.destroy()
            return false
        }
        ClipLine.ready(root).let {
            ready = it
            send(it)
        }
        runCatching { lines.forEachLine(::send) }
        exit("clipboard reader ended")
    }

    /** Sends Link's own clipboard-denial lines and nothing else; logcat ending ends the helper. */
    private fun follow(pkg: String) {
        val denial = "Denying clipboard access to $pkg"
        val logcat = ProcessBuilder("logcat", "-T", "1", "-v", "brief", "ClipboardService:E", "*:S").start()
        logcat.inputStream.bufferedReader().useLines { lines ->
            for (line in lines) if (denial in line) send(line)
        }
        exit("logcat ended")
    }

    /** A heartbeat (empty line) is never kept; a copy or denial waits in [pending] while Link is away. */
    @Synchronized
    private fun send(line: String) {
        if (!write(line) && line.isNotEmpty()) {
            if (pending.size == PENDING) pending.removeFirst()
            pending.addLast(SystemClock.uptimeMillis() to line)
        }
    }

    @Synchronized
    private fun reconnect(pkg: String?) {
        link?.destroy()
        link = pkg?.let(::connect)
        if (link == null) return
        ready?.let(::write)
        val since = SystemClock.uptimeMillis() - PENDING_MS
        pending.removeAll { (at, line) -> at < since || line == ready }
        while (pending.isNotEmpty() && write(pending.first().second)) pending.removeFirst()
    }

    private fun write(line: String): Boolean {
        val process = link ?: return false
        return try {
            process.outputStream.write((line + "\n").toByteArray())
            process.outputStream.flush()
            true
        } catch (_: IOException) {
            process.destroy()
            link = null
            false
        }
    }

    private fun connect(pkg: String): Process? =
        runCatching {
            ProcessBuilder("content", "write", "--uri", "content://$pkg.loghelper/lines")
                .redirectOutput(ProcessBuilder.Redirect.INHERIT)
                .redirectError(ProcessBuilder.Redirect.INHERIT)
                .start()
        }.getOrNull()

    private fun running(pkg: String): Boolean = pid(pkg) != null

    private fun pid(pkg: String): String? =
        runCatching {
            val process = ProcessBuilder("pidof", pkg).start()
            process.inputStream
                .bufferedReader()
                .readText()
                .trim()
                .takeIf { process.waitFor() == 0 }
        }.getOrNull()

    private fun appId(pkg: String): Int? =
        runCatching {
            val process = ProcessBuilder("cmd", "package", "list", "packages", "-U", pkg).start()
            val prefix = "package:$pkg uid:"
            process.inputStream
                .bufferedReader()
                .readLines()
                .firstOrNull { it.startsWith(prefix) }
                ?.removePrefix(prefix)
                ?.takeWhile(Char::isDigit)
                ?.toInt()
                ?.rem(PER_USER)
        }.getOrNull()

    private fun exit(why: String): Nothing {
        println(why)
        link?.destroy()
        InputRelay.reader?.destroy()
        exitProcess(0)
    }

    private const val PER_USER = 100000
    private const val PENDING = 20
    private const val PENDING_MS = 10_000L
    private const val SYSTEM_UI = "com.android.systemui"
    private const val SHELL = "com.android.shell"
}
