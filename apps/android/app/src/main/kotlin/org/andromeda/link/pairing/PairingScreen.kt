package org.andromeda.link.pairing

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import org.andromeda.link.AppContainer
import org.andromeda.link.R
import org.andromeda.link.core.domain.Desktop
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.Screen
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok

/**
 * Pairing: the camera first, reading the desktop's QR code and pairing at once; the 6-digit code one tap away; and a
 * QR link opened from outside the app shown for confirmation, since nobody here chose to scan it.
 */
@Composable
fun PairingScreen(
    container: AppContainer,
    onBack: () -> Unit,
    onPair: (Desktop) -> Unit,
    modifier: Modifier = Modifier,
    viewModel: PairingViewModel = viewModel { PairingViewModel(container.repository) },
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    var typing by rememberSaveable { mutableStateOf(false) }
    val pair by rememberUpdatedState(onPair)
    LaunchedEffect(viewModel) { viewModel.paired.collect { pair(it) } }
    BackHandler(typing && !state.busy && state.link == null) { typing = false }
    BackHandler(state.link != null && !state.busy, viewModel::dismissLink)
    val failure = state.failure?.text(LocalResources.current)
    Box(modifier) {
        Screen(background = LinkTheme.scheme.solidRegular) {
            Column(Modifier.fillMaxSize().imePadding().navigationBarsPadding()) {
                when {
                    state.busy -> Busy()
                    state.link != null -> LinkConfirmation(failure, viewModel::pairWithLink, viewModel::dismissLink)
                    typing -> CodeEntry(state, failure, viewModel::updateCode, viewModel::pairWithCode) { typing = false }
                    else -> Scan(failure, onBack, viewModel::pairScanned, viewModel::scanAgain) { typing = true }
                }
            }
        }
    }
}

@Composable
private fun ColumnScope.Scan(
    failure: String?,
    onBack: () -> Unit,
    onPairingUri: (String) -> Unit,
    onScanAgain: () -> Unit,
    onType: () -> Unit,
) {
    val context = LocalContext.current
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    // Pairing falls back to Bluetooth when no IP path answers, and a typed code scans for the desktop over
    // Bluetooth LE: both need Nearby devices from Android 12.
    val wanted =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            arrayOf(Manifest.permission.CAMERA, Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_SCAN)
        } else {
            arrayOf(Manifest.permission.CAMERA)
        }
    val ask =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            granted = it[Manifest.permission.CAMERA] ?: granted
        }
    LaunchedEffect(Unit) {
        val missing = wanted.filter { ContextCompat.checkSelfPermission(context, it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) ask.launch(missing.toTypedArray())
    }
    BackKey(LinkTheme.scheme.label1, onBack, Modifier.padding(horizontal = Tok.Space.p12.dp))
    Column(Modifier.weight(1f).padding(horizontal = Tok.Space.p16.dp)) {
        Heading(stringResource(R.string.pair_title), stringResource(R.string.pair_scan_body))
        Viewfinder(granted && failure == null, Modifier.padding(top = Tok.Space.p20.dp)) {
            when {
                // After a failure the scanner waits for a tap, so a code still in view does not pair again at once.
                failure != null ->
                    FinderCard(Icons.Alert, stringResource(R.string.pair_failed), failure) {
                        Capsule(stringResource(R.string.scan_again), onScanAgain, icon = Icons.Refresh, kind = CapsuleKind.Primary)
                    }
                !granted ->
                    FinderCard(Icons.Qr, null, stringResource(R.string.scan_camera_needed)) {
                        Capsule(stringResource(R.string.scan_camera_allow), { ask.launch(wanted) }, kind = CapsuleKind.Primary)
                    }
                else -> {
                    QrScanner(onPairingUri, Modifier.fillMaxSize())
                    Looking()
                }
            }
        }
        Spacer(Modifier.weight(1f))
        Capsule(
            stringResource(R.string.scan_type_code),
            onType,
            Modifier.fillMaxWidth().padding(top = Tok.Space.p16.dp, bottom = Tok.Space.p12.dp),
            icon = Icons.Kbd,
            large = true,
        )
    }
}

