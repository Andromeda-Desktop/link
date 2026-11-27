package org.andromeda.link.control

import android.app.Activity
import android.graphics.SurfaceTexture
import android.os.SystemClock
import android.view.Surface
import android.view.TextureView
import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.AwaitPointerEventScope
import androidx.compose.ui.input.pointer.PointerEvent
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.andromeda.link.R
import org.andromeda.link.core.domain.MIRROR_UNIT
import org.andromeda.link.core.domain.PointerButton
import org.andromeda.link.core.domain.RemoteInput
import org.andromeda.link.core.domain.RemoteKey
import org.andromeda.link.ui.kit.BackKey
import org.andromeda.link.ui.kit.Capsule
import org.andromeda.link.ui.kit.CapsuleKind
import org.andromeda.link.ui.kit.GroupCard
import org.andromeda.link.ui.kit.Icons
import org.andromeda.link.ui.kit.PageHeader
import org.andromeda.link.ui.kit.RoundKey
import org.andromeda.link.ui.kit.Segmented
import org.andromeda.link.ui.kit.Sheet
import org.andromeda.link.ui.kit.SwitchRow
import org.andromeda.link.ui.kit.Symbol
import org.andromeda.link.ui.kit.Text
import org.andromeda.link.ui.kit.ValueRow
import org.andromeda.link.ui.text
import org.andromeda.link.ui.theme.LinkTheme
import org.andromeda.link.ui.theme.Tok
import org.andromeda.link.ui.theme.elevation
import kotlin.math.abs
import kotlin.math.roundToInt

/** What the Viewer shows under its picture, one at a time. */
enum class ViewerMode {
    Pad,
    Keys,
    Typing,
    Sources,
    More,
    ;

    /** [next] when the Viewer can go there from here, else this mode: typing opens only from the keys. */
    fun to(next: ViewerMode): ViewerMode = if (next == Typing && this != Keys) this else next
}

/** The picture's transform: [scale] screen px per desktop px, [offset] of the desktop's top left in the view. */
@Immutable
data class Camera(
    val scale: Float,
    val offset: Offset,
)

/** Camera rules for a [view] showing a [desk] of desktop px: fit, follow, the middle band, pinch limits. */
@Immutable
class Lens(
    val view: Size,
    val desk: Size,
    private val zoom: Float,
) {
    val fit = minOf(view.width / desk.width, view.height / desk.height)
    private val follow = maxOf(fit, view.width / desk.width) * zoom

    fun whole() = clamp(Camera(fit, Offset.Zero))

    /** [zoom] x the width fit around [pointer] (a fraction of the desktop). */
    fun follow(pointer: Offset) = clamp(Camera(follow, Offset(view.width / 2, view.height / 2) - at(pointer, follow)))

    fun isWhole(camera: Camera) = camera.scale <= fit * WHOLE_SLACK

    /** The camera moves only as far as keeps [pointer] inside the middle band of the view. */
    fun keep(
        camera: Camera,
        pointer: Offset,
    ): Camera {
        val p = camera.offset + at(pointer, camera.scale)
        val bx = view.width * (1 - Tok.Phone.followBand) / 2
        val by = view.height * (1 - Tok.Phone.followBand) / 2
        val dx = (bx - p.x).coerceAtLeast(0f) - (p.x - (view.width - bx)).coerceAtLeast(0f)
        val dy = (by - p.y).coerceAtLeast(0f) - (p.y - (view.height - by)).coerceAtLeast(0f)
        return clamp(camera.copy(offset = camera.offset + Offset(dx, dy)))
    }

    /** Zooms by [factor] around [centroid] and moves by [moved]; the place under the fingers stays under them. */
    fun pinch(
        camera: Camera,
        centroid: Offset,
        factor: Float,
        moved: Offset,
    ): Camera {
        val scale = (camera.scale * factor).coerceIn(fit, fit * Tok.Phone.maxZoom)
        return clamp(Camera(scale, centroid - (centroid - camera.offset) * (scale / camera.scale) + moved))
    }

    fun pan(
        camera: Camera,
        by: Offset,
    ) = clamp(camera.copy(offset = camera.offset + by))

    /** The desktop fraction under [at] on the view, on the picture. */
    fun fraction(
        camera: Camera,
        at: Offset,
    ): Offset {
        val local = (at - camera.offset) / camera.scale
        return Offset((local.x / desk.width).coerceIn(0f, 1f), (local.y / desk.height).coerceIn(0f, 1f))
    }

    /** Where [pointer] is on the view. */
    fun onView(
        camera: Camera,
        pointer: Offset,
    ) = camera.offset + at(pointer, camera.scale)

    private fun at(
        pointer: Offset,
        scale: Float,
    ) = Offset(pointer.x * desk.width * scale, pointer.y * desk.height * scale)

    private fun clamp(camera: Camera): Camera {
        fun axis(
            o: Float,
            shown: Float,
            v: Float,
        ) = if (shown <= v) (v - shown) / 2 else o.coerceIn(v - shown, 0f)
        val s = camera.scale
        return Camera(s, Offset(axis(camera.offset.x, desk.width * s, view.width), axis(camera.offset.y, desk.height * s, view.height)))
    }
}

/**
 * A desktop's screen above the Remote's pad: the picture is for seeing and aiming (tap clicks there, a hold grabs,
 * one finger pans, a pinch zooms), the pad is the Remote's touchpad. Round keys open the keys, the sources and
 * Mute and End. Landscape gives the picture the whole screen.
 */
