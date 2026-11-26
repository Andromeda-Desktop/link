package org.andromeda.link.devices.galaxy

import android.app.ActivityManager
import android.content.Context
import android.content.pm.ApplicationInfo
import android.graphics.SurfaceTexture
import android.opengl.EGL14
import android.opengl.EGLConfig
import android.opengl.EGLContext
import android.opengl.EGLDisplay
import android.opengl.EGLSurface
import android.opengl.GLES20
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import android.view.Choreographer
import android.view.TextureView
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.floor
import kotlin.math.ln
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt
import kotlin.math.tan

fun galaxyTier(context: Context): GalaxyTier {
    val am = context.getSystemService(ActivityManager::class.java)
    val low = am.isLowRamDevice || am.memoryClass <= LOW_MEMORY_CLASS_MB || am.deviceConfigurationInfo.reqGlEsVersion < GLES3
    return if (low) GalaxyTier.Low else GalaxyTier.Full
}

private const val GLES3 = 0x30000
private const val LOW_MEMORY_CLASS_MB = 192

/** Built once per process: the arrays stay so a new GL context uploads them again without a rebuild. */
object GalaxyCache {
    @Volatile private var built: Pair<GalaxyTier, GalaxyPoints>? = null

    fun get(tier: GalaxyTier): GalaxyPoints =
        built?.takeIf { it.first == tier }?.second ?: synchronized(this) {
            built?.takeIf { it.first == tier }?.second ?: buildGalaxy(tier).also { built = tier to it }
        }
}

/**
 * The Home galaxy on its own GL thread. It draws only on demand (a gesture, a fly, a size or theme change), so an
 * idle Home costs no frames. Light adds up in an HDR target at [GalaxyTier.renderScale], then one pass tone-maps it.
 */
