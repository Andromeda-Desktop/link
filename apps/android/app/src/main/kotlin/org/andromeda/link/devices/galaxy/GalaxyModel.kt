/*
 * The galaxy model follows beltoforion/Galaxy-Renderer (density-wave orbits, bulge and disc CDF, glow, filaments).
 * Copyright 2026 Ingo Berg. BSD-2-Clause; full notice in NOTICE.
 */
package org.andromeda.link.devices.galaxy

import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.acos
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.floor
import kotlin.math.hypot
import kotlin.math.ln
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/** Point layout: x, y, z, r, g, b, alpha, size. Size > 0 is a star (world units), size < 0 a soft glow. */
const val POINT_FLOATS = 8

/** Detail per device class: the low tier keeps the shape and drops density. */
enum class GalaxyTier(
    val stars: Int,
    val fieldOctaves: Int,
) {
    Full(45_000, 4),
    Low(25_000, 1),
}

/**
 * [stars] are sharp; [glow] is soft and large, so it draws at a quarter of the resolution; [dust] absorbs light. The
 * first [galaxyStars] and [galaxyGlow] points are the galaxy, the rest the far sky; infinite zoom copies only the galaxy.
 */
class GalaxyPoints(
    val stars: FloatArray,
    val glow: FloatArray,
    val dust: FloatArray,
    val field: FloatArray,
    val galaxyStars: Int,
    val galaxyGlow: Int,
)

/** One galaxy's form: the orbit model's parameters and the random [seed] its stars come from. */
data class GalaxyShape(
    val seed: Int,
    val core: Double,
    val ex1: Double,
    val ex2: Double,
    val dA: Double,
    val pertN: Int,
    val pertAmp: Double,
    val temp: Double,
) {
    companion object {
        /** The home galaxy (the look of link fac318b); it alone carries the sky. */
        val HOME = GalaxyShape(7, .31, .85, .95, 5.2, 2, 40.0, 3000.0)

        /** Another desktop's galaxy: a stable form per [seed], in the range of gen-depth.py GAL. */
        fun of(seed: Int): GalaxyShape {
            val r = Rng(seed)
            return GalaxyShape(
                seed,
                core = .28 + .08 * r.next(),
                ex1 = .62 + .26 * r.next(),
                ex2 = .9 + .08 * r.next(),
                dA = 2.4 + 3.0 * r.next(),
                pertN = if (r.next() < .5) 2 else 3,
                pertAmp = 35 + 40 * r.next(),
                temp = 3000 + 1500 * r.next(),
            )
        }
    }
}

private class Rng(
    private var s: Int,
) {
    fun next(): Double {
        s += 0x6D2B79F5
        var t = (s xor (s ushr 15)) * (1 or s)
        t = (t + (t xor (t ushr 7)) * (61 or t)) xor t
        return ((t xor (t ushr 14)).toLong() and 0xffffffffL) / 4294967296.0
    }

    fun gauss(): Double = sqrt(-2 * ln(1 - next())) * cos(TAU * next())
}

private const val TAU = 2 * PI

private class Buf(
    capacity: Int,
) {
    var a = FloatArray(capacity * POINT_FLOATS)
    var n = 0

    fun put(
        x: Double,
        y: Double,
        z: Double,
        c: DoubleArray,
        alpha: Double,
        size: Double,
    ) {
        if (n + POINT_FLOATS > a.size) a = a.copyOf(a.size * 2)
        a[n] = x.toFloat()
        a[n + 1] = y.toFloat()
        a[n + 2] = z.toFloat()
        a[n + 3] = c[0].toFloat()
        a[n + 4] = c[1].toFloat()
        a[n + 5] = c[2].toFloat()
        a[n + 6] = alpha.toFloat()
        a[n + 7] = size.toFloat()
        n += POINT_FLOATS
    }

    fun copy(
        src: FloatArray,
        at: Int,
    ) {
        if (n + POINT_FLOATS > a.size) a = a.copyOf(a.size * 2)
        src.copyInto(a, n, at, at + POINT_FLOATS)
        n += POINT_FLOATS
    }

    fun done(): FloatArray = a.copyOf(n)
}

private fun kelvin(k: Double): DoubleArray {
    val t = k / 100
    val r: Double
    val g: Double
    val b: Double
    if (t <= 66) {
        r = 255.0
        g = 99.47 * ln(t) - 161.12
        b = if (t <= 19) 0.0 else 138.52 * ln(t - 10) - 305.04
    } else {
        r = 329.7 * (t - 60).pow(-.1332)
        g = 288.12 * (t - 60).pow(-.0755)
        b = 255.0
    }
    return doubleArrayOf(r, g, b).also { for (i in 0..2) it[i] = it[i].coerceIn(0.0, 255.0) / 255 }
}

/**
 * Builds [shape]'s galaxy; the home shape also gets the scene around it: a far star sphere with nebula glow and one unit
 * cube of field stars.
 */
