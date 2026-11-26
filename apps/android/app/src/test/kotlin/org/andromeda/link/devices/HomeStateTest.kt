package org.andromeda.link.devices

import org.andromeda.link.core.domain.Desktop
import org.andromeda.link.core.domain.DesktopLink
import org.junit.Assert.assertEquals
import org.junit.Test

class HomeStateTest {
    private fun desktop(
        id: String,
        link: DesktopLink,
        lastSeen: Long,
    ) = Desktop(id = id, name = id, link = link, lastSeen = lastSeen)

    @Test
    fun withNoneChosenTheHeroIsTheConnectedDesktopElseTheOneSeenLast() {
        val stale = desktop("stale", DesktopLink.Offline, lastSeen = 100)
        val recent = desktop("recent", DesktopLink.Offline, lastSeen = 200)
        val reconnecting = desktop("reconnecting", DesktopLink.Reconnecting, lastSeen = 150)
        val live = desktop("live", DesktopLink.Live, lastSeen = 50)

        assertEquals("recent", HomeState(listOf(stale, recent, reconnecting)).primary?.id)
        assertEquals("live", HomeState(listOf(stale, recent, live)).primary?.id)
        assertEquals("stale", HomeState(listOf(stale, recent, live), selectedDesktopId = "stale").primary?.id)
    }
}