@Composable
fun ViewerScreen(
    viewModel: ViewerViewModel,
    input: RemoteViewModel,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val desk by input.desk.collectAsStateWithLifecycle()
    val held by input.held.collectAsStateWithLifecycle()
    val shortcuts by input.shortcuts.collectAsStateWithLifecycle()
    val touchpad by input.touchpad.collectAsStateWithLifecycle()
    val actions: RemoteActions = input
    val haptic = LocalHapticFeedback.current
    LaunchedEffect(input) { input.ticks.collect { haptic.performHapticFeedback(HapticFeedbackType.ContextClick) } }
    val leave = {
        viewModel.leave()
        onBack()
    }
    LaunchedEffect(Unit) { viewModel.open() }
    var mode by rememberSaveable { mutableStateOf(ViewerMode.Pad) }
    val go = { next: ViewerMode -> mode = mode.to(next) }
    BackHandler {
        when (mode) {
            ViewerMode.Pad -> leave()
            ViewerMode.Typing -> go(ViewerMode.Keys)
            else -> go(ViewerMode.Pad)
        }
    }
    val view = LocalView.current
    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }
    var surface by remember { mutableStateOf<StreamSurface?>(null) }
    val muted by rememberUpdatedState(state.muted)
    val stream = state.stream
    // One decoder at a time: a surface takes a new one only after the old one let it go.
    val decoding = remember { Mutex() }
    LaunchedEffect(stream, surface) {
        val target = surface
        if (stream != null && target != null) {
            decoding.withLock {
                if (target.take()) {
                    try {
                        play(stream, target.surface) { muted }
                    } finally {
                        target.drop()
                    }
                }
            }
        }
    }
    val live = stream != null
    val canInput = desk.input && live
    LaunchedEffect(canInput) { if (!canInput && (mode == ViewerMode.Keys || mode == ViewerMode.Typing)) go(ViewerMode.Pad) }

    var px by rememberSaveable { mutableFloatStateOf(CENTRE) }
    var py by rememberSaveable { mutableFloatStateOf(CENTRE) }
    val pointer = Offset(px, py)
    var viewSize by remember { mutableStateOf(IntSize.Zero) }
    BoxWithConstraints(modifier.fillMaxSize().background(LinkTheme.scheme.solidRegular)) {
        val landscape = maxWidth > maxHeight
        val lens =
            remember(viewSize, stream?.width, stream?.height, landscape) {
                if (stream == null || viewSize == IntSize.Zero) {
                    null
                } else {
                    Lens(
                        Size(viewSize.width.toFloat(), viewSize.height.toFloat()),
                        Size(stream.width.toFloat(), stream.height.toFloat()),
                        if (landscape) Tok.Phone.followZoomWide else Tok.Phone.followZoom,
                    )
                }
            }
        var camera by remember(lens) { mutableStateOf(lens?.let { if (state.window) it.whole() else it.follow(pointer) }) }
        val aimTo = { at: Offset ->
            px = at.x
            py = at.y
            input.point(RemoteInput.Point((at.x * MIRROR_UNIT).roundToInt(), (at.y * MIRROR_UNIT).roundToInt()))
            lens?.let { l -> camera?.let { camera = l.keep(it, at) } }
        }
        // On a monitor one finger on the pad moves the phone's pointer, so the picture shows it and follows it at once;
        // a window's place is unknown, so there the pad moves the desktop's.
        val padAim: ((Float, Float) -> Unit)? =
            if (state.window || stream == null) {
                null
            } else {
                { dx, dy ->
                    aimTo(
                        Offset(
                            (px + dx * PAD_SPEED / stream.width).coerceIn(0f, 1f),
                            (py + dy * PAD_SPEED / stream.height).coerceIn(0f, 1f),
                        ),
                    )
                }
            }
        val picture: @Composable (Modifier) -> Unit = { modifier ->
            Picture(
                state = state,
                deskName = desk.name,
                lens = lens,
                camera = camera,
                pointer = if (state.window) null else pointer,
                clicks = canInput && !state.window,
                pad = canInput && state.touchpad,
                touchpad = touchpad,
                padAim = padAim,
                input = actions,
                onCamera = { camera = it },
                onAim = { aimTo(it) },
                onSurface = { surface = it },
                onSize = { viewSize = it },
                immersive = landscape,
                modifier = modifier,
            )
        }
        val chip: @Composable (Modifier) -> Unit = {
            DeskChip(desk, if (desk.input) null else stringResource(R.string.viewer_view_only, desk.name), it)
        }
        val zoom: @Composable () -> Unit = {
            val l = lens
            val c = camera
            if (live && l != null && c != null) ZoomCapsule(l, c) { camera = if (l.isWhole(c)) l.follow(pointer) else l.whole() }
        }
        val keys: @Composable () -> Unit = {
            val d = if (landscape) Tok.Size.rowTwo.dp else Tok.Phone.roundKeyL.dp
            RoundKey(
                Icons.Kbd,
                stringResource(R.string.remote_keys),
                { go(if (mode == ViewerMode.Keys || mode == ViewerMode.Typing) ViewerMode.Pad else ViewerMode.Keys) },
                size = d,
                enabled = canInput,
            )
            RoundKey(
                Icons.Monitor,
                stringResource(R.string.viewer_sources),
                { go(ViewerMode.Sources) },
                size = d,
                enabled = state.sources.isNotEmpty(),
            )
            RoundKey(Icons.More, stringResource(R.string.viewer_more), { go(ViewerMode.More) }, size = d)
        }
        if (landscape) {
            Immersive()
            Box(Modifier.fillMaxSize()) {
                picture(Modifier.fillMaxSize())
                Row(
                    Modifier
                        .align(Alignment.TopStart)
                        .padding(
                            start = Tok.Space.p8.dp,
                            top = Tok.Space.p8.dp,
                            end = (Tok.Space.p12 + Tok.Size.rowTwo + Tok.Space.p12).dp,
                        ),
                    horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val s = LinkTheme.scheme
                    val h = Tok.Phone.hit.dp
                    BackKey(s.label1, leave, Modifier.elevation(Tok.Elevation.e3, h / 2).clip(CircleShape).background(s.view))
                    chip(Modifier.weight(1f, fill = false))
                    zoom()
                }
                // Landscape keys stand in a column at the right edge, Keys on top, More at the bottom.
                Column(
                    Modifier.align(Alignment.CenterEnd).fillMaxHeight().padding(Tok.Space.p12.dp),
                    verticalArrangement = Arrangement.SpaceBetween,
                ) { keys() }
                when (mode) {
                    ViewerMode.Keys ->
                        CompactKeys(
                            actions,
                            held,
                            desk.name,
                            { go(ViewerMode.Typing) },
                            Modifier.align(Alignment.BottomCenter),
                        )
                    ViewerMode.Typing ->
                        TypingBar(
                            actions,
                            held,
                            shortcuts,
                            desk.name,
                            { go(ViewerMode.Keys) },
                            Modifier.align(Alignment.BottomCenter).imePadding(),
                        )
                    else -> Unit
                }
                Sheets(state, mode, go, leave, viewModel::select, viewModel::toggleMute, viewModel::setTouchpad)
            }
        } else {
            Box(Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().statusBarsPadding().imePadding()) {
                    PageHeader(desk.name, leave, Modifier.padding(end = Tok.Space.p8.dp)) {
                        if (!desk.input) chip(Modifier.weight(1f, fill = false))
                        zoom()
                    }
                    picture(Modifier.fillMaxWidth().fillMaxHeight(Tok.Phone.picture))
                    when (mode) {
                        ViewerMode.Keys -> CompactKeys(actions, held, desk.name, { go(ViewerMode.Typing) }, Modifier.weight(1f))
                        ViewerMode.Typing -> {
                            Spacer(Modifier.weight(1f))
                            TypingBar(actions, held, shortcuts, desk.name, { go(ViewerMode.Keys) })
                        }
                        else ->
                            Pad(
                                input = actions,
                                deskName = desk.name,
                                canInput = canInput,
                                pictureIsPad = state.touchpad,
                                raw = touchpad,
                                aim = padAim,
                                modifier = Modifier.weight(1f).fillMaxWidth(),
                            ) {
                                Row(horizontalArrangement = Arrangement.spacedBy(Tok.Space.p12.dp)) { keys() }
                            }
                    }
                }
                Sheets(state, mode, go, leave, viewModel::select, viewModel::toggleMute, viewModel::setTouchpad)
            }
        }
    }
}