fun buildGalaxy(
    tier: GalaxyTier,
    shape: GalaxyShape = GalaxyShape.HOME,
): GalaxyPoints {
    val sky = shape == GalaxyShape.HOME
    val light = Buf(tier.stars * 2)
    val dust = Buf(8_000)
    galaxy(light, dust, tier.stars, shape)
    val galaxyEnd = light.n
    val rnd = Rng(99)
    repeat(
        if (!sky) {
            0
        } else if (tier == GalaxyTier.Full) {
            7000
        } else {
            3000
        },
    ) {
        val u = rnd.next() * TAU
        val v = acos(2 * rnd.next() - 1)
        val t = rnd.next()
        val c =
            if (t < .3) {
                doubleArrayOf(.7, .8, 1.0)
            } else if (t < .8) {
                doubleArrayOf(1.0, .97, .92)
            } else {
                doubleArrayOf(1.0, .8, .6)
            }
        light.put(80 * sin(v) * cos(u), 80 * cos(v), 80 * sin(v) * sin(u), c, .35 + .6 * rnd.next(), .035 + rnd.next().pow(4) * .12)
    }
    repeat(if (sky) 160 else 0) {
        var x = rnd.gauss()
        var y = rnd.gauss() * .22
        var z = rnd.gauss()
        val l = sqrt(x * x + y * y + z * z) / 95
        x /= l
        y /= l
        z /= l
        val ca = cos(.5)
        val sa = sin(.5)
        val y1 = y * ca - z * sa
        val z1 = y * sa + z * ca
        val cz = cos(.3)
        val sz = sin(.3)
        val t = rnd.next()
        val c =
            if (t < .4) {
                doubleArrayOf(.22, .28, .8)
            } else if (t < .75) {
                doubleArrayOf(.5, .22, .7)
            } else {
                doubleArrayOf(.15, .5, .6)
            }
        light.put(x * cz - y1 * sz, x * sz + y1 * cz, z1, c, .02, -(6 + rnd.next() * 14))
    }
    val field = Buf(9000)
    val fr = Rng(5)
    repeat(
        if (!sky) {
            0
        } else if (tier == GalaxyTier.Full) {
            9000
        } else {
            4000
        },
    ) {
        val t = fr.next()
        val c =
            if (t < .3) {
                doubleArrayOf(.7, .8, 1.0)
            } else if (t < .8) {
                doubleArrayOf(1.0, .96, .9)
            } else {
                doubleArrayOf(1.0, .78, .58)
            }
        field.put(fr.next(), fr.next(), fr.next(), c, .25 + .75 * fr.next().pow(2), 0.0)
    }
    val all = light.done()
    val sharp = Buf(all.size / POINT_FLOATS)
    val soft = Buf(all.size / POINT_FLOATS)
    var galaxyStars = 0
    var galaxyGlow = 0
    for (i in all.indices step POINT_FLOATS) {
        if (i == galaxyEnd) {
            galaxyStars = sharp.n / POINT_FLOATS
            galaxyGlow = soft.n / POINT_FLOATS
        }
        (if (all[i + 7] > 0) sharp else soft).copy(all, i)
    }
    if (galaxyEnd == all.size) {
        galaxyStars = sharp.n / POINT_FLOATS
        galaxyGlow = soft.n / POINT_FLOATS
    }
    return GalaxyPoints(sharp.done(), soft.done(), dust.done(), field.done(), galaxyStars, galaxyGlow)
}

