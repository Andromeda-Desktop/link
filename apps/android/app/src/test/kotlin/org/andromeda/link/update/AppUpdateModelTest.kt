package org.andromeda.link.update

import org.andromeda.link.update.AppUpdate.Event
import org.andromeda.link.update.AppUpdate.State
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.random.Random

class AppUpdateModelTest {
    private val updates = listOf(State.Available("2.0", "u1", 16_000_000), State.Available("2.1", "u2", 0))

    private fun randomEvent(r: Random): Event =
        when (r.nextInt(9)) {
            0 -> Event.Check
            1 -> Event.Checked(listOf(State.Current, updates.random(r), State.Failed("x"), State.Installing, State.Idle).random(r))
            2 -> Event.Start(updates.random(r))
            3 -> Event.Progress(r.nextLong(0, 20_000_000), listOf(0L, 16_000_000L).random(r))
            4 -> Event.Downloaded
            5 -> Event.Cancelled
            6 -> Event.Fail("y")
            7 -> Event.Installed
            else -> Event.Aborted
        }

    @Test
    fun randomSequencesKeepTheInvariants() {
        val seen = mutableSetOf<Class<*>>()
        repeat(2_000) { seed ->
            val r = Random(seed)
            var state: State = State.Idle
            repeat(60) {
                val event = randomEvent(r)
                val next = AppUpdate.reduce(state, event) ?: state
                seen += next.javaClass
                if (state is State.Downloading || state == State.Installing) {
                    assertTrue("a check must not drop a running install: $state", event != Event.Check || next == state)
                }
                if (next is State.Downloading) {
                    assertTrue("progress in 0..1", next.progress in 0f..1f)
                    if (state is State.Downloading) {
                        assertEquals("one download keeps its update", state.update, next.update)
                        assertTrue("received never goes back", next.received >= state.received)
                    } else {
                        assertEquals("a download starts only from its own offer", next.update, state)
                    }
                }
                if (event == Event.Cancelled && state is State.Downloading) {
                    assertEquals("cancel offers the same update again", state.update, next)
                }
                if (next == State.Current && state != State.Current) {
                    assertTrue("Current comes from a check or an install", state == State.Checking || state == State.Installing)
                }
                if (event is Event.Checked && next != state) {
                    assertTrue("a check result is never a running install", next !is State.Downloading && next != State.Installing)
                }
                state = next
            }
        }
        assertEquals("every state is reachable", 7, seen.size)
    }
}