/** The camera square: dark in both themes, since the feed is content; four corner marks frame the code. */
@Composable
private fun Viewfinder(
    marks: Boolean,
    modifier: Modifier = Modifier,
    content: @Composable BoxScope.() -> Unit,
) {
    val dark = Tok.Scheme.dark
    val mark = dark.label1
    Box(
        modifier
            .fillMaxWidth()
            .aspectRatio(1f)
            .clip(RoundedCornerShape(Tok.Radius.r24.dp))
            .background(Brush.radialGradient(listOf(dark.fill2, dark.solidRegular))),
        contentAlignment = Alignment.Center,
    ) {
        content()
        // A card in the viewfinder covers the marks.
        if (!marks) return@Box
        Canvas(Modifier.fillMaxSize()) {
            val stroke =
                Tok.Space.p4.dp
                    .toPx()
            val arm =
                Tok.Phone.finderCorner.dp
                    .toPx() - stroke / 2
            val r =
                Tok.Radius.r12.dp
                    .toPx()
            val inset = size.minDimension * Tok.Phone.finderInset + stroke / 2
            for ((dx, dy) in listOf(1f to 1f, -1f to 1f, 1f to -1f, -1f to -1f)) {
                val x = if (dx > 0) inset else size.width - inset
                val y = if (dy > 0) inset else size.height - inset
                val corner =
                    Path().apply {
                        moveTo(x, y + arm * dy)
                        lineTo(x, y + r * dy)
                        arcTo(
                            Rect(Offset(if (dx > 0) x else x - 2 * r, if (dy > 0) y else y - 2 * r), Size(2 * r, 2 * r)),
                            if (dx > 0) 180f else 0f,
                            QUARTER * dx * dy,
                            false,
                        )
                        lineTo(x + arm * dx, y)
                    }
                drawPath(corner, mark, style = Stroke(stroke, cap = StrokeCap.Round))
            }
        }
    }
}

@Composable
private fun BoxScope.Looking() {
    val dark = Tok.Scheme.dark
    Row(
        Modifier
            .align(Alignment.BottomCenter)
            .padding(bottom = Tok.Space.p16.dp)
            .height(Tok.Phone.chip.dp)
            .clip(CircleShape)
            .background(dark.scrim)
            .padding(horizontal = Tok.Phone.pillPad.dp),
        horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Symbol(Icons.Qr, dark.label1, size = Tok.Size.symbol.dp)
        Text(
            stringResource(R.string.scan_looking),
            LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold),
            color = dark.label1,
            maxLines = 1,
        )
    }
}

/** A view card inside the viewfinder, 12 in from its edges: why the camera is not reading, and the way on. */
@Composable
private fun FinderCard(
    icon: ImageVector,
    title: String?,
    body: String,
    action: @Composable () -> Unit,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    Column(
        Modifier
            .fillMaxSize()
            .padding(Tok.Space.p12.dp)
            .clip(RoundedCornerShape(Tok.Radius.r16.dp))
            .background(s.view)
            .padding(Tok.Space.p16.dp),
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Symbol(icon, if (title != null) s.dangerText else s.label2, size = Tok.Size.controlS.dp)
        if (title != null) Text(title, t.headline, align = TextAlign.Center, maxLines = 2)
        Text(body, t.footnote, color = s.label2, align = TextAlign.Center, modifier = Modifier.padding(bottom = Tok.Space.p8.dp))
        action()
    }
}

@Composable
private fun Heading(
    title: String,
    body: String,
    modifier: Modifier = Modifier,
    center: Boolean = false,
) {
    val align = if (center) TextAlign.Center else TextAlign.Start
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
        Text(title, LinkTheme.phoneType.large, modifier = Modifier.fillMaxWidth(), align = align)
        Text(body, LinkTheme.phoneType.paragraph, modifier = Modifier.fillMaxWidth(), color = LinkTheme.scheme.label2, align = align)
    }
}

@Composable
private fun ColumnScope.CodeEntry(
    state: PairingState,
    failure: String?,
    onCode: (String) -> Unit,
    onPair: () -> Unit,
    onBack: () -> Unit,
) {
    val s = LinkTheme.scheme
    BackKey(s.label1, onBack, Modifier.padding(horizontal = Tok.Space.p12.dp))
    Column(Modifier.weight(1f).padding(horizontal = Tok.Space.p16.dp)) {
        Heading(stringResource(R.string.pair_code_title), stringResource(R.string.pair_code_body))
        CodeCells(state.code, onCode, { if (state.canPairCode) onPair() }, Modifier.padding(top = Tok.Space.p24.dp))
        if (failure != null) Failure(failure, Modifier.padding(top = Tok.Space.p12.dp))
        Spacer(Modifier.weight(1f))
        Capsule(
            stringResource(R.string.pair_button),
            onPair,
            Modifier.fillMaxWidth().padding(top = Tok.Space.p16.dp, bottom = Tok.Space.p12.dp),
            kind = CapsuleKind.Primary,
            large = true,
            enabled = state.canPairCode,
        )
    }
}

