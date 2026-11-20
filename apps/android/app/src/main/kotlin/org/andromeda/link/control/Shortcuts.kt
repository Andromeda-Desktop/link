package org.andromeda.link.control

import android.content.Context

/** `super+t` as `Super+T`. */
internal fun label(shortcut: String) = shortcut.split('+').joinToString("+") { part -> part.replaceFirstChar { it.uppercase() } }

internal fun prefs(context: Context) = context.getSharedPreferences("remote", Context.MODE_PRIVATE)

/** The user's shortcuts for the Keys sheet, as `ctrl+c`, in the order they were added. */
internal fun savedShortcuts(context: Context): List<String> =
    prefs(context).getString(SHORTCUTS_KEY, null)?.split("\n")?.filter { it.isNotEmpty() } ?: DEFAULT

internal const val SHORTCUTS_KEY = "shortcuts"
private val DEFAULT = listOf("ctrl+c", "ctrl+v", "alt+tab", "super+t", "ctrl+w")
