package org.andromeda.link.control

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.andromeda.link.core.domain.RemoteEvent
import org.andromeda.link.core.domain.RemoteScreens
import org.andromeda.link.core.domain.RemoteSource
import org.andromeda.link.core.domain.RemoteUnit
import org.andromeda.link.core.domain.RemoteVideo
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import kotlin.random.Random

private const val DESKTOP = "desk"

private class Video : RemoteVideo {
    var stopped = false

    override suspend fun next(): RemoteUnit? = null

    override suspend fun stop() {
        stopped = true
    }
}

private class Screens : RemoteScreens {
    override val remote = MutableSharedFlow<RemoteEvent>(extraBufferCapacity = 64)
    var failNextStart = false
    var stops = 0

    override suspend fun startRemote(
        desktopId: String,
        source: Int?,
    ): Result<Unit> =
        if (failNextStart) {
            failNextStart = false
            Result.failure(IllegalStateException("refused"))
        } else {
            Result.success(Unit)
        }

    override suspend fun stopRemote(desktopId: String): Result<Unit> {
        stops++
        return Result.success(Unit)
    }
}

/** Random open, leave and desktop event sequences; after each step the viewer's phase and streams keep the invariants. */
@OptIn(ExperimentalCoroutinesApi::class)
class ViewerViewModelTest {
    private val dispatcher = StandardTestDispatcher()

    @Before
    fun setUp() = Dispatchers.setMain(dispatcher)

    @After
    fun tearDown() = Dispatchers.resetMain()

    @Test
    fun randomSequencesKeepThePhaseInvariants() =
        runTest(dispatcher) {
            repeat(200) { seed -> sequence(Random(seed), seed) }
        }

    private fun TestScope.sequence(
        random: Random,
        seed: Int,
    ) {
        val screens = Screens()
        val viewer = ViewerViewModel(screens, DESKTOP, dispatcher)
        val streams = mutableListOf<Video>()
        var open = false
        var leaves = 0
        var refused = false
        advanceUntilIdle()
        repeat(40) { step ->
            val action = random.nextInt(6)
            when (action) {
                0 -> {
                    if (!open) refused = random.nextInt(4) == 0
                    screens.failNextStart = !open && refused
                    viewer.open()
                    open = true
                }
                1 -> {
                    if (open) leaves++
                    viewer.leave()
                    open = false
                }
                2 -> {
                    val video = Video().also(streams::add)
                    screens.remote.tryEmit(RemoteEvent.Stream(DESKTOP, 1, 1, video))
                }
                3 -> screens.remote.tryEmit(RemoteEvent.Stopped(DESKTOP, "ended"))
                4 -> screens.remote.tryEmit(RemoteEvent.Started(DESKTOP, listOf(RemoteSource("screen", false)), 0))
                else -> screens.remote.tryEmit(RemoteEvent.Stream("other", 1, 1, Video()))
            }
            advanceUntilIdle()
            val at = "seed $seed step $step action $action"
            val state = viewer.state.value
            val shown = state.stream?.video
            if (!open) assertEquals(at, ViewerPhase.Waiting, state.phase)
            if (shown != null) assertFalse(at, (shown as Video).stopped)
            streams.filter { it !== shown }.forEach { assertTrue("$at: a stream not shown runs", it.stopped) }
            assertEquals(at, leaves, screens.stops)
        }
    }
}