/** The system bars hide while the picture has the whole screen, and come back with a swipe from an edge. */
@Composable
private fun Immersive() {
    val view = LocalView.current
    DisposableEffect(view) {
        val window = (view.context as? Activity)?.window
        val bars = window?.let { WindowCompat.getInsetsController(it, view) }
        bars?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        bars?.hide(WindowInsetsCompat.Type.systemBars())
        onDispose { bars?.show(WindowInsetsCompat.Type.systemBars()) }
    }
}

/** The zoom capsule: the zoom over the fit while zoomed (tap: the whole desktop), "Follow" on the whole (tap: zoom in). */
@Composable
private fun ZoomCapsule(
    lens: Lens,
    camera: Camera,
    onClick: () -> Unit,
) {
    val s = LinkTheme.scheme
    val whole = lens.isWhole(camera)
    val tenths = (camera.scale / lens.fit * TENTHS).roundToInt()
    val label =
        if (whole) {
            stringResource(R.string.viewer_follow)
        } else if (tenths % 10 ==
            0
        ) {
            "${tenths / 10}×"
        } else {
            "${tenths / 10}.${tenths % 10}×"
        }
    val hint = stringResource(if (whole) R.string.viewer_follow_hint else R.string.viewer_whole_hint)
    val h = Tok.Phone.hit.dp
    Box(
        Modifier
            .height(h)
            .widthIn(min = Tok.Size.rowTwo.dp)
            .elevation(Tok.Elevation.e3, h / 2)
            .clip(CircleShape)
            .background(s.view)
            .border(Dp.Hairline, s.stroke, CircleShape)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = hint }
            .padding(horizontal = Tok.Phone.pillPad.dp),
        contentAlignment = Alignment.Center,
    ) { Text(label, LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.Bold), maxLines = 1) }
}

