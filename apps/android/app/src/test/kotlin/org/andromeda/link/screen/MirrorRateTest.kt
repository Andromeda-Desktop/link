package org.andromeda.link.screen

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MirrorRateTest {
    private val max = 24_000_000
    private val min = 1_000_000

    /**
     * Sends one unit every 16 ms that is queued at once, so the send itself never lags; [reportLagUs] gives the
     * desktop's report every 500 ms, none when null. Returns the bitrates set.
     */
    private fun run(
        rate: MirrorRate,
        fromUs: Long,
        toUs: Long,
        reportLagUs: ((Long) -> Long)? = null,
    ): List<Int> {
        val steps = mutableListOf<Int>()
        var nowUs = fromUs
        while (nowUs < toUs) {
            if (reportLagUs != null && nowUs % 500_000 < 16_000) rate.reported(reportLagUs(nowUs), nowUs)
            rate.sent(nowUs, nowUs, keyframe = nowUs % 2_000_000 < 16_000)?.let(steps::add)
            nowUs += 16_000
        }
        return steps
    }

    @Test
    fun aLagTheDesktopReportsStepsTheBitrateDownAndCalmStepsItBackUp() {
        val rate = MirrorRate(max, min)
        assertEquals(emptyList<Int>(), run(rate, 1_000_000, 4_000_000) { 10_000 })
        val down = run(rate, 4_000_000, 10_000_000) { 400_000 }
        assertTrue("no step down: $down", down.isNotEmpty() && down.last() < max / 4)
        val up = run(rate, 10_000_000, 30_000_000) { 10_000 }
        assertTrue("no step up: $up", up.isNotEmpty() && up.last() > down.last())
    }

    @Test
    fun reportsThatStopCountAsLag() {
        val rate = MirrorRate(max, min)
        run(rate, 1_000_000, 2_000_000) { 10_000 }
        val down = run(rate, 2_000_000, 6_000_000)
        assertTrue("no step down: $down", down.isNotEmpty())
    }

    @Test
    fun aDesktopWithoutReportsLeavesAQuickSendAlone() {
        assertEquals(emptyList<Int>(), run(MirrorRate(max, min), 1_000_000, 10_000_000))
    }
}
