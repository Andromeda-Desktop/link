package org.andromeda.link.transfer

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import androidx.compose.runtime.getValue
import java.io.File

/**
 * An intent that opens [uri] in another app, or null when the file is gone. A `file:` URI, which the Files tab sends
 * from, cannot leave this app, so it is swapped for the file's MediaStore entry.
 */
fun viewIntent(
    context: Context,
    uri: String,
): Intent? {
    val parsed = Uri.parse(uri)
    val content = if (parsed.scheme == "file") mediaStoreUri(context, File(requireNotNull(parsed.path))) else parsed
    content ?: return null
    val mime = context.contentResolver.getType(content) ?: mimeOf(content.lastPathSegment.orEmpty())
    return Intent(Intent.ACTION_VIEW).setDataAndType(content, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
}

private fun mediaStoreUri(
    context: Context,
    file: File,
): Uri? {
    if (!file.exists()) return null
    val files = MediaStore.Files.getContentUri("external")
    return context.contentResolver
        .query(
            files,
            arrayOf(MediaStore.MediaColumns._ID),
            "${MediaStore.MediaColumns.DATA} = ?",
            arrayOf(file.path),
            null,
        )?.use { cursor -> if (cursor.moveToFirst()) Uri.withAppendedPath(files, cursor.getLong(0).toString()) else null }
}

private fun mimeOf(name: String): String =
    MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "*/*"