/** The picture, the phone's pointer, the hold ring and the minimap; or why there is no picture. */
@Composable
private fun Picture(
    state: ViewerState,
    deskName: String,
    lens: Lens?,
    camera: Camera?,
    pointer: Offset?,
    clicks: Boolean,
    /** Touchpad mode: the picture is the Remote's touchpad. */
    pad: Boolean,
    touchpad: Boolean,
    padAim: ((Float, Float) -> Unit)?,
    input: RemoteActions,
    onCamera: (Camera) -> Unit,
    onAim: (Offset) -> Unit,
    onSurface: (StreamSurface?) -> Unit,
    onSize: (IntSize) -> Unit,
    immersive: Boolean,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    // Landscape: the picture fills the screen edge to edge, square and flat.
    val shape = if (immersive) RectangleShape else RoundedCornerShape(bottomStart = Tok.Radius.r24.dp, bottomEnd = Tok.Radius.r24.dp)
    val stream = state.stream
    val description = stringResource(R.string.viewer_picture)
    // The place a finger holds on the picture, and whether the hold became a grab.
    var aim by remember { mutableStateOf<Offset?>(null) }
    var grabbed by remember { mutableStateOf(false) }
    val currentCamera by rememberUpdatedState(camera)
    val minimap = lens != null && camera != null && !lens.isWhole(camera) && aim == null
    val minimapShown by rememberUpdatedState(minimap)
    // The minimap's picture: the whole stream, copied small from the TextureView at most every THUMB_MS, only while shown.
    var thumb by remember { mutableStateOf<ImageBitmap?>(null) }
    var texture by remember { mutableStateOf<TextureView?>(null) }
    var thumbAt by remember { mutableLongStateOf(0L) }
    val thumbWidth = with(LocalDensity.current) { (Tok.Phone.minimap - 2 * Tok.Space.p4).dp.roundToPx() }
    val snap = {
        val view = texture
        if (view != null && lens != null && thumbWidth > 0) {
            thumbAt = SystemClock.uptimeMillis()
            val height = (thumbWidth * lens.desk.height / lens.desk.width).toInt().coerceAtLeast(1)
            view.getBitmap(thumbWidth, height)?.let { thumb = it.asImageBitmap() }
        }
    }
    val currentSnap by rememberUpdatedState(snap)
    LaunchedEffect(minimap) { if (minimap) currentSnap() }
    Box(
        modifier
            .then(if (immersive) Modifier else Modifier.elevation(Tok.Elevation.e2, Tok.Radius.r24.dp))
            .clip(shape)
            .background(s.fill2)
            .onSizeChanged(onSize),
    ) {
        if (stream != null && lens != null && camera != null) {
            val density = LocalDensity.current
            val fitted = with(density) { Modifier.size((lens.desk.width * lens.fit).toDp(), (lens.desk.height * lens.fit).toDp()) }
            AndroidView(
                factory = { context ->
                    TextureView(context).also { texture = it }.apply {
                        surfaceTextureListener =
                            object : TextureView.SurfaceTextureListener {
                                private var current: StreamSurface? = null

                                override fun onSurfaceTextureAvailable(
                                    texture: SurfaceTexture,
                                    w: Int,
                                    h: Int,
                                ) = onSurface(StreamSurface(texture).also { current = it })

                                override fun onSurfaceTextureSizeChanged(
                                    texture: SurfaceTexture,
                                    w: Int,
                                    h: Int,
                                ) = Unit

                                // False: the texture stays alive until the decoder that draws into it has stopped.
                                override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
                                    onSurface(null)
                                    current?.viewGone()
                                    current = null
                                    return false
                                }

                                override fun onSurfaceTextureUpdated(texture: SurfaceTexture) {
                                    if (minimapShown && SystemClock.uptimeMillis() - thumbAt >= THUMB_MS) currentSnap()
                                }
                            }
                    }
                },
                // Laid out once at the fit; zoom and pan are the layer's transform, never a new layout.
                modifier =
                    fitted.graphicsLayer {
                        val c = currentCamera ?: return@graphicsLayer
                        transformOrigin = TransformOrigin(0f, 0f)
                        scaleX = c.scale / lens.fit
                        scaleY = c.scale / lens.fit
                        translationX = c.offset.x
                        translationY = c.offset.y
                    },
            )
            Box(
                Modifier
                    .fillMaxSize()
                    .semantics { contentDescription = description }
                    .then(
                        if (pad) {
                            Modifier.touchpad(input, touchpad, padAim)
                        } else {
                            Modifier.pointerInput(lens, clicks) {
                                pictureGestures(
                                    lens = lens,
                                    clicks = clicks,
                                    camera = { currentCamera ?: lens.whole() },
                                    input = input,
                                    onCamera = onCamera,
                                    onAim = onAim,
                                    onHold = { at, grab ->
                                        aim = at
                                        grabbed = grab
                                    },
                                )
                            }
                        },
                    ),
            )
            if (pointer != null && aim == null) PointerArrow(lens.onView(camera, pointer))
            aim?.let {
                HoldRing(it, grabbed)
                if (grabbed) GrabTag(it)
            }
            if (minimap) {
                // Landscape: left of the key column (12 + key + 12), as the mockup's .ov right: 80.
                val end = if (immersive) Tok.Space.p12 + Tok.Size.rowTwo + Tok.Space.p12 else Tok.Space.p8
                Minimap(lens, camera, thumb, Modifier.align(Alignment.BottomEnd).padding(end = end.dp, bottom = Tok.Space.p8.dp))
            }
        } else {
            val ended = state.phase as? ViewerPhase.Ended
            Column(
                Modifier.align(Alignment.Center).padding(horizontal = Tok.Space.p32.dp),
                verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                val t = LinkTheme.phoneType
                if (ended == null) {
                    Text(stringResource(R.string.viewer_waiting, deskName), t.body, color = s.label2, align = TextAlign.Center)
                } else {
                    Text(stringResource(R.string.viewer_stopped, deskName), t.body, color = s.label2, align = TextAlign.Center)
                    val why = ended.failure?.text(LocalResources.current) ?: ended.reason?.ifEmpty { null }
                    if (why != null) Text(why, t.footnote, color = s.label3, align = TextAlign.Center)
                }
            }
        }
    }
}