// Every star rides an ellipse whose eccentricity and tilt grow with radius; where ellipses crowd, arms appear. Galaxy radius = 1.
private fun galaxy(
    light: Buf,
    dust: Buf,
    n: Int,
    shape: GalaxyShape,
) {
    val rnd = Rng(shape.seed)
    val core = shape.core
    val ex1 = shape.ex1
    val ex2 = shape.ex2
    val dA = shape.dA
    val pertN = shape.pertN
    val pertAmp = shape.pertAmp
    val temp = shape.temp
    val glow = .5
    val h2 = 700 * n / 90_000
    val far = 2.0
    val pc = 13000.0

    fun ecc(r: Double) =
        when {
            r < core -> 1 + r / core * (ex1 - 1)
            r <= 1 -> ex1 + (r - core) / (1 - core) * (ex2 - ex1)
            r < far -> ex2 + (r - 1) / (far - 1) * (1 - ex2)
            else -> 1.0
        }
    val out = DoubleArray(2)

    fun pos(
        a: Double,
        th: Double,
        tilt: Double = a * dA,
    ): DoubleArray {
        val b = a * ecc(a)
        val cb = cos(-tilt)
        val sb = sin(-tilt)
        val ca = cos(th)
        val sa = sin(th)
        out[0] = a * ca * cb - b * sa * sb + a / pertAmp * sin(th * 2 * pertN)
        out[1] = a * ca * sb + b * sa * cb + a / pertAmp * cos(th * 2 * pertN)
        return out
    }
    val steps = 1000

    fun ib(x: Double) = exp(-.02 * (x * pc).pow(.25))

    fun intensity(x: Double) = if (x < core) ib(x) else ib(core) * exp(-(x - core) * 3)
    val cdf = DoubleArray(steps + 1)
    for (i in 1..steps) cdf[i] = cdf[i - 1] + intensity(i * far / steps)

    fun radius(): Double {
        val u = rnd.next() * cdf[steps]
        var lo = 0
        var hi = steps
        while (hi - lo > 1) {
            val m = (lo + hi) ushr 1
            if (cdf[m] < u) lo = m else hi = m
        }
        return hi * far / steps
    }

    fun thick(r: Double) = .012 + .07 * exp(-(r / .2).pow(2))

    fun jitter(a: Double) = .06 * exp(-(a / .12).pow(2))
    for (i in 0 until n) {
        val a = radius()
        val p = pos(a, rnd.next() * TAU)
        val x = p[0]
        val z = p[1]
        var mag = .1 + .4 * rnd.next()
        if (i < n / 60) mag = min(1.0, mag + .1 + .4 * rnd.next())
        val j = jitter(a)
        light.put(
            x + rnd.gauss() * j,
            rnd.gauss() * thick(a),
            z + rnd.gauss() * j,
            kelvin(3800 + a * 6500 + 1500 * rnd.next()),
            .35 + mag,
            .0022 + mag * .006,
        )
    }
    for (i in 0 until (n * glow).toInt()) {
        val a = if (i % 2 == 1) min(radius(), 1.15) else hypot(2 * rnd.next() - 1, 2 * rnd.next() - 1) * .8
        val p = pos(a, rnd.next() * TAU)
        val x = p[0]
        val z = p[1]
        val mag = .02 + .15 * rnd.next()
        val j = jitter(a)
        light.put(x + rnd.gauss() * j, rnd.gauss() * thick(a) * .7, z + rnd.gauss() * j, kelvin(temp + a * 8000), mag * .14, -(mag * .2))
    }
    for (i in 0 until (n * glow / 100).toInt()) {
        var a = hypot(2 * rnd.next() - 1, 2 * rnd.next() - 1)
        val th = rnd.next() * TAU
        val mag = .1 + .05 * rnd.next()
        val m = floor(100 * rnd.next()).toInt()
        repeat(m) {
            a += (200 - 400 * rnd.next()) / pc
            val p = pos(a, th + (10 - 20 * rnd.next()) * PI / 180)
            light.put(p[0], rnd.gauss() * .006, p[1], kelvin(temp + a * pc / 4.5 - 1000), (mag + .025 * rnd.next()) * .15, -(mag * .14))
        }
    }
    // arm crests: where neighbouring orbits crowd, gas ignites: H-alpha knots, young clusters, dust lanes
    val d = 1000 / pc
    var lit = 0
    var i = 0
    while (i < h2 * 12 && lit < h2) {
        i++
        val x0 = 2 * rnd.next() - 1
        val z0 = 2 * rnd.next() - 1
        val a = hypot(x0, z0)
        if (a < .12 || a > 1) continue
        val th = rnd.next() * TAU
        val aI = max(a - d, 0.0)
        val aO = a + d
        val tA = a * dA
        val p = pos(a, th).copyOf()
        val pI = pos(aI, th - (tA - aI * dA), aI * dA).copyOf()
        val pO = pos(aO, th + (aO * dA - tA), aO * dA).copyOf()
        val rho = .5 * ((a - aI) / max(hypot(p[0] - pI[0], p[1] - pI[1]), 1 / pc) + d / max(hypot(p[0] - pO[0], p[1] - pO[1]), 1 / pc))
        val ig = ((rho - 1.25) / .6).coerceIn(0.0, 1.0)
        if (ig <= 0) continue
        lit++
        val y = rnd.gauss() * .006
        light.put(p[0], y, p[1], HALPHA, .6 * ig, -(.016 + .024 * rnd.next()))
        repeat(12) {
            light.put(
                p[0] + rnd.gauss() * .006,
                y + rnd.gauss() * .003,
                p[1] + rnd.gauss() * .006,
                YOUNG,
                .8,
                .0022 + rnd.next() * .003,
            )
        }
        val aq = a - d * .6
        val q = pos(aq, th - (tA - aq * dA), aq * dA)
        repeat(
            10,
        ) { dust.put(q[0] + rnd.gauss() * .02, rnd.gauss() * .004, q[1] + rnd.gauss() * .02, BLACK, .06, -(.03 + rnd.next() * .04)) }
    }
    repeat(80 * n / 90_000) {
        val r = .25 + rnd.next() * 1.1
        val u = rnd.next() * TAU
        val v = acos(2 * rnd.next() - 1)
        val x = r * sin(v) * cos(u)
        val y = r * cos(v) * .6
        val z = r * sin(v) * sin(u)
        repeat(60) {
            val g = abs(rnd.gauss())
            light.put(
                x + rnd.gauss() * .004 * g,
                y + rnd.gauss() * .004 * g,
                z + rnd.gauss() * .004 * g,
                GLOBULAR,
                .7,
                .0018 + rnd.next() * .002,
            )
        }
    }
}

private val HALPHA = doubleArrayOf(1.0, .25, .42)
private val YOUNG = doubleArrayOf(.75, .85, 1.0)
private val BLACK = doubleArrayOf(0.0, 0.0, 0.0)
private val GLOBULAR = doubleArrayOf(1.0, .88, .72)
