package org.andromeda.link.core.data

import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pGroup
import android.net.wifi.p2p.WifiP2pManager
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import org.andromeda.link.ffi.HotspotCredentials
import org.andromeda.link.ffi.PhoneWifiDirect
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit

/**
 * A Wi-Fi Direct group this phone owns, which a desktop joins with the credentials Link sends it over Bluetooth, so
 * the phone keeps its own Wi-Fi and the desktop may keep its own. No peer negotiates: an incoming negotiation or
 * invitation needs a tap on Android ("Invitation to connect"), and a client joining with the passphrase does not.
 * Needs Nearby devices (Android 13+) or location, like the hotspot.
 */
@SuppressLint("MissingPermission")
class WifiDirectGroup(
    private val context: Context,
) : PhoneWifiDirect {
    private val manager: WifiP2pManager? = context.getSystemService(WifiP2pManager::class.java)
    private var channel: WifiP2pManager.Channel? = null

    /** Forms [group], the same name and passphrase each time for one desktop, so the desktop knows it again. */
    override fun start(group: HotspotCredentials): HotspotCredentials? {
        val (manager, channel) = ready() ?: return null
        // On 2.4 GHz the group shares airtime with Bluetooth, which stays up beside it: 5 GHz first.
        val formed =
            form(manager, channel, group, WifiP2pConfig.GROUP_OWNER_BAND_5GHZ, BAND_5GHZ_TIMEOUT_MS)
                ?: form(manager, channel, group, WifiP2pConfig.GROUP_OWNER_BAND_AUTO, FORM_TIMEOUT_MS)
                ?: return null.also { Log.i(TAG, "the group did not come up") }
        Log.i(TAG, "wifi-direct group on ${formed.frequency} MHz")
        return HotspotCredentials(
            formed.networkName,
            formed.passphrase ?: return null,
            formed.frequency.takeIf { it > 0 }?.toUInt(),
        )
    }

    private fun form(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        group: HotspotCredentials,
        band: Int,
        timeoutMs: Long,
    ): WifiP2pGroup? {
        val config =
            WifiP2pConfig
                .Builder()
                .setNetworkName(group.ssid)
                .setPassphrase(group.passphrase)
                .setGroupOperatingBand(band)
                .build()
        // A group left from an earlier attempt would make Android refuse a new one.
        call(manager, channel, "removing the group", manager::removeGroup)
        if (!created(manager, channel, config)) return null
        return group(manager, channel, timeoutMs) ?: null.also {
            // A driver that fails to start the group leaves Android creating it, answering BUSY until cancelled.
            call(manager, channel, "cancelling the group", manager::cancelConnect)
        }
    }

    override fun stop() {
        val (manager, channel) = ready() ?: return
        call(manager, channel, "removing the group", manager::removeGroup)
    }

    /** Retries while Android answers BUSY, as it does while it turns Wi-Fi Direct on for a freshly started app. */
    private fun created(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        config: WifiP2pConfig,
    ): Boolean {
        repeat(ATTEMPTS) {
            val reason = call(manager, channel, "creating the group") { c, done -> manager.createGroup(c, config, done) }
            if (reason == null) return true
            if (reason != WifiP2pManager.BUSY) return false
            SystemClock.sleep(RETRY_MS)
        }
        return false
    }

    /** The group once Android reports it up with this phone as its owner, read again on each connection change. */
    private fun group(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        timeoutMs: Long,
    ): WifiP2pGroup? {
        val changed = Semaphore(0)
        val receiver =
            object : BroadcastReceiver() {
                override fun onReceive(
                    context: Context,
                    intent: Intent,
                ) = changed.release()
            }
        val filter = IntentFilter(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
        // A protected system broadcast: only Android sends it, so the receiver needs no export flag.
        context.registerReceiver(receiver, filter)
        try {
            val deadline = SystemClock.elapsedRealtime() + timeoutMs
            do {
                owned(manager, channel)?.let { return it }
                val left = deadline - SystemClock.elapsedRealtime()
            } while (left > 0 && changed.tryAcquire(left, TimeUnit.MILLISECONDS))
            return null
        } catch (error: SecurityException) {
            return null.also { Log.w(TAG, "Wi-Fi Direct: ${error.message}") }
        } finally {
            context.unregisterReceiver(receiver)
        }
    }

    private fun owned(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
    ): WifiP2pGroup? {
        val answered = CountDownLatch(1)
        var group: WifiP2pGroup? = null
        manager.requestGroupInfo(channel) {
            group = it
            answered.countDown()
        }
        answered.await(ANSWER_TIMEOUT_S, TimeUnit.SECONDS)
        return group?.takeIf { it.isGroupOwner && it.networkName != null }
    }

    /** Runs a [WifiP2pManager] action and waits for it: null on success, else Android's failure reason. */
    private fun call(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        what: String,
        action: (WifiP2pManager.Channel, WifiP2pManager.ActionListener) -> Unit,
    ): Int? {
        val answered = CountDownLatch(1)
        var failure: Int? = ERROR_TIMEOUT
        val listener =
            object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    failure = null
                    answered.countDown()
                }

                override fun onFailure(reason: Int) {
                    Log.i(TAG, "$what failed: $reason")
                    failure = reason
                    answered.countDown()
                }
            }
        try {
            action(channel, listener)
        } catch (error: SecurityException) {
            Log.w(TAG, "Wi-Fi Direct: ${error.message}")
            return WifiP2pManager.ERROR
        }
        answered.await(ANSWER_TIMEOUT_S, TimeUnit.SECONDS)
        return failure
    }

    private fun ready(): Pair<WifiP2pManager, WifiP2pManager.Channel>? {
        val manager = manager ?: return null
        val channel =
            synchronized(this) {
                channel ?: manager.initialize(context, Looper.getMainLooper(), null).also { channel = it }
            } ?: return null
        return manager to channel
    }

    private companion object {
        const val TAG = "WifiDirectGroup"
        const val ANSWER_TIMEOUT_S = 5L
        const val FORM_TIMEOUT_MS = 10_000L

        // A 5 GHz group forms in ~0.5 s on the OnePlus 12; a driver that refuses the band never reports it.
        const val BAND_5GHZ_TIMEOUT_MS = 3_000L
        const val ATTEMPTS = 5
        const val RETRY_MS = 1_000L
        const val ERROR_TIMEOUT = -1
    }
}