/**
 * One finger pans the picture and never sends; a tap clicks where it lands; a still finger for [HOLD_MS] grabs there
 * (lift: right click, move: drag). Two fingers lock to a pinch or a scroll within their first 12 px. Without [clicks]
 * the picture only pans and zooms.
 */
private suspend fun PointerInputScope.pictureGestures(
    lens: Lens,
    clicks: Boolean,
    camera: () -> Camera,
    input: RemoteActions,
    onCamera: (Camera) -> Unit,
    onAim: (Offset) -> Unit,
    onHold: (Offset?, Boolean) -> Unit,
) {
    // The last tap: a second press soon after it at the same place drags (tap, then hold and move).
    var tapAt = 0L
    var tapPosition = Offset.Zero
    awaitEachGesture {
        pictureGesture(lens, clicks, camera, input, onCamera, onAim, onHold, tapAt, tapPosition) { at, time ->
            tapAt = time
            tapPosition = at
        }
    }
}

/**
 * One gesture on the picture: tap = click; one finger moving, at once or after a pause, pans and never sends;
 * hold still then lift = right click; tap then hold = drag from there (lift without moving = double click).
 */
private suspend fun AwaitPointerEventScope.pictureGesture(
    lens: Lens,
    clicks: Boolean,
    camera: () -> Camera,
    input: RemoteActions,
    onCamera: (Camera) -> Unit,
    onAim: (Offset) -> Unit,
    onHold: (Offset?, Boolean) -> Unit,
    tapAt: Long,
    tapPosition: Offset,
    onTap: (Offset, Long) -> Unit,
) {
    val down = awaitFirstDown()
    val slop = viewConfiguration.touchSlop
    if (clicks &&
        down.uptimeMillis - tapAt <= viewConfiguration.doubleTapTimeoutMillis &&
        (down.position - tapPosition).getDistance() <= DRAG_NEAR * slop
    ) {
        drag(lens, camera, input, onAim, onHold, down)
        return
    }
    var travelled = 0f
    if (clicks) onHold(down.position, false)
    val early =
        withTimeoutOrNull(HOLD_MS) {
            var result: Early? = null
            do {
                val event = awaitPointerEvent()
                val pressed = event.changes.filter { it.pressed }
                event.changes.forEach { it.consume() }
                when {
                    pressed.isEmpty() -> result = Early.Up
                    pressed.size > 1 -> result = Early.Two
                    else -> {
                        travelled += pressed[0].positionChange().getDistance()
                        if (travelled >= slop) result = Early.Pan
                    }
                }
            } while (result == null)
            result
        }
    val held = early == null && clicks
    if (!held) onHold(null, false)
    when {
        early == Early.Up -> {
            if (clicks) {
                onAim(lens.fraction(camera(), down.position))
                input.send(RemoteInput.Click(PointerButton.Left))
                onTap(down.position, down.uptimeMillis)
            }
        }
        held -> {
            // The ring is full: lifting here right-clicks; moving instead pans like any one-finger move.
            input.tick()
            var moved = 0f
            var event: PointerEvent
            do {
                event = awaitPointerEvent()
                val change = event.changes.firstOrNull { it.id == down.id } ?: event.changes.first()
                moved += change.positionChange().getDistance()
                event.changes.forEach { it.consume() }
            } while (event.changes.any { it.pressed } && moved < slop && event.changes.count { it.pressed } < 2)
            onHold(null, false)
            if (event.changes.none { it.pressed }) {
                onAim(lens.fraction(camera(), down.position))
                input.send(RemoteInput.Click(PointerButton.Right))
            } else {
                twoOrPan(lens, clicks, camera, input, onCamera)
            }
        }
        else -> twoOrPan(lens, clicks, camera, input, onCamera)
    }
}

/** Tap, then hold: presses where the finger is and drags until it lifts; no move = the second click of a double click. */
private suspend fun AwaitPointerEventScope.drag(
    lens: Lens,
    camera: () -> Camera,
    input: RemoteActions,
    onAim: (Offset) -> Unit,
    onHold: (Offset?, Boolean) -> Unit,
    down: PointerInputChange,
) {
    onHold(down.position, true)
    input.tick()
    onAim(lens.fraction(camera(), down.position))
    input.send(RemoteInput.Press(PointerButton.Left))
    do {
        val event = awaitPointerEvent()
        val change = event.changes.firstOrNull { it.id == down.id } ?: event.changes.first()
        event.changes.forEach { it.consume() }
        if (change.pressed) onAim(lens.fraction(camera(), change.position))
    } while (event.changes.any { it.pressed })
    onHold(null, false)
    input.send(RemoteInput.Release(PointerButton.Left))
}

private enum class Early { Up, Two, Pan }

/**
 * A TextureView's surface shared with the decoder. It is released only when both have let go: a decoder still
 * rendering into a released surface makes MediaCodec throw. Main thread only.
 */