class GalaxyView(
    context: Context,
) : TextureView(context),
    TextureView.SurfaceTextureListener {
    val tier = galaxyTier(context)
    private val thread = HandlerThread("andro-galaxy").apply { start() }
    private val gl = Handler(thread.looper)
    private val lock = Any()
    private var cam = Cam()
    private var points: GalaxyPoints? = null
    private var light = false
    private var vw = 1
    private var vh = 1
    private var pending = false

    /** Number of frames drawn; stays still while Home is idle. */
    @Volatile var frames = 0L
        private set

    init {
        surfaceTextureListener = this
        isOpaque = true
    }

    fun setPoints(p: GalaxyPoints) {
        synchronized(lock) { points = p }
        requestRender()
    }

    fun setLight(on: Boolean) {
        if (light == on) return
        light = on
        requestRender()
    }

    // ---- gestures (main thread) ----

    fun orbit(
        dx: Float,
        dy: Float,
    ) {
        stopMotion()
        val ky = -dx / density * TURN_YAW
        val kp = dy / density * TURN_PITCH
        synchronized(lock) {
            cam.yaw += ky
            cam.pitch = (cam.pitch + kp).coerceIn(-PITCH_MAX, PITCH_MAX)
        }
        velYaw = ky
        velPitch = kp
        requestRender()
    }

    fun zoom(
        cx: Float,
        cy: Float,
        factor: Float,
    ) {
        stopMotion()
        synchronized(lock) { zoomAt(cx, cy, 1.0 / factor) }
        requestRender()
    }

    /** Lets the last drag speed carry the turn on and slow down. */
    fun release() {
        if (abs(velYaw) + abs(velPitch) > SPIN_MIN) startMotion()
    }

    fun home() {
        synchronized(lock) {
            val to = homeCam()
            var dy = to.yaw - cam.yaw
            dy = ((dy + PI) % TAU + TAU) % TAU - PI
            tween = Tween(SystemClock.uptimeMillis(), cam.copy(), to.copy(yaw = cam.yaw + dy))
        }
        velYaw = 0.0
        velPitch = 0.0
        startMotion()
    }

    private val density = resources.displayMetrics.density
    private val debuggable = context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0
    private var velYaw = 0.0
    private var velPitch = 0.0
    private var tween: Tween? = null
    private var moving = false
    private val tick: Choreographer.FrameCallback =
        Choreographer.FrameCallback { now ->
            var busy = false
            synchronized(lock) {
                val tw = tween
                if (tw != null) {
                    val k = min(1.0, (now / 1_000_000 - tw.t0).toDouble() / FLY_MS)
                    val e = if (k < .5) 4 * k * k * k else 1 - (-2 * k + 2).pow(3) / 2
                    val a = tw.a
                    val b = tw.b

                    fun l(
                        x: Double,
                        y: Double,
                    ) = x + (y - x) * e
                    cam =
                        Cam(
                            doubleArrayOf(l(a.t[0], b.t[0]), l(a.t[1], b.t[1]), l(a.t[2], b.t[2])),
                            l(a.yaw, b.yaw),
                            l(a.pitch, b.pitch),
                            exp(l(ln(a.dist), ln(b.dist))),
                            l(a.sh, b.sh),
                        )
                    if (k >= 1) tween = null else busy = true
                } else if (abs(velYaw) + abs(velPitch) > SPIN_MIN) {
                    cam.yaw += velYaw
                    cam.pitch = (cam.pitch + velPitch).coerceIn(-PITCH_MAX, PITCH_MAX)
                    velYaw *= SPIN_DECAY
                    velPitch *= SPIN_DECAY
                    busy = true
                }
            }
            requestRender()
            moving = busy
            if (busy) Choreographer.getInstance().postFrameCallback(this.tick)
        }

    private fun startMotion() {
        if (moving) return
        moving = true
        Choreographer.getInstance().postFrameCallback(tick)
    }

    private fun stopMotion() {
        tween = null
        if (moving) Choreographer.getInstance().removeFrameCallback(tick)
        moving = false
    }

    // ---- camera (call under lock) ----

    private class Cam(
        var t: DoubleArray = doubleArrayOf(0.0, 0.0, 0.0),
        var yaw: Double = .55,
        var pitch: Double = .43,
        var dist: Double = 1.0,
        var sh: Double = .44,
    ) {
        fun copy() = Cam(t.copyOf(), yaw, pitch, dist, sh)

        fun copy(yaw: Double) = Cam(t.copyOf(), yaw, pitch, dist, sh)
    }

    private class Tween(
        val t0: Long,
        val a: Cam,
        val b: Cam,
    )

    private fun aspect() = vw.toDouble() / vh

    private fun homeCam() = Cam(dist = 1.45 * F / aspect())

    private fun eye(): DoubleArray {
        val c = cos(cam.pitch)
        return doubleArrayOf(
            cam.t[0] + c * sin(cam.yaw) * cam.dist,
            cam.t[1] + sin(cam.pitch) * cam.dist,
            cam.t[2] + c * cos(cam.yaw) * cam.dist,
        )
    }

    private fun basis(): Array<DoubleArray> {
        val e = eye()
        val f = nrm(sub(cam.t, e))
        val r = nrm(cross(f, doubleArrayOf(0.0, 1.0, 0.0)))
        return arrayOf(e, f, r, cross(r, f))
    }

    private fun zoomAt(
        cx: Float,
        cy: Float,
        factor: Double,
    ) {
        val k = (cam.dist * factor).coerceIn(DIST_MIN, ZOOM_MAX) / cam.dist
        val c = child
        if (c != null && k < 1) {
            cam.t = add(cam.t, mul(sub(c, cam.t), min(1.0, (1 - k) * 3)))
            cam.dist *= k
            return
        }
        val (e, f, r, u) = basis()
        val nx = cx / vw * 2 - 1
        val ny = 1 - cy / vh * 2
        val d = nrm(add(f, add(mul(r, nx * aspect() / F), mul(u, (ny - cam.sh) / F))))
        var t = -e[1] / d[1]
        if (!(t > 0 && t < cam.dist * 3)) t = cam.dist
        val hit = add(e, mul(d, t))
        cam.t = add(cam.t, mul(sub(hit, cam.t), 1 - k))
        cam.dist *= k
    }

    /*
     * Infinite zoom: near the floor a copy of the home galaxy at scale LS waits at the star nearest the focus while the
     * big one fades out; at the copy's home distance the camera maps into the copy's frame, so the loop has no seam.
     * fo keeps the field-star lattice continuous across the jump.
     */
    private var child: DoubleArray? = null
    private var fo = doubleArrayOf(0.0, 0.0, 0.0)

    private fun loopZoom(p: GalaxyPoints) {
        val hd = homeCam().dist
        if (cam.dist < hd * LS * SEED_AT) {
            if (child == null) child = seed(p)
        } else {
            child = null
        }
        val c = child ?: return
        if (cam.dist < hd * LS) {
            cam.t = mul(sub(cam.t, c), 1 / LS)
            cam.dist /= LS
            fo = mul(sub(fo, c), 1 / LS)
            tween = null
            child = null
        }
    }

    /** The galaxy star or field star nearest the focus. */
    private fun seed(p: GalaxyPoints): DoubleArray {
        val t = cam.t
        val a = p.stars
        var best = t.copyOf()
        var bd = Double.MAX_VALUE
        for (i in 0 until p.galaxyStars * POINT_FLOATS step POINT_FLOATS) {
            val dx = a[i] - t[0]
            val dy = a[i + 1] - t[1]
            val dz = a[i + 2] - t[2]
            val d = dx * dx + dy * dy + dz * dz
            if (d < bd) {
                bd = d
                best = doubleArrayOf(a[i].toDouble(), a[i + 1].toDouble(), a[i + 2].toDouble())
            }
        }
        val e = eye()
        val lf = floor(ln(cam.dist) / ln(4.0))
        val f = p.field
        for (j in 0..3) {
            val sz = 4.0.pow(lf + j) * 1.5
            for (i in f.indices step POINT_FLOATS) {
                val w =
                    DoubleArray(3) { k ->
                        val v = f[i + k] * sz - (e[k] - fo[k]) + sz * .5
                        e[k] + (v - floor(v / sz) * sz) - sz * .5
                    }
                val d = (w[0] - t[0]).pow(2) + (w[1] - t[1]).pow(2) + (w[2] - t[2]).pow(2)
                if (d < bd) {
                    bd = d
                    best = w
                }
            }
        }
        return best
    }

    private fun matrix(): FloatArray {
        val (e, f, r, u) = basis()
        val n = max(cam.dist * .004, .0005)
        val fa = 400.0
        val v =
            doubleArrayOf(r[0], u[0], -f[0], 0.0, r[1], u[1], -f[1], 0.0, r[2], u[2], -f[2], 0.0, -dot(r, e), -dot(u, e), dot(f, e), 1.0)
        val p =
            doubleArrayOf(
                F / aspect(),
                0.0,
                0.0,
                0.0,
                0.0,
                F,
                0.0,
                0.0,
                0.0,
                -cam.sh,
                (fa + n) / (n - fa),
                -1.0,
                0.0,
                0.0,
                2 * fa * n / (n - fa),
                0.0,
            )
        val o = FloatArray(16)
        for (i in 0..3) {
            for (j in 0..3) {
                var s = 0.0
                for (k in 0..3) s += p[k * 4 + j] * v[i * 4 + k]
                o[i * 4 + j] = s.toFloat()
            }
        }
        return o
    }

    // ---- GL thread ----

    fun requestRender() {
        synchronized(lock) {
            if (pending) return
            pending = true
        }
        gl.post(::frame)
    }

    private var egl: Egl? = null
    private var scene: Scene? = null

    override fun onSurfaceTextureAvailable(
        st: SurfaceTexture,
        width: Int,
        height: Int,
    ) {
        synchronized(lock) {
            vw = width
            vh = height
            cam = homeCam()
        }
        gl.post {
            egl = Egl(st)
            scene = Scene(tier, egl!!.halfFloat)
        }
        requestRender()
    }

    override fun onSurfaceTextureSizeChanged(
        st: SurfaceTexture,
        width: Int,
        height: Int,
    ) {
        synchronized(lock) {
            val home = abs(cam.dist - homeCam().dist) < 1e-9
            vw = width
            vh = height
            if (home) cam = homeCam()
        }
        requestRender()
    }

    override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
        stopMotion()
        gl.post {
            scene?.release()
            scene = null
            egl?.release()
            egl = null
        }
        return true
    }

    override fun onSurfaceTextureUpdated(st: SurfaceTexture) = Unit

    override fun onDetachedFromWindow() {
        super.onDetachedFromWindow()
        stopMotion()
        thread.quitSafely()
    }

    private fun frame() {
        val m: FloatArray
        val e: DoubleArray
        val dist: Double
        val homeDist: Double
        val w: Int
        val h: Int
        val p: GalaxyPoints
        val mainA: Float
        var mc: FloatArray? = null
        val o: DoubleArray
        synchronized(lock) {
            pending = false
            p = points ?: return
            loopZoom(p)
            m = matrix()
            val hd = homeCam().dist
            val c = child
            mainA = if (c == null) 1f else (ln(cam.dist / (hd * LS)) / ln(FADE_RANGE)).coerceIn(0.0, 1.0).toFloat()
            if (c != null) {
                val sc = LS.toFloat()
                mc =
                    times(
                        m,
                        floatArrayOf(sc, 0f, 0f, 0f, 0f, sc, 0f, 0f, 0f, 0f, sc, 0f, c[0].toFloat(), c[1].toFloat(), c[2].toFloat(), 1f),
                    )
            }
            o = fo.copyOf()
            e = eye()
            dist = cam.dist
            homeDist = homeCam().dist
            w = vw
            h = vh
        }
        val egl = egl ?: return
        val scene = scene ?: return
        val t0 = SystemClock.elapsedRealtimeNanos()
        val scale = tier.renderScale
        val fw = max(1, (w * scale).toInt())
        val fh = max(1, (h * scale).toInt())
        val dl = if (light) ((ln(dist / (homeDist * .15)) / ln(1 / .15)).coerceIn(0.0, 1.0)) else 0.0
        scene.draw(p, Frame(m, mc, mainA, e, o, dist), fw, fh, w, h, density * scale, light, dl.toFloat())
        egl.swap()
        frames++
        if (debuggable) {
            GLES20.glFinish()
            Log.d(TAG, "frame $frames ${(SystemClock.elapsedRealtimeNanos() - t0) / 1e6} ms ${tier.name} ${fw}x$fh")
        }
    }

    private class Egl(
        st: SurfaceTexture,
    ) {
        val display: EGLDisplay = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY)
        val context: EGLContext
        val surface: EGLSurface
        val halfFloat: Boolean

        init {
            val v = IntArray(2)
            EGL14.eglInitialize(display, v, 0, v, 1)

            fun config(renderable: Int): EGLConfig? {
                val attrs =
                    intArrayOf(
                        EGL14.EGL_RED_SIZE,
                        8,
                        EGL14.EGL_GREEN_SIZE,
                        8,
                        EGL14.EGL_BLUE_SIZE,
                        8,
                        EGL14.EGL_RENDERABLE_TYPE,
                        renderable,
                        EGL14.EGL_NONE,
                    )
                val configs = arrayOfNulls<EGLConfig>(1)
                val n = IntArray(1)
                EGL14.eglChooseConfig(display, attrs, 0, configs, 0, 1, n, 0)
                return configs[0].takeIf { n[0] > 0 }
            }
            // ES 3 where the device has it, so a half-float target can be rendered to; ES 2 otherwise.
            val es3Config = config(EGL_OPENGL_ES3_BIT)
            val es3 = es3Config != null
            val config = es3Config ?: config(EGL14.EGL_OPENGL_ES2_BIT)
            context =
                EGL14.eglCreateContext(
                    display,
                    config,
                    EGL14.EGL_NO_CONTEXT,
                    intArrayOf(EGL14.EGL_CONTEXT_CLIENT_VERSION, if (es3) 3 else 2, EGL14.EGL_NONE),
                    0,
                )
            surface = EGL14.eglCreateWindowSurface(display, config, st, intArrayOf(EGL14.EGL_NONE), 0)
            EGL14.eglMakeCurrent(display, surface, surface, context)
            val ext = GLES20.glGetString(GLES20.GL_EXTENSIONS).orEmpty()
            halfFloat = es3 && ("GL_EXT_color_buffer_half_float" in ext || "GL_EXT_color_buffer_float" in ext)
        }

        fun swap() = EGL14.eglSwapBuffers(display, surface)

        fun release() {
            EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT)
            EGL14.eglDestroySurface(display, surface)
            EGL14.eglDestroyContext(display, context)
        }
    }

    private class Frame(
        val m: FloatArray,
        val mc: FloatArray?,
        val mainA: Float,
        val eye: DoubleArray,
        val fo: DoubleArray,
        val dist: Double,
    )

    private class Scene(
        private val tier: GalaxyTier,
        private val halfFloat: Boolean,
    ) {
        private val star = program(VS, FS)
        private val field = program(FVS, FS)
        private val tone = program(TVS, TFS)
        private var uploaded: GalaxyPoints? = null
        private val vbo = IntArray(BUFFERS)
        private val counts = IntArray(BUFFERS)
        private val tri = IntArray(1)
        private val sharp = Target()
        private val soft = Target()

        // RGBA8 has no headroom, so light is stored at 1/4 and the tone pass scales it back.
        private val store = if (halfFloat) 1f else .25f
        private val pointMax: Float =
            FloatArray(2).also { GLES20.glGetFloatv(GLES20.GL_ALIASED_POINT_SIZE_RANGE, it, 0) }[1]

        init {
            locs.clear()
            GLES20.glGenBuffers(BUFFERS, vbo, 0)
            GLES20.glGenBuffers(1, tri, 0)
            GLES20.glBindBuffer(GLES20.GL_ARRAY_BUFFER, tri[0])
            val q = floats(floatArrayOf(-1f, -1f, 3f, -1f, -1f, 3f))
            GLES20.glBufferData(GLES20.GL_ARRAY_BUFFER, 24, q, GLES20.GL_STATIC_DRAW)
        }

        private fun upload(p: GalaxyPoints) {
            for ((i, a) in listOf(p.stars, p.glow, p.dust, p.field).withIndex()) {
                GLES20.glBindBuffer(GLES20.GL_ARRAY_BUFFER, vbo[i])
                GLES20.glBufferData(GLES20.GL_ARRAY_BUFFER, a.size * 4, floats(a), GLES20.GL_STATIC_DRAW)
                counts[i] = a.size / POINT_FLOATS
            }
            uploaded = p
        }

        private inner class Target {
            val fbo = IntArray(1)
            val tex = IntArray(1)
            var w = 0
            var h = 0

            fun bind(
                width: Int,
                height: Int,
            ) {
                if (width != w || height != h) {
                    if (tex[0] == 0) {
                        GLES20.glGenTextures(1, tex, 0)
                        GLES20.glGenFramebuffers(1, fbo, 0)
                    }
                    GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, tex[0])
                    if (halfFloat) {
                        GLES20.glTexImage2D(GLES20.GL_TEXTURE_2D, 0, GL_RGBA16F, width, height, 0, GLES20.GL_RGBA, GL_HALF_FLOAT, null)
                    } else {
                        GLES20.glTexImage2D(
                            GLES20.GL_TEXTURE_2D,
                            0,
                            GLES20.GL_RGBA,
                            width,
                            height,
                            0,
                            GLES20.GL_RGBA,
                            GLES20.GL_UNSIGNED_BYTE,
                            null,
                        )
                    }
                    for (k in intArrayOf(GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_TEXTURE_MAG_FILTER)) {
                        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, k, GLES20.GL_LINEAR)
                    }
                    for (k in intArrayOf(GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_TEXTURE_WRAP_T)) {
                        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, k, GLES20.GL_CLAMP_TO_EDGE)
                    }
                    GLES20.glBindFramebuffer(GLES20.GL_FRAMEBUFFER, fbo[0])
                    GLES20.glFramebufferTexture2D(GLES20.GL_FRAMEBUFFER, GLES20.GL_COLOR_ATTACHMENT0, GLES20.GL_TEXTURE_2D, tex[0], 0)
                    w = width
                    h = height
                }
                GLES20.glBindFramebuffer(GLES20.GL_FRAMEBUFFER, fbo[0])
                GLES20.glViewport(0, 0, w, h)
                GLES20.glClearColor(0f, 0f, 0f, 1f)
                GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)
            }

            fun release() {
                if (tex[0] == 0) return
                GLES20.glDeleteTextures(1, tex, 0)
                GLES20.glDeleteFramebuffers(1, fbo, 0)
            }
        }

        fun draw(
            p: GalaxyPoints,
            frame: Frame,
            fw: Int,
            fh: Int,
            w: Int,
            h: Int,
            dpr: Float,
            light: Boolean,
            dl: Float,
        ) {
            if (uploaded !== p) upload(p)
            val m = frame.m
            val e = frame.eye
            GLES20.glEnable(GLES20.GL_BLEND)
            GLES20.glBlendFunc(GLES20.GL_ONE, GLES20.GL_ONE)

            // Soft glow covers most of the pixels and has no detail to lose: it draws at GLOW_PER_DP px per dp on any screen.
            val gw = max(1, (fw / dpr * GLOW_PER_DP).toInt())
            val gh = max(1, (fh / dpr * GLOW_PER_DP).toInt())
            soft.bind(gw, gh)
            galaxy(vbo[1], p.galaxyGlow, counts[1], frame, (gh * F / 2).toFloat(), dpr * gh / fh)

            sharp.bind(fw, fh)
            val k = (fh * F / 2).toFloat()
            // Field stars: one unit cube wrapped around the eye at octaves 4x apart that cross-fade with zoom.
            use(field, vbo[3], size = false)
            GLES20.glUniformMatrix4fv(loc(field, "M"), 1, false, m, 0)
            GLES20.glUniform3f(loc(field, "E"), e[0].toFloat(), e[1].toFloat(), e[2].toFloat())
            GLES20.glUniform3f(loc(field, "O"), frame.fo[0].toFloat(), frame.fo[1].toFloat(), frame.fo[2].toFloat())
            GLES20.glUniform1f(loc(field, "K"), k)
            GLES20.glUniform1f(loc(field, "dpr"), dpr)
            GLES20.glUniform1f(loc(field, "hs"), store)
            val l = ln(frame.dist) / ln(4.0)
            val lf = floor(l)
            val fr = l - lf
            for (j in 0..3) {
                val x = j + 1 - fr
                val a =
                    if (tier.fieldOctaves >= 4) {
                        sin(PI * x / 4).pow(2)
                    } else if (j == 1) {
                        1.0
                    } else {
                        0.0
                    }
                if (a <= 0) continue
                GLES20.glUniform1f(loc(field, "S"), (4.0.pow(lf + j) * 1.5).toFloat())
                GLES20.glUniform1f(loc(field, "A"), a.toFloat())
                GLES20.glDrawArrays(GLES20.GL_POINTS, 0, counts[3])
            }
            galaxy(vbo[0], p.galaxyStars, counts[0], frame, k, dpr)
            GLES20.glBlendFunc(GLES20.GL_ZERO, GLES20.GL_ONE_MINUS_SRC_ALPHA)
            galaxy(vbo[2], counts[2], counts[2], frame, k, dpr)

            GLES20.glBindFramebuffer(GLES20.GL_FRAMEBUFFER, 0)
            GLES20.glDisable(GLES20.GL_BLEND)
            GLES20.glViewport(0, 0, w, h)
            GLES20.glUseProgram(tone)
            GLES20.glActiveTexture(GLES20.GL_TEXTURE0)
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, sharp.tex[0])
            GLES20.glActiveTexture(GLES20.GL_TEXTURE1)
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, soft.tex[0])
            GLES20.glUniform1i(loc(tone, "x"), 0)
            GLES20.glUniform1i(loc(tone, "y"), 1)
            GLES20.glUniform1f(loc(tone, "hs"), 1f / store)
            GLES20.glUniform1f(loc(tone, "lt"), if (light) 1f else 0f)
            GLES20.glUniform1f(loc(tone, "dl"), dl)
            GLES20.glBindBuffer(GLES20.GL_ARRAY_BUFFER, tri[0])
            val q = GLES20.glGetAttribLocation(tone, "q")
            GLES20.glEnableVertexAttribArray(q)
            GLES20.glVertexAttribPointer(q, 2, GLES20.GL_FLOAT, false, 8, 0)
            GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, 3)
            GLES20.glActiveTexture(GLES20.GL_TEXTURE0)
        }

        /** The galaxy part [0, split) fades as the zoom copy takes over and is drawn again as the copy; the sky part stays. */
        private fun galaxy(
            buffer: Int,
            split: Int,
            count: Int,
            frame: Frame,
            k: Float,
            dpr: Float,
        ) {
            use(star, buffer, size = true)
            GLES20.glUniform1f(loc(star, "K"), k)
            GLES20.glUniform1f(loc(star, "dpr"), dpr)
            GLES20.glUniform1f(loc(star, "hs"), store)
            GLES20.glUniform1f(loc(star, "pmax"), pointMax)

            fun pass(
                mx: FloatArray,
                a: Float,
                z: Float,
                first: Int,
                n: Int,
            ) {
                if (a <= 0f || n <= 0) return
                GLES20.glUniformMatrix4fv(loc(star, "M"), 1, false, mx, 0)
                GLES20.glUniform1f(loc(star, "A"), a)
                GLES20.glUniform1f(loc(star, "Z"), z)
                GLES20.glDrawArrays(GLES20.GL_POINTS, first, n)
            }
            pass(frame.m, 1f, 1f, split, count - split)
            pass(frame.m, frame.mainA, 1f, 0, split)
            frame.mc?.let { pass(it, 1f, LS.toFloat(), 0, split) }
        }

        private fun use(
            prog: Int,
            buffer: Int,
            size: Boolean,
        ) {
            GLES20.glUseProgram(prog)
            GLES20.glBindBuffer(GLES20.GL_ARRAY_BUFFER, buffer)
            val stride = POINT_FLOATS * 4
            val p = GLES20.glGetAttribLocation(prog, "p")
            GLES20.glEnableVertexAttribArray(p)
            GLES20.glVertexAttribPointer(p, 3, GLES20.GL_FLOAT, false, stride, 0)
            val c = GLES20.glGetAttribLocation(prog, "c")
            GLES20.glEnableVertexAttribArray(c)
            GLES20.glVertexAttribPointer(c, 4, GLES20.GL_FLOAT, false, stride, 12)
            if (size) {
                val s = GLES20.glGetAttribLocation(prog, "s")
                GLES20.glEnableVertexAttribArray(s)
                GLES20.glVertexAttribPointer(s, 1, GLES20.GL_FLOAT, false, stride, 28)
            }
        }

        fun release() {
            GLES20.glDeleteBuffers(BUFFERS, vbo, 0)
            GLES20.glDeleteBuffers(1, tri, 0)
            sharp.release()
            soft.release()
            for (p in intArrayOf(star, field, tone)) GLES20.glDeleteProgram(p)
        }
    }

    private companion object {
        const val TAG = "LinkGalaxy"
        const val GL_RGBA16F = 0x881A
        const val GL_HALF_FLOAT = 0x140B
        const val EGL_OPENGL_ES3_BIT = 0x40
        const val BUFFERS = 4
        const val GLOW_PER_DP = .5f
        const val TAU = 2 * PI
        val F = 1 / tan(40 * PI / 180 / 2)

        /** Turn per dp of drag, radians. */
        const val TURN_YAW = .006
        const val TURN_PITCH = .005
        const val PITCH_MAX = 1.4
        const val SPIN_MIN = 2e-5
        const val SPIN_DECAY = .93
        const val FLY_MS = 1000.0
        const val DIST_MIN = 1e-5

        /** The zoom copy's scale, the distance (x its home distance) where it is seeded, and the fade range of the big one. */
        const val LS = 1.0 / 256
        const val SEED_AT = 40.0
        const val FADE_RANGE = 6.0
        const val ZOOM_MAX = 80.0

        fun sub(
            a: DoubleArray,
            b: DoubleArray,
        ) = doubleArrayOf(a[0] - b[0], a[1] - b[1], a[2] - b[2])

        fun add(
            a: DoubleArray,
            b: DoubleArray,
        ) = doubleArrayOf(a[0] + b[0], a[1] + b[1], a[2] + b[2])

        fun mul(
            a: DoubleArray,
            k: Double,
        ) = doubleArrayOf(a[0] * k, a[1] * k, a[2] * k)

        fun dot(
            a: DoubleArray,
            b: DoubleArray,
        ) = a[0] * b[0] + a[1] * b[1] + a[2] * b[2]

        fun cross(
            a: DoubleArray,
            b: DoubleArray,
        ) = doubleArrayOf(a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])

        fun times(
            a: FloatArray,
            b: FloatArray,
        ) = FloatArray(16) { o ->
            val i = o / 4
            val j = o % 4
            var s = 0f
            for (k in 0..3) s += a[k * 4 + j] * b[i * 4 + k]
            s
        }

        fun nrm(a: DoubleArray) = mul(a, 1 / sqrt(dot(a, a)))

        fun abs(x: Double) = kotlin.math.abs(x)

        fun floats(a: FloatArray) =
            ByteBuffer
                .allocateDirect(a.size * 4)
                .order(ByteOrder.nativeOrder())
                .asFloatBuffer()
                .put(a)
                .position(0)

        val locs = HashMap<Pair<Int, String>, Int>()

        fun loc(
            prog: Int,
            name: String,
        ) = locs.getOrPut(prog to name) { GLES20.glGetUniformLocation(prog, name) }

        fun program(
            vs: String,
            fs: String,
        ): Int {
            fun shader(
                type: Int,
                src: String,
            ) = GLES20.glCreateShader(type).also {
                GLES20.glShaderSource(it, src)
                GLES20.glCompileShader(it)
                val ok = IntArray(1)
                GLES20.glGetShaderiv(it, GLES20.GL_COMPILE_STATUS, ok, 0)
                check(ok[0] != 0) { GLES20.glGetShaderInfoLog(it) }
            }
            val p = GLES20.glCreateProgram()
            GLES20.glAttachShader(p, shader(GLES20.GL_VERTEX_SHADER, vs))
            GLES20.glAttachShader(p, shader(GLES20.GL_FRAGMENT_SHADER, fs))
            GLES20.glLinkProgram(p)
            return p
        }

        // Size > 0: a star, a point of 1.3 to 2.2 dp: closer stars get brighter, never bigger, so zoom stays crisp.
        // Size < 0: soft glow, fading past 140 dp.
        const val VS =
            """attribute vec3 p;attribute vec4 c;attribute float s;uniform mat4 M;uniform float K,dpr,A,Z,hs,pmax;varying vec4 v;varying float h;
void main(){vec4 q=M*vec4(p,1.);gl_Position=q;float d=max(q.w,1e-4),a=c.a*A,z;
 if(s>0.){z=s*Z*K/d;float m=1.3*dpr,x=2.2*dpr;if(z<m){a*=z*z/(m*m);z=m;}else if(z>x){a*=min(z/x,2.);z=x;}h=1.;}
 else{z=-s*Z*K/d;float b=140.*dpr;if(z>b){a*=b*b*b/(z*z*z);}z=min(z,min(300.*dpr,pmax));if(z<1.){a*=z;z=1.;}h=0.;}
 if(q.w<=0.)a=0.;gl_PointSize=z;v=vec4(c.rgb*hs,a);}"""
        const val FS =
            """precision mediump float;varying vec4 v;varying float h;
void main(){vec2 d=gl_PointCoord-.5;float r2=dot(d,d)*4.;float f=h>.5?exp(-r2*7.):exp(-r2*3.5);f*=step(r2,1.);gl_FragColor=vec4(v.rgb*v.a*f,v.a*f);}"""
        const val FVS =
            """attribute vec3 p;attribute vec4 c;uniform mat4 M;uniform vec3 E,O;uniform float S,A,K,dpr,hs;varying vec4 v;varying float h;
void main(){vec3 w=E+mod(p*S-(E-O)+S*.5,S)-S*.5;vec4 q=M*vec4(w,1.);gl_Position=q;float d=max(q.w,1e-6);
 float a=c.a*A*(1.-smoothstep(.55,1.,length(w-E)/(S*.5)));float z=c.a*.004*S*K/d,m=1.3*dpr;if(z<m){a*=z*z/(m*m);z=m;}else if(z>2.2*dpr){a*=min(z/(2.2*dpr),2.);z=2.2*dpr;}
 if(q.w<=0.)a=0.;gl_PointSize=z;v=vec4(c.rgb*hs,a);h=1.;}"""
        const val TVS = "attribute vec2 q;varying vec2 t;void main(){t=q*.5+.5;gl_Position=vec4(q,0.,1.);}"

        // Dark: space, tone-mapped 1-exp. Light: dusk sky (deep blue top, dusk blue toward the sheet), galaxy screen-blended;
        // zoomed in (dl to 0) it turns to night.
        const val TFS =
            """precision mediump float;uniform sampler2D x,y;uniform float lt,dl,hs;varying vec2 t;
void main(){vec3 h=(texture2D(x,t).rgb+texture2D(y,t).rgb)*hs*1.5;vec3 c=1.-exp(-h);float l=dot(c,vec3(.3,.59,.11));c=max(mix(vec3(l),c,1.5),0.);
 if(lt>.5){vec3 sky=mix(vec3(.5,.56,.74),vec3(.1,.12,.26),smoothstep(.25,1.,t.y));vec3 night=1.-(1.-sky)*(1.-c*.85);
  vec3 day=mix(vec3(.2,.24,.48),vec3(.04,.05,.15),smoothstep(.2,1.,t.y));day=1.-(1.-day)*(1.-c);c=mix(night,day,dl);}
 gl_FragColor=vec4(pow(c,vec3(.9)),1.);}"""
    }
}
