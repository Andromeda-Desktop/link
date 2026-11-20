package org.andromeda.link.devices

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import org.andromeda.link.R
import org.andromeda.link.clipboard.LogAccess
import org.andromeda.link.clipboard.LogAccess.Method
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Pill
import org.andromeda.link.ui.kit.Separator
import org.andromeda.link.ui.kit.Sheet
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

/** What each method needs, read when Settings resumes or the pick changes. */
data class LogMethods(
    val inUse: Method?,
    val shizuku: Boolean,
    val shizukuAsks: Boolean,
    val root: Boolean,
    val su: Boolean,
    val helper: Boolean,
    val readLogs: Boolean,
) {
    fun ready(method: Method): Boolean =
        when (method) {
            Method.Automatic -> inUse != null
            Method.Shizuku -> shizuku
            Method.Root -> root
            Method.Helper -> helper
            Method.Android -> readLogs
        }
}

/** Settings > Advanced: one radio row per method, and the setup step of a pick not ready. */
@Suppress("ktlint:standard:function-naming")
@Composable
fun BoxScope.LogAccessSheet(
    open: Boolean,
    picked: Method,
    methods: LogMethods,
    onPick: (Method) -> Unit,
    onClipboardSetup: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    Sheet(open, onClose, stringResource(R.string.log_method), modifier) {
        GroupCard {
            Method.entries.forEachIndexed { index, method ->
                val selected = picked == method
                if (index > 0) Separator()
                ValueRow(
                    stringResource(method.title()),
                    Modifier.selectable(selected, role = Role.RadioButton) { onPick(method) },
                    subtitle =
                        when (method) {
                            Method.Automatic ->
                                methods.inUse?.let { stringResource(R.string.log_method_found, stringResource(it.title())) }
                                    ?: stringResource(R.string.log_method_none)
                            Method.Helper -> stringResource(R.string.log_method_helper_hint)
                            Method.Android -> stringResource(R.string.log_method_android_hint)
                            else -> null
                        },
                    leading = { Radio(selected) },
                ) {
                    stateWord(method, methods, s)?.let { (word, ink) ->
                        Text(stringResource(word), LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold), color = ink)
                    }
                }
                if (selected && !methods.ready(method)) {
                    Column(
                        Modifier.padding(start = Tok.Space.p16.dp, end = Tok.Space.p16.dp, bottom = Tok.Space.p12.dp),
                        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
                    ) {
                        Text(stringResource(setupStep(method, methods)), LinkTheme.phoneType.footnote, color = s.label2)
                        if (method == Method.Shizuku && methods.shizukuAsks) {
                            Pill(stringResource(R.string.onboarding_clipboard_shizuku), LogAccess::askShizuku)
                        } else if (method == Method.Automatic || method == Method.Android) {
                            Pill(stringResource(R.string.log_method_show_command), onClipboardSetup)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Radio(selected: Boolean) {
    val s = LinkTheme.scheme
    Box(
        Modifier
            .size(Tok.Phone.symbol.dp)
            .border(Tok.Space.p2.dp, if (selected) s.accent else s.label3, CircleShape),
        contentAlignment = Alignment.Center,
    ) { if (selected) Box(Modifier.size(Tok.Space.p12.dp).background(s.accent, CircleShape)) }
}

private fun stateWord(
    method: Method,
    methods: LogMethods,
    s: Tok.SchemeToken,
): Pair<Int, Color>? =
    when (method) {
        Method.Automatic -> null
        Method.Shizuku ->
            if (methods.shizuku) R.string.log_method_running to s.success else R.string.settings_set_up to s.warning
        Method.Root ->
            when {
                methods.root -> R.string.log_method_granted to s.success
                methods.su -> R.string.settings_set_up to s.warning
                else -> R.string.log_method_no_su to s.label3
            }
        Method.Helper ->
            if (methods.helper) R.string.log_method_running to s.success else R.string.log_method_off to s.label3
        Method.Android -> if (methods.readLogs) null else R.string.settings_set_up to s.warning
    }

private fun setupStep(
    method: Method,
    methods: LogMethods,
): Int =
    when (method) {
        Method.Automatic, Method.Android -> R.string.log_method_android_step
        Method.Shizuku -> if (methods.shizukuAsks) R.string.log_method_shizuku_allow_step else R.string.log_method_shizuku_step
        Method.Root -> if (methods.su) R.string.log_method_root_step else R.string.log_method_no_su_step
        Method.Helper -> R.string.log_method_helper_step
    }

fun Method.title(): Int =
    when (this) {
        Method.Automatic -> R.string.log_method_automatic
        Method.Shizuku -> R.string.log_method_shizuku
        Method.Root -> R.string.log_method_root
        Method.Helper -> R.string.log_method_helper
        Method.Android -> R.string.log_method_android
    }

fun logMethods(context: Context): LogMethods =
    LogMethods(
        LogAccess.inUse(context),
        LogAccess.shizukuReady(),
        LogAccess.shizukuAsks(),
        LogAccess.rootGranted(),
        LogAccess.suFound(),
        LogAccess.helperRunning(),
        LogAccess.readLogs(context),
    )