class StreamSurface(
    private val texture: SurfaceTexture,
) {
    val surface = Surface(texture)
    private var phase = Phase.Idle

    private enum class Phase { Idle, Decoding, Orphaned, Released }

    /** A decoder starts on it; false once the view let go. */
    fun take(): Boolean = (phase == Phase.Idle).also { if (it) phase = Phase.Decoding }

    /** The decoder stopped. */
    fun drop() {
        phase =
            when (phase) {
                Phase.Decoding -> Phase.Idle
                Phase.Orphaned -> release()
                else -> error("drop in $phase")
            }
    }

    /** The view let go. */
    fun viewGone() {
        phase =
            when (phase) {
                Phase.Idle -> release()
                Phase.Decoding -> Phase.Orphaned
                else -> error("viewGone in $phase")
            }
    }

    private fun release(): Phase {
        surface.release()
        texture.release()
        return Phase.Released
    }
}

/** After the first moves: one finger pans; two fingers lock to a pinch or a scroll and keep it until they lift. */
private suspend fun AwaitPointerEventScope.twoOrPan(
    lens: Lens,
    clicks: Boolean,
    camera: () -> Camera,
    input: RemoteActions,
    onCamera: (Camera) -> Unit,
) {
    val lock =
        Tok.Space.p12.dp
            .toPx()
    var pinch: Boolean? = null
    var spread0 = 0f
    var travel = 0f
    do {
        val event = awaitPointerEvent()
        val pressed = event.changes.filter { it.pressed }
        if (pressed.size >= 2) {
            val a = pressed[0]
            val b = pressed[1]
            val centroid = (a.position + b.position) / 2f
            val previous = (a.previousPosition + b.previousPosition) / 2f
            val spread = (a.position - b.position).getDistance()
            val before = (a.previousPosition - b.previousPosition).getDistance()
            if (spread0 == 0f) spread0 = before
            travel += (centroid - previous).getDistance()
            if (pinch == null) {
                pinch =
                    when {
                        abs(spread - spread0) >= lock -> true
                        travel >= lock -> false
                        else -> null
                    }
            }
            when (pinch) {
                true -> if (before > 0f) onCamera(lens.pinch(camera(), centroid, spread / before, centroid - previous))
                false ->
                    if (clicks) {
                        input.scroll(centroid.x - previous.x, centroid.y - previous.y)
                    } else {
                        onCamera(
                            lens.pan(
                                camera(),
                                centroid - previous,
                            ),
                        )
                    }
                null -> Unit
            }
        } else if (pressed.size == 1 && pinch == null) {
            onCamera(lens.pan(camera(), pressed[0].positionChange()))
        }
        event.changes.forEach { it.consume() }
    } while (pressed.isNotEmpty())
}

/** The accent arrow the phone draws at its own pointer; its tip is at [at]. */
@Composable
private fun PointerArrow(at: Offset) {
    val s = LinkTheme.scheme
    val arrow =
        remember(s.accent, s.view) {
            ImageVector
                .Builder("pointer", 24.dp, 24.dp, 24f, 24f)
                .addPath(
                    PathParser().parsePathString(ARROW).toNodes(),
                    fill = SolidColor(s.accent),
                    stroke = SolidColor(s.view),
                    strokeLineWidth = ARROW_STROKE,
                ).build()
        }
    val size = Tok.Phone.pointer.dp
    Image(
        rememberVectorPainter(arrow),
        contentDescription = null,
        modifier =
            Modifier.size(size).layout { measurable, constraints ->
                val placeable = measurable.measure(constraints)
                // The arrow's tip sits at (4, 2.5) of its 24 grid.
                val tip = Offset(placeable.width * ARROW_TIP_X, placeable.height * ARROW_TIP_Y)
                layout(placeable.width, placeable.height) { placeable.place((at.x - tip.x).roundToInt(), (at.y - tip.y).roundToInt()) }
            },
    )
}

/** Nothing for 150 ms, then a ring whose arc fills until the grab; a grab fills it and says what lift and move do. */
@Composable
private fun HoldRing(
    at: Offset,
    grabbed: Boolean,
) {
    val s = LinkTheme.scheme
    val progress = remember(at) { Animatable(0f) }
    LaunchedEffect(at) {
        delay(RING_DELAY_MS)
        progress.animateTo(1f, tween(RING_FILL_MS, easing = LinearEasing))
    }
    val d = Tok.Phone.holdRing.dp
    Box(
        Modifier
            .size(d)
            .centeredAt(at)
            .alpha(if (grabbed || progress.value > 0f) 1f else 0f)
            .drawBehind {
                val w =
                    Tok.Space.p2.dp
                        .toPx()
                val inset = Size(size.width - w, size.height - w)
                val topLeft = Offset(w / 2, w / 2)
                if (grabbed) drawCircle(s.accent.copy(alpha = Tok.Phone.glyphTint))
                drawArc(s.accent.copy(alpha = Tok.Phone.offAlpha), 0f, FULL_TURN, false, topLeft, inset, style = Stroke(w))
                drawArc(s.accent, -QUARTER_TURN, FULL_TURN * if (grabbed) 1f else progress.value, false, topLeft, inset, style = Stroke(w))
            },
    )
}

