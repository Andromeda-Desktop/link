package org.andromeda.link.clipboard

import android.annotation.SuppressLint
import android.content.ClipData
import android.os.Binder
import android.os.IBinder
import android.os.Looper
import android.os.Parcel
import android.os.SystemClock
import android.system.Os
import android.util.Base64
import java.lang.reflect.Method
import kotlin.concurrent.thread
import kotlin.system.exitProcess

/**
 * The clipboard reader [LogHelper] runs as a child process. It registers Android's clipboard listener as the shell,
 * which may read the clipboard in the background, and prints one [ClipLine] per copy on stdout. Started by root, it
 * first becomes SystemUI's uid and calls as SystemUI, which Android also exempts from the "pasted from your clipboard"
 * toast; root itself gets empty clips. It ends when its stdin closes, that is when the helper ends.
 *
 * Started as `CLASSPATH=<Link's APK> [AS_UID=<uid>] app_process /system/bin <this class> <calling package>`.
 */
object ClipWatch {
    const val AS_UID = "AS_UID"

    private lateinit var service: Any
    private lateinit var get: Method
    private lateinit var caller: String
    private var last = ""
    private var lastAt = 0L

    // Runs under app_process, outside any app sandbox; IClipboard has no public API for another uid.
    @SuppressLint("DiscouragedPrivateApi", "PrivateApi")
    @JvmStatic
    fun main(args: Array<String>) {
        caller = args.firstOrNull() ?: exitProcess(2)
        System.getenv(AS_UID)?.toIntOrNull()?.let {
            Os.setgid(it)
            Os.setuid(it)
        }
        Looper.prepareMainLooper()
        val binder = Class.forName("android.os.ServiceManager").getMethod("getService", String::class.java).invoke(null, "clipboard")
        service = Class.forName("android.content.IClipboard\$Stub").getMethod("asInterface", IBinder::class.java).invoke(null, binder)!!
        val methods = service.javaClass.methods
        get = methods.first { it.name == "getPrimaryClip" }
        val add = methods.first { it.name == "addPrimaryClipChangedListener" }
        val listener =
            object : Binder() {
                override fun onTransact(
                    code: Int,
                    data: Parcel,
                    reply: Parcel?,
                    flags: Int,
                ): Boolean {
                    if (code != FIRST_CALL_TRANSACTION) return super.onTransact(code, data, reply, flags)
                    changed()
                    return true
                }
            }
        val stub =
            Class
                .forName("android.content.IOnPrimaryClipChangedListener\$Stub")
                .getMethod("asInterface", IBinder::class.java)
                .invoke(null, listener)
        add.invoke(service, *args(add, stub))
        println(ClipLine.READY)
        thread(isDaemon = true, name = "stdin") {
            while (System.`in`.read() >= 0) Unit
            exitProcess(0)
        }
        Looper.loop()
    }

    /** Android calls a listener twice per copy; the same line within [REPEAT_MS] is sent once. */
    @Synchronized
    private fun changed() {
        val clip = runCatching { get.invoke(service, *args(get, null)) as ClipData? }.getOrNull() ?: return
        val line = ClipLine.of(clip)
        val now = SystemClock.uptimeMillis()
        if (line == last && now - lastAt < REPEAT_MS) return
        last = line
        lastAt = now
        println(line)
        if (System.out.checkError()) exitProcess(0)
    }

    /** IClipboard's calls take the listener or nothing, then the calling package, tag, user and device; user 0 only. */
    private fun args(
        method: Method,
        first: Any?,
    ): Array<Any?> {
        var strings = 0
        return method.parameterTypes
            .map {
                when (it) {
                    String::class.java -> if (strings++ == 0) caller else null
                    Int::class.javaPrimitiveType -> 0
                    else -> first
                }
            }.toTypedArray()
    }

    private const val REPEAT_MS = 500L
}

/**
 * One copy as one line, so clip text with newlines survives [LogHelper]'s line pipe: `clip`, then the label and each
 * item's text and HTML in Base64, tab-separated. A clip with any item that is not text is [OTHER]: the app reads it
 * itself through [ClipboardOverlay], since a URI is only readable with the copying app's grant.
 */
object ClipLine {
    const val READY = "clip-ready"
    const val OTHER = "clip-other"
    private const val CLIP = "clip"
    private const val SYSTEM_UI = "systemui"

    /** What [LogHelper] sends Link on each connect: its reader runs, as SystemUI or as the shell. */
    fun ready(asSystemUi: Boolean) = "$READY ${if (asSystemUi) SYSTEM_UI else "shell"}"

    /** The capture mode a [ready] line announces; null for any other line. */
    fun mode(line: String): CaptureMode.Direct? =
        line.takeIf { it.startsWith("$READY ") }?.let { CaptureMode.Direct(it.endsWith(" $SYSTEM_UI")) }

    fun of(clip: ClipData): String {
        val items = (0 until clip.itemCount).map(clip::getItemAt)
        if (items.isEmpty() || items.any { it.text == null }) return OTHER
        val label = clip.description.label?.toString() ?: ""
        val fields = listOf(label) + items.flatMap { listOf(it.text.toString(), it.htmlText.orEmpty()) }
        return (listOf(CLIP) + fields.map { Base64.encodeToString(it.toByteArray(), Base64.NO_WRAP) }).joinToString("\t")
    }

    /** The clip a line carries; null for any other line. */
    fun parse(line: String): ClipData? {
        val fields = line.split('\t')
        if (fields.first() != CLIP || fields.size < 4 || fields.size % 2 != 0) return null
        val values = fields.drop(1).map { String(Base64.decode(it, Base64.NO_WRAP)) }
        val items = values.drop(1).chunked(2).map { (text, html) -> ClipData.Item(text, html.ifEmpty { null }) }
        val mimes = listOfNotNull("text/plain", "text/html".takeIf { items.any { it.htmlText != null } })
        val clip = ClipData(android.content.ClipDescription(values[0], mimes.toTypedArray()), items.first())
        items.drop(1).forEach(clip::addItem)
        return clip
    }
}
