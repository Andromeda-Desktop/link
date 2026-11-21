package org.andromeda.link.core.data

import android.Manifest
import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Telephony.Mms
import android.provider.Telephony.Sms
import android.telephony.SmsManager
import org.andromeda.link.ffi.PhoneText
import org.andromeda.link.ffi.PhoneTextPart
import org.andromeda.link.ffi.PhoneTextState
import org.andromeda.link.ffi.TextOutcome
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.roundToInt

/**
 * The phone's SMS and MMS, newest first, and sending an SMS. Android writes a text this app sends into the provider
 * itself, since the app is not the default SMS app, so a sent text shows like any other; it asks for a delivery
 * report, and Android writes the report into the text's status. [texts], [part] and [send] are called off the main
 * thread; [changed] runs on it, at most once per [SETTLE_MS] of changes.
 */
class PhoneTexts(
    private val context: Context,
    private val changed: () -> Unit,
) {
    private val main = Handler(Looper.getMainLooper())
    private val notify = Runnable { changed() }
    private var watching = false
    private val sends = AtomicInteger()

    private class Entry(
        val date: Long,
        val id: Long,
        val mms: Boolean,
    )

    /** Every text's date and id, newest first; null after a change, rebuilt on the next page. */
    @Volatile private var index: List<Entry>? = null

    fun texts(
        offset: UInt,
        limit: UInt,
    ): List<PhoneText>? {
        if (!granted(Manifest.permission.READ_SMS)) return null
        val all = index ?: entries().also { index = it }
        val page = all.drop(offset.toInt()).take(limit.toInt())
        val sms = smsRows(page.filter { !it.mms }.map { it.id })
        return page.mapNotNull { entry -> if (entry.mms) mmsText(entry.id) else sms[entry.id] }
    }

    private fun entries(): List<Entry> {
        val entries = mutableListOf<Entry>()
        context.contentResolver
            .query(Sms.CONTENT_URI, arrayOf(Sms._ID, Sms.DATE), "${Sms.TYPE} != ${Sms.MESSAGE_TYPE_DRAFT}", null, null)
            ?.use { cursor -> while (cursor.moveToNext()) entries += Entry(cursor.getLong(1), cursor.getLong(0), false) }
        val shown = "${Mms.MESSAGE_BOX} != ${Mms.MESSAGE_BOX_DRAFTS} AND ${Mms.MESSAGE_TYPE} IN ($MMS_SEND_REQ, $MMS_RETRIEVE_CONF)"
        context.contentResolver
            .query(Mms.CONTENT_URI, arrayOf(Mms._ID, Mms.DATE), shown, null, null)
            ?.use { cursor -> while (cursor.moveToNext()) entries += Entry(cursor.getLong(1) * MS_PER_S, cursor.getLong(0), true) }
        return entries.sortedWith(compareByDescending<Entry> { it.date }.thenByDescending { it.id })
    }

    private fun smsRows(ids: List<Long>): Map<Long, PhoneText> {
        if (ids.isEmpty()) return emptyMap()
        val rows = mutableMapOf<Long, PhoneText>()
        context.contentResolver
            .query(
                Sms.CONTENT_URI,
                arrayOf(Sms._ID, Sms.THREAD_ID, Sms.ADDRESS, Sms.BODY, Sms.DATE, Sms.TYPE, Sms.STATUS),
                "${Sms._ID} IN (${ids.joinToString(",")})",
                null,
                null,
            )?.use { cursor ->
                while (cursor.moveToNext()) {
                    val type = cursor.getInt(5)
                    val status = cursor.getInt(6)
                    val state =
                        when {
                            type == Sms.MESSAGE_TYPE_OUTBOX || type == Sms.MESSAGE_TYPE_QUEUED -> PhoneTextState.SENDING
                            type == Sms.MESSAGE_TYPE_FAILED -> PhoneTextState.FAILED
                            type != Sms.MESSAGE_TYPE_SENT -> null
                            status == Sms.STATUS_COMPLETE -> PhoneTextState.DELIVERED
                            status >= Sms.STATUS_FAILED -> PhoneTextState.FAILED
                            else -> null
                        }
                    val id = cursor.getLong(0)
                    rows[id] =
                        PhoneText(
                            id = id.toULong(),
                            thread = cursor.getLong(1).toULong(),
                            address = cursor.getString(2).orEmpty(),
                            body = cursor.getString(3).orEmpty(),
                            date = cursor.getLong(4).toULong(),
                            sent = type != Sms.MESSAGE_TYPE_INBOX,
                            state = state,
                            parts = emptyList(),
                        )
                }
            }
        return rows
    }

    private fun mmsText(id: Long): PhoneText? {
        val resolver = context.contentResolver
        val row =
            resolver.query(Mms.CONTENT_URI, arrayOf(Mms.THREAD_ID, Mms.DATE, Mms.MESSAGE_BOX), "${Mms._ID} = $id", null, null)?.use {
                if (it.moveToFirst()) Triple(it.getLong(0), it.getLong(1), it.getInt(2)) else null
            } ?: return null
        val (thread, date, box) = row
        val sent = box != Mms.MESSAGE_BOX_INBOX
        val from = if (sent) MMS_ADDR_TO else MMS_ADDR_FROM
        val address =
            resolver
                .query(Uri.parse("content://mms/$id/addr"), arrayOf("address"), "type = $from", null, null)
                ?.use {
                    if (it.moveToFirst()) it.getString(0).orEmpty() else ""
                }.orEmpty()
        val body = StringBuilder()
        val parts = mutableListOf<PhoneTextPart>()
        resolver.query(MMS_PARTS, arrayOf("_id", "ct", "text"), "mid = $id", null, "seq")?.use { cursor ->
            while (cursor.moveToNext()) {
                val mime = cursor.getString(1).orEmpty()
                when {
                    mime == "text/plain" -> body.append(cursor.getString(2).orEmpty())
                    mime.startsWith("image/") && parts.size < MAX_PARTS ->
                        parts += PhoneTextPart(id = cursor.getLong(0).toULong(), mime = mime)
                }
            }
        }
        val state =
            when (box) {
                Mms.MESSAGE_BOX_OUTBOX -> PhoneTextState.SENDING
                Mms.MESSAGE_BOX_FAILED -> PhoneTextState.FAILED
                else -> null
            }
        return PhoneText(
            id = (id.toULong() or MMS_ID),
            thread = thread.toULong(),
            address = address,
            body = body.toString(),
            date = (date * MS_PER_S).toULong(),
            sent = sent,
            state = state,
            parts = parts,
        )
    }

    /** A PNG of the MMS image part [id], at most [px] on its longer side; null when it is gone or no image. */
    fun part(
        id: ULong,
        px: UShort,
    ): ByteArray? {
        if (!granted(Manifest.permission.READ_SMS)) return null
        val uri = Uri.withAppendedPath(MMS_PARTS, id.toString())
        val mime =
            context.contentResolver
                .query(
                    uri,
                    arrayOf("ct"),
                    null,
                    null,
                    null,
                )?.use { if (it.moveToFirst()) it.getString(0) else null }
        if (mime?.startsWith("image/") != true) return null
        return try {
            val bitmap =
                ImageDecoder.decodeBitmap(ImageDecoder.createSource(context.contentResolver, uri)) { decoder, info, _ ->
                    val (width, height) = info.size.width to info.size.height
                    val scale = minOf(1f, px.toFloat() / maxOf(width, height))
                    decoder.setTargetSize(maxOf(1, (width * scale).roundToInt()), maxOf(1, (height * scale).roundToInt()))
                    decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
                }
            ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
        } catch (_: IOException) {
            null
        }
    }

    /** Sends [body] to [to] and waits until Android reports every part sent, at most [SEND_TIMEOUT_S]. */
    fun send(
        to: String,
        body: String,
    ): TextOutcome {
        if (!granted(Manifest.permission.SEND_SMS)) return TextOutcome.DENIED
        val sms = context.getSystemService(SmsManager::class.java) ?: return TextOutcome.FAILED
        val parts = sms.divideMessage(body)
        val action = "${context.packageName}.TEXT_SENT.${sends.incrementAndGet()}"
        val left = CountDownLatch(parts.size)
        val ok = AtomicBoolean(true)
        val receiver =
            object : BroadcastReceiver() {
                override fun onReceive(
                    context: Context,
                    intent: Intent,
                ) {
                    if (resultCode != Activity.RESULT_OK) ok.set(false)
                    left.countDown()
                }
            }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, IntentFilter(action), Context.RECEIVER_NOT_EXPORTED)
        } else {
            context.registerReceiver(receiver, IntentFilter(action))
        }
        try {
            val sent =
                parts.indices.map { part ->
                    val intent = Intent(action).setPackage(context.packageName)
                    PendingIntent.getBroadcast(context, part, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_ONE_SHOT)
                }
            val delivered =
                parts.indices.map { part ->
                    val intent = Intent("${context.packageName}.TEXT_DELIVERED").setPackage(context.packageName)
                    PendingIntent.getBroadcast(context, part, intent, PendingIntent.FLAG_IMMUTABLE)
                }
            sms.sendMultipartTextMessage(to, null, parts, ArrayList(sent), ArrayList(delivered))
            val finished = left.await(SEND_TIMEOUT_S, TimeUnit.SECONDS)
            return if (finished && ok.get()) TextOutcome.SENT else TextOutcome.FAILED
        } catch (_: RuntimeException) {
            return TextOutcome.FAILED
        } finally {
            context.unregisterReceiver(receiver)
        }
    }

    /** Starts watching for texts once the permission is there, since the provider refuses observers before it. */
    fun watch() {
        if (watching || !granted(Manifest.permission.READ_SMS)) return
        val observer =
            object : ContentObserver(main) {
                override fun onChange(selfChange: Boolean) {
                    index = null
                    main.removeCallbacks(notify)
                    main.postDelayed(notify, SETTLE_MS)
                }
            }
        context.contentResolver.registerContentObserver(Sms.CONTENT_URI, true, observer)
        context.contentResolver.registerContentObserver(Mms.CONTENT_URI, true, observer)
        watching = true
        index = null
        changed()
    }

    private fun granted(permission: String) = context.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED

    private companion object {
        const val SETTLE_MS = 1_000L
        const val SEND_TIMEOUT_S = 60L
        const val MS_PER_S = 1_000L
        const val MAX_PARTS = 4

        /** Matches link_proto MMS_ID: an MMS id never collides with an SMS id. */
        const val MMS_ID = 0x4000_0000_0000_0000uL

        /** PduHeaders: m-send-req (a sent MMS) and m-retrieve-conf (a downloaded one). */
        const val MMS_SEND_REQ = 128
        const val MMS_RETRIEVE_CONF = 132

        /** PduHeaders FROM and TO, the address row types. */
        const val MMS_ADDR_FROM = 137
        const val MMS_ADDR_TO = 151
        val MMS_PARTS: Uri = Uri.parse("content://mms/part")
    }
}