/** "Grabbed" over the finger while a hold drags. */
@Composable
private fun GrabTag(at: Offset) {
    val s = LinkTheme.scheme
    run {
        val h = Tok.Size.controlL.dp
        Box(
            Modifier
                .layout { measurable, constraints ->
                    val placeable = measurable.measure(constraints.copy(minWidth = 0, minHeight = 0))
                    val gap = (Tok.Size.controlL + Tok.Space.p4).dp.toPx()
                    layout(placeable.width, placeable.height) {
                        placeable.place((at.x - placeable.width / 2).roundToInt(), (at.y - gap - placeable.height).roundToInt())
                    }
                }.height(h)
                .clip(CircleShape)
                .background(s.label1)
                .padding(horizontal = Tok.Space.p16.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                stringResource(R.string.viewer_grab),
                LinkTheme.phoneType.footnote.copy(fontWeight = FontWeight.SemiBold),
                color = s.view,
                maxLines = 1,
            )
        }
    }
}

private fun Modifier.centeredAt(at: Offset) =
    layout { measurable, constraints ->
        val placeable = measurable.measure(constraints)
        layout(placeable.width, placeable.height) {
            placeable.place((at.x - placeable.width / 2).roundToInt(), (at.y - placeable.height / 2).roundToInt())
        }
    }

/** The whole desktop small, with an accent box round the part on screen; it moves 1:1 with the camera. */
@Composable
private fun Minimap(
    lens: Lens,
    camera: Camera,
    thumb: ImageBitmap?,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val outer = RoundedCornerShape(Tok.Radius.r12.dp)
    Box(
        modifier
            .width(Tok.Phone.minimap.dp)
            .elevation(Tok.Elevation.e3, Tok.Radius.r12.dp)
            .clip(outer)
            .background(s.view)
            .border(Dp.Hairline, s.stroke, outer)
            .padding(Tok.Space.p4.dp),
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(lens.desk.width / lens.desk.height)
                .clip(RoundedCornerShape(Tok.Radius.r8.dp))
                .background(s.fill2)
                .drawBehind {
                    thumb?.let { drawImage(it, dstSize = IntSize(size.width.toInt(), size.height.toInt())) }
                    val shown = lens.desk * camera.scale
                    val l = (-camera.offset.x / shown.width).coerceIn(0f, 1f)
                    val t = (-camera.offset.y / shown.height).coerceIn(0f, 1f)
                    val r = ((lens.view.width - camera.offset.x) / shown.width).coerceIn(0f, 1f)
                    val b = ((lens.view.height - camera.offset.y) / shown.height).coerceIn(0f, 1f)
                    val w =
                        Tok.Space.p2.dp
                            .toPx()
                    drawRoundRect(
                        s.accent,
                        Offset(l * size.width + w / 2, t * size.height + w / 2),
                        Size((r - l) * size.width - w, (b - t) * size.height - w),
                        CornerRadius(
                            Tok.Radius.r4.dp
                                .toPx() - w / 2,
                        ),
                        style = Stroke(w),
                    )
                },
        )
    }
}

/** The Remote's dotted pad under the picture, with three tips and the round keys; view only names itself. */
@Composable
private fun Pad(
    input: RemoteActions,
    deskName: String,
    canInput: Boolean,
    pictureIsPad: Boolean,
    raw: Boolean,
    aim: ((Float, Float) -> Unit)?,
    modifier: Modifier = Modifier,
    keys: @Composable () -> Unit,
) {
    val s = LinkTheme.scheme
    val touchpad = stringResource(R.string.remote_touchpad)
    Box(modifier) {
        Box(
            Modifier
                .fillMaxSize()
                .dots(s.fill2)
                .then(if (canInput) Modifier.touchpad(input, raw, aim) else Modifier)
                .semantics { contentDescription = touchpad },
        )
        if (canInput) {
            Column(
                Modifier.padding(start = Tok.Space.p24.dp, end = Tok.Space.p48.dp, top = Tok.Space.p24.dp),
                verticalArrangement = Arrangement.spacedBy((Tok.Space.p4 + Tok.Space.p2).dp),
            ) {
                if (!pictureIsPad) Tip("", stringResource(R.string.viewer_tip_picture), Icons.Screen)
                Tip("1", stringResource(R.string.viewer_tip_pad))
                Tip("2", stringResource(R.string.viewer_tip_two))
            }
            val context = LocalContext.current
            if (raw && needsMultiFingerGrant(context)) {
                Text(
                    stringResource(R.string.remote_multi_finger_grant, context.packageName),
                    LinkTheme.phoneType.footnote,
                    color = s.label2,
                    modifier =
                        Modifier
                            .align(Alignment.BottomCenter)
                            .navigationBarsPadding()
                            .padding(
                                start = Tok.Space.p24.dp,
                                end = Tok.Space.p24.dp,
                                bottom = (Tok.Space.p24 + Tok.Phone.roundKeyL + Tok.Space.p8).dp,
                            ),
                )
            }
        } else {
            Text(
                stringResource(R.string.viewer_off_pad, deskName),
                LinkTheme.phoneType.body,
                color = s.label2,
                modifier = Modifier.align(Alignment.Center).padding(horizontal = Tok.Space.p40.dp),
                align = TextAlign.Center,
            )
        }
        Box(
            Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding()
                .padding(bottom = Tok.Space.p24.dp),
        ) { keys() }
    }
}