/** The code as six cells over one text field, which keeps the keyboard, paste, and IME behaviour of a real field. */
@Composable
private fun CodeCells(
    code: String,
    onChange: (String) -> Unit,
    onDone: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val description = stringResource(R.string.pair_code_label)
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val shape = RoundedCornerShape(Tok.Radius.r12.dp)
    BasicTextField(
        value = code,
        onValueChange = onChange,
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        modifier = modifier.fillMaxWidth().focusRequester(focus).semantics { contentDescription = description },
        decorationBox = { field ->
            Box {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
                    repeat(CODE_LENGTH) { index ->
                        val current = index == code.length
                        Box(
                            Modifier
                                .weight(1f)
                                .height(Tok.Phone.codeCell.dp)
                                .clip(shape)
                                .background(s.view)
                                .border(if (current) Tok.Space.p2.dp else Dp.Hairline, if (current) s.accent else s.stroke, shape),
                            contentAlignment = Alignment.Center,
                        ) { Text(code.getOrNull(index)?.toString().orEmpty(), LinkTheme.phoneType.title1, maxLines = 1) }
                    }
                }
                // The field stays in the tree for focus, input, and the caret's position, drawn invisible.
                Box(Modifier.matchParentSize().alpha(0f)) { field() }
            }
        },
    )
}

@Composable
private fun ColumnScope.LinkConfirmation(
    failure: String?,
    onPair: () -> Unit,
    onCancel: () -> Unit,
) {
    val s = LinkTheme.scheme
    BackKey(s.label1, onCancel, Modifier.padding(horizontal = Tok.Space.p12.dp))
    Column(
        Modifier.weight(1f).padding(horizontal = Tok.Space.p16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.weight(1f))
        Box(
            Modifier
                .padding(bottom = Tok.Space.p16.dp)
                .size(Tok.Phone.heroGlyph.dp)
                .clip(RoundedCornerShape(Tok.Radius.r24.dp))
                .background(s.accent.copy(alpha = Tok.Phone.glyphTint)),
            contentAlignment = Alignment.Center,
        ) { Symbol(Icons.Laptop, s.accentText, size = Tok.Phone.heroSymbol.dp) }
        Heading(stringResource(R.string.pair_link_title), stringResource(R.string.pair_link_body), center = true)
        if (failure != null) Failure(failure, Modifier.padding(top = Tok.Space.p12.dp))
        Spacer(Modifier.weight(1f))
        Column(Modifier.padding(bottom = Tok.Space.p12.dp), verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
            Capsule(stringResource(R.string.pair_button), onPair, Modifier.fillMaxWidth(), kind = CapsuleKind.Primary, large = true)
            Capsule(stringResource(R.string.cancel), onCancel, Modifier.fillMaxWidth(), large = true)
        }
    }
}

@Composable
private fun ColumnScope.Busy() {
    val s = LinkTheme.scheme
    Column(
        Modifier.weight(1f).fillMaxWidth().padding(horizontal = Tok.Space.p16.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier
                .padding(bottom = Tok.Space.p16.dp)
                .size(Tok.Phone.orb.dp)
                .background(s.accent.copy(alpha = Tok.Phone.orbRing), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier.size(Tok.Phone.heroGlyph.dp).background(s.accent.copy(alpha = Tok.Phone.orbTint), CircleShape),
                contentAlignment = Alignment.Center,
            ) { Symbol(Icons.Laptop, s.accentText, size = Tok.Phone.heroSymbol.dp) }
        }
        Heading(stringResource(R.string.pairing_progress), stringResource(R.string.pairing_body), center = true)
    }
}

private const val QUARTER = 90f

/** Why pairing did not work, in danger text after an alert symbol. */
@Composable
private fun Failure(
    text: String,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp)) {
        Symbol(Icons.Alert, s.dangerText, Modifier.padding(top = Tok.Space.p2.dp), size = Tok.Size.symbol.dp)
        Text(text, LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold), color = s.dangerText)
    }
}
