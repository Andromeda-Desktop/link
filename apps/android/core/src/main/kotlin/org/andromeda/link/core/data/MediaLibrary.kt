package org.andromeda.link.core.data

import android.Manifest
import android.app.PendingIntent
import android.app.RecoverableSecurityException
import android.content.ContentResolver
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.media.ThumbnailUtils
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.provider.MediaStore
import android.util.Size
import org.andromeda.link.ffi.LibraryItem
import org.andromeda.link.ffi.PhoneLibrary
import org.andromeda.link.ffi.PhotoMedia
import org.andromeda.link.ffi.RecentPhoto
import org.andromeda.link.ffi.RecentPhotos
import java.io.ByteArrayOutputStream
import java.io.File
import kotlin.math.roundToInt

/**
 * The media store by album, for a desktop browsing `Photos`: images and videos, which the core reads by path (the
 * media permissions allow that), and PNG thumbnails of them. Called off the main thread.
 */
class MediaLibrary(
    private val context: Context,
    /** Shows Android's prompt to trash an item Link may not change itself; the user confirms it on the phone. */
    private val askToTrash: (PendingIntent) -> Unit = {},
) : PhoneLibrary {
    /** Reads only the album column: on a large library the full rows took a slow phone past the desktop's timeout. */
    override fun albums(): List<String> {
        val type = MediaStore.Files.FileColumns.MEDIA_TYPE
        val args =
            arrayOf(
                MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE
                    .toString(),
                MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO
                    .toString(),
            )
        val collection = MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
        return context.contentResolver
            .query(collection, arrayOf(MediaStore.MediaColumns.BUCKET_DISPLAY_NAME), "($type=? OR $type=?)", args, null)
            ?.use { cursor -> buildSet { while (cursor.moveToNext()) add(cursor.getString(0) ?: "Other") } }
            .orEmpty()
            .sorted()
    }

    /** Browse path ("album/name") to the file path of the [recent] answers since the last one from offset 0. */
    @Volatile private var recentPaths: Map<String, String> = emptyMap()

    /** An item of the last [recent] answer comes first, so a name two files share resolves to the one that was listed. */
    override fun album(name: String): List<LibraryItem> {
        val listed = recentPaths
        return query(name).map { it.item }.sortedBy { if (listed["$name/${it.name}"] == it.path) 0 else 1 }
    }

    /**
     * The [count] newest images and videos of [media] from [offset] on, by date added, with how many there are; null
     * while Android withholds photo access: no grant at all, or no result set. A partial grant (Android 14+) lists the
     * items the user picked.
     */
    override fun recent(
        offset: UInt,
        count: UInt,
        media: PhotoMedia,
    ): RecentPhotos? {
        if (!canReadPhotos()) return null
        val image = MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE
        val video = MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO
        val types =
            when (media) {
                PhotoMedia.ALL -> listOf(image, video)
                PhotoMedia.PHOTOS -> listOf(image)
                PhotoMedia.VIDEOS -> listOf(video)
            }
        val type = MediaStore.Files.FileColumns.MEDIA_TYPE
        val selection =
            Bundle().apply {
                putString(ContentResolver.QUERY_ARG_SQL_SELECTION, types.joinToString(" OR ", "(", ")") { "$type=?" })
                putStringArray(ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, types.map { it.toString() }.toTypedArray())
            }
        val projection =
            arrayOf(
                MediaStore.MediaColumns.BUCKET_DISPLAY_NAME,
                MediaStore.MediaColumns.DISPLAY_NAME,
                @Suppress("DEPRECATION") MediaStore.MediaColumns.DATA,
                MediaStore.MediaColumns.SIZE,
                MediaStore.MediaColumns.DATE_ADDED,
                type,
                MediaStore.MediaColumns.DURATION,
            )
        val page =
            Bundle(selection).apply {
                putStringArray(ContentResolver.QUERY_ARG_SORT_COLUMNS, arrayOf(MediaStore.MediaColumns.DATE_ADDED))
                putInt(ContentResolver.QUERY_ARG_SORT_DIRECTION, ContentResolver.QUERY_SORT_DIRECTION_DESCENDING)
                putInt(ContentResolver.QUERY_ARG_OFFSET, offset.toInt())
                putInt(ContentResolver.QUERY_ARG_LIMIT, count.toInt())
            }
        val collection = MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
        var total = -1
        val rows =
            context.contentResolver
                .query(collection, projection, page, null)
                ?.use { cursor ->
                    total = cursor.extras.getInt(ContentResolver.EXTRA_TOTAL_COUNT, -1)
                    buildList<RecentPhoto> {
                        while (cursor.moveToNext()) {
                            val name = cursor.getString(1) ?: continue
                            val path = cursor.getString(2) ?: continue
                            val item = LibraryItem(name, path, cursor.getLong(3).toULong(), cursor.getLong(4).toULong())
                            val length = if (cursor.getInt(5) == video) (cursor.getLong(6) / 1000).toUInt() else null
                            add(RecentPhoto(cursor.getString(0) ?: "Other", item, length))
                        }
                    }
                } ?: return null
        // Not every media provider fills EXTRA_TOTAL_COUNT for a limited query: count the ids then.
        if (total < 0) {
            total = context.contentResolver
                .query(collection, arrayOf(MediaStore.MediaColumns._ID), selection, null)
                ?.use { it.count } ?: rows.size
        }
        val listed = rows.asReversed().associate { "${it.album}/${it.item.name}" to it.item.path }
        recentPaths = if (offset == 0u) listed else recentPaths + listed
        return RecentPhotos(total.toUInt(), rows)
    }

    private fun canReadPhotos(): Boolean {
        fun granted(permission: String) = context.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED
        return when {
            Build.VERSION.SDK_INT >= 34 ->
                granted(Manifest.permission.READ_MEDIA_IMAGES) ||
                    granted(Manifest.permission.READ_MEDIA_VIDEO) ||
                    granted(Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED)
            Build.VERSION.SDK_INT >= 33 ->
                granted(Manifest.permission.READ_MEDIA_IMAGES) || granted(Manifest.permission.READ_MEDIA_VIDEO)
            else -> granted(Manifest.permission.READ_EXTERNAL_STORAGE)
        }
    }

    override fun thumbnail(
        path: String,
        px: UShort,
    ): ByteArray? =
        runCatching {
            val file = File(path)
            val bitmap =
                if (mimeTypeOf(file.name).startsWith("video/")) {
                    ThumbnailUtils.createVideoThumbnail(file, Size(px.toInt(), px.toInt()), null)
                } else {
                    imageThumbnail(file, px.toInt())
                }
            ByteArrayOutputStream()
                .also { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
                .toByteArray()
                .also { bitmap.recycle() }
        }.getOrNull()

    /**
     * The image scaled to [px] on its longer side, upright. ThumbnailUtils returns the small embedded EXIF thumbnail
     * when there is one, and its whole-number subsampling leaves the shorter side of a tall screenshot near px / 2.
     * ImageDecoder applies the EXIF orientation itself.
     */
    private fun imageThumbnail(
        file: File,
        px: Int,
    ): Bitmap =
        ImageDecoder.decodeBitmap(ImageDecoder.createSource(file)) { decoder, info, _ ->
            val (width, height) = info.size.width to info.size.height
            val scale = minOf(1f, px.toFloat() / maxOf(width, height))
            decoder.setTargetSize(maxOf(1, (width * scale).roundToInt()), maxOf(1, (height * scale).roundToInt()))
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }

    /**
     * Moves the item to the trash (Android 11+) or deletes it (Android 10). An item another app owns needs the user's
     * consent: Android's prompt shows on the phone, and this waits up to [CONSENT_MS] for the item to go.
     */
    override fun trash(path: String): Boolean {
        val uri = uriOf(path) ?: return false
        val resolver = context.contentResolver
        val done =
            runCatching {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_TRASHED, 1) }, null) > 0
                } else {
                    resolver.delete(uri, null, null) > 0
                }
            }
        if (done.getOrDefault(false)) return true
        val error = done.exceptionOrNull()
        val prompt =
            when {
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.R -> MediaStore.createTrashRequest(resolver, listOf(uri), true)
                error is RecoverableSecurityException -> error.userAction.actionIntent
                else -> return false
            }
        askToTrash(prompt)
        val deadline = SystemClock.elapsedRealtime() + CONSENT_MS
        while (SystemClock.elapsedRealtime() < deadline) {
            if (gone(uri)) return true
            SystemClock.sleep(POLL_MS)
        }
        return false
    }

    /** The media store's own uri of the file at [path]. */
    private fun uriOf(path: String): Uri? {
        val collection = MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
        val projection = arrayOf(MediaStore.MediaColumns._ID, MediaStore.Files.FileColumns.MEDIA_TYPE)

        @Suppress("DEPRECATION")
        val selection = "${MediaStore.MediaColumns.DATA}=?"
        return context.contentResolver.query(collection, projection, selection, arrayOf(path), null)?.use { cursor ->
            if (!cursor.moveToFirst()) return@use null
            val base =
                if (cursor.getInt(1) == MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO) {
                    MediaStore.Video.Media.EXTERNAL_CONTENT_URI
                } else {
                    MediaStore.Images.Media.EXTERNAL_CONTENT_URI
                }
            ContentUris.withAppendedId(base, cursor.getLong(0))
        }
    }

    /** A query leaves out trashed items, so a trashed or deleted item finds nothing. */
    private fun gone(uri: Uri): Boolean =
        context.contentResolver.query(uri, arrayOf(MediaStore.MediaColumns._ID), null, null, null)?.use { it.count == 0 } ?: true

    private class Row(
        val album: String,
        val item: LibraryItem,
    )

    private fun query(album: String?): List<Row> {
        val type = MediaStore.Files.FileColumns.MEDIA_TYPE
        val bucket = MediaStore.MediaColumns.BUCKET_DISPLAY_NAME
        val selection = "($type=? OR $type=?)" + if (album != null) " AND $bucket=?" else ""
        val args =
            listOfNotNull(
                MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE
                    .toString(),
                MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO
                    .toString(),
                album,
            ).toTypedArray()
        val projection =
            arrayOf(
                bucket,
                MediaStore.MediaColumns.DISPLAY_NAME,
                @Suppress("DEPRECATION") MediaStore.MediaColumns.DATA,
                MediaStore.MediaColumns.SIZE,
                MediaStore.MediaColumns.DATE_MODIFIED,
            )
        val collection = MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
        return context.contentResolver
            .query(collection, projection, selection, args, null)
            ?.use { cursor ->
                buildList {
                    while (cursor.moveToNext()) {
                        val name = cursor.getString(1) ?: continue
                        val path = cursor.getString(2) ?: continue
                        val item = LibraryItem(name, path, cursor.getLong(3).toULong(), cursor.getLong(4).toULong())
                        add(Row(cursor.getString(0) ?: "Other", item))
                    }
                }
            }.orEmpty()
    }

    private companion object {
        /** Under the desktop's 60 s wait for a delete in Photos (link-daemon browse.rs DELETE_TIMEOUT). */
        const val CONSENT_MS = 55_000L
        const val POLL_MS = 500L
    }
}