/** The Viewer's keys in the pad's place: the composer, then Esc, Tab, the arrows, Enter and a held Ctrl. */
@Composable
private fun CompactKeys(
    input: RemoteActions,
    held: Set<String>,
    deskName: String,
    onType: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val s = LinkTheme.scheme
    val t = LinkTheme.phoneType
    val r = Tok.Radius.r28.dp
    val gap = Tok.Space.p4.dp
    Column(
        modifier
            .fillMaxWidth()
            .elevation(Tok.Elevation.e4, r)
            .clip(RoundedCornerShape(topStart = r, topEnd = r))
            .background(s.view)
            .navigationBarsPadding()
            .padding(start = Tok.Space.p12.dp, end = Tok.Space.p12.dp, top = Tok.Space.p8.dp, bottom = Tok.Phone.dockInset.dp),
        verticalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
    ) {
        Box(
            Modifier
                .align(Alignment.CenterHorizontally)
                .padding(bottom = Tok.Space.p4.dp)
                .size(Tok.Phone.grab.dp, Tok.Space.p4.dp)
                .clip(CircleShape)
                .background(s.fill3),
        )
        Row(
            Modifier
                .fillMaxWidth()
                .height(Tok.Phone.composer.dp)
                .clip(CircleShape)
                .background(s.fill1)
                .clickable(role = Role.Button, onClick = onType)
                .padding(start = (Tok.Space.p16 + Tok.Space.p2).dp, end = Tok.Space.p16.dp),
            horizontalArrangement = Arrangement.spacedBy(Tok.Space.p8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(stringResource(R.string.remote_type_on, deskName), t.body, color = s.label2, modifier = Modifier.weight(1f), maxLines = 1)
            Symbol(Icons.Kbd, s.label2)
        }
        val rows =
            listOf(
                listOf(
                    "Esc" to RemoteKey.Escape,
                    "Tab" to RemoteKey.Tab,
                    "↑" to RemoteKey.Up,
                    stringResource(R.string.remote_enter) to RemoteKey.Enter,
                ),
                listOf(null, "←" to RemoteKey.Left, "↓" to RemoteKey.Down, "→" to RemoteKey.Right),
            )
        for (row in rows) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(gap)) {
                for (key in row) {
                    val ctrl = key == null
                    val on = ctrl && "ctrl" in held
                    Box(
                        Modifier
                            .weight(1f)
                            .height(Tok.Phone.composer.dp)
                            .clip(RoundedCornerShape(Tok.Radius.r16.dp))
                            .background(if (on) s.accent else s.fill1)
                            .clickable(role = Role.Button) { if (key == null) input.hold("ctrl") else input.press(key.second) },
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            key?.first ?: "Ctrl",
                            t.body.copy(fontWeight = FontWeight.SemiBold),
                            color = if (on) s.onAccent else s.label1,
                            maxLines = 1,
                        )
                    }
                }
            }
        }
    }
}

/** Typing under the picture: the composer, then the key strip right on the system keyboard. */
@Composable
private fun TypingBar(
    input: RemoteActions,
    held: Set<String>,
    shortcuts: Boolean,
    deskName: String,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxWidth()) {
        Composer(input, deskName, onClose, Modifier.padding(horizontal = Tok.Space.p12.dp, vertical = Tok.Space.p8.dp))
        KeyStrip(input, held, shortcuts)
    }
}

/** The sources and the Mute / End sheets, over everything. */
@Composable
private fun BoxScope.Sheets(
    state: ViewerState,
    mode: ViewerMode,
    go: (ViewerMode) -> Unit,
    leave: () -> Unit,
    onSelect: (Int) -> Unit,
    onMute: () -> Unit,
    onTouchpad: (Boolean) -> Unit,
) {
    val s = LinkTheme.scheme
    Sheet(mode == ViewerMode.Sources, { go(ViewerMode.Pad) }, stringResource(R.string.viewer_sources)) {
        GroupCard {
            state.sources.forEachIndexed { index, source ->
                ValueRow(
                    source.name,
                    icon = if (source.window) Icons.Maxi else Icons.Monitor,
                    onClick = {
                        onSelect(index)
                        go(ViewerMode.Pad)
                    },
                ) { if (index == state.current) Symbol(Icons.Check, s.accentText, size = Tok.Size.symbolL.dp) }
            }
        }
        if (state.window) Text(stringResource(R.string.viewer_window_touchpad), LinkTheme.phoneType.footnote, color = s.label2)
    }
    Sheet(mode == ViewerMode.More, { go(ViewerMode.Pad) }, stringResource(R.string.viewer_more)) {
        Segmented(
            listOf(true to stringResource(R.string.viewer_touchpad), false to stringResource(R.string.viewer_direct)),
            state.touchpad,
            onTouchpad,
            Modifier.fillMaxWidth(),
        )
        GroupCard { SwitchRow(stringResource(R.string.viewer_sound), !state.muted, { onMute() }) }
        Capsule(stringResource(R.string.viewer_end), leave, Modifier.fillMaxWidth(), kind = CapsuleKind.Danger, large = true)
    }
}

private const val CENTRE = 0.5f
private const val PAD_SPEED = 1.5f
private const val HOLD_MS = 400L
private const val THUMB_MS = 500L
private const val DRAG_NEAR = 4f
private const val RING_DELAY_MS = 150L
private const val RING_FILL_MS = 250
private const val TENTHS = 10
private const val WHOLE_SLACK = 1.01f
private const val FULL_TURN = 360f
private const val QUARTER_TURN = 90f
private const val ARROW = "M4 2.5v17l4.6-4.4 3 6.6 3-1.4-3-6.4H18z"
private const val ARROW_STROKE = 1.6f
private const val ARROW_TIP_X = 4f / 24f
private const val ARROW_TIP_Y = 2.5f / 24f
