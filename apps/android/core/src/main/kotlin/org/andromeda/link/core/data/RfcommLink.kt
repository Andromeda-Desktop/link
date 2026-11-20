package org.andromeda.link.core.data

import android.Manifest
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.system.Os
import android.system.OsConstants
import android.util.Log
import org.andromeda.link.ffi.BluetoothLink
import java.io.Closeable
import java.io.FileDescriptor
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

/**
 * Reaches a desktop's Link service over RFCOMM or an LE L2CAP channel for the core, which needs a descriptor:
 * `BluetoothSocket` exposes only streams, so two threads pump it through a socket pair and the core gets the other end.
 */
class RfcommLink(
    private val context: Context,
) : BluetoothLink {
    override fun open(
        address: String,
        pair: Boolean,
    ): Int {
        if (!allowed()) return NO_PERMISSION.also { Log.i(TAG, "RFCOMM to $address: no BLUETOOTH_CONNECT permission") }
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
        if (adapter == null || !adapter.isEnabled) return BLUETOOTH_OFF.also { Log.i(TAG, "RFCOMM to $address: Bluetooth is off") }
        val device = adapter.getRemoteDevice(address)
        val socket =
            try {
                if (pair && !bonded(device)) return NOT_BONDED
                connect(device) ?: return NOT_CONNECTED
            } catch (error: SecurityException) {
                Log.w(TAG, "RFCOMM to $address: ${error.message}")
                return NOT_CONNECTED
            }
        val (ours, theirs) = ParcelFileDescriptor.createSocketPair()
        val writer = ours.dup()
        val closeAll = { listOf<Closeable>(socket, ours, writer).forEach { runCatching { it.close() } } }
        pump("rfcomm-in", socket.inputStream, FileOutputStream(writer.fileDescriptor)) {
            // The core reads end-of-stream only once no end can write, so shut down ours before closing.
            runCatching { Os.shutdown(writer.fileDescriptor, OsConstants.SHUT_WR) }
            closeAll()
        }
        pump("rfcomm-out", FileInputStream(ours.fileDescriptor), socket.outputStream, closeAll)
        return theirs.detachFd()
    }

    /**
     * An LE L2CAP channel to [psm] at the desktop's public address, pumped through a `SOCK_SEQPACKET` pair so each
     * QUIC packet stays one SDU. Unencrypted: QUIC's TLS authenticates both ends. Needs API 33 for the LE address.
     */
    override fun openL2cap(
        address: String,
        psm: UShort,
    ): Int {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return NOT_CONNECTED
        if (!allowed()) return NO_PERMISSION
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
        if (adapter == null || !adapter.isEnabled) return BLUETOOTH_OFF
        val socket =
            try {
                adapter.getRemoteLeDevice(address, BluetoothDevice.ADDRESS_TYPE_PUBLIC).createInsecureL2capChannel(psm.toInt())
            } catch (error: SecurityException) {
                Log.w(TAG, "L2CAP to $address: ${error.message}")
                return NOT_CONNECTED
            }
        val connected = AtomicBoolean(false)
        // An LE connect to a desktop that does not listen waits for Android's own 30 s timeout; closing ends it.
        thread(name = "l2cap-connect", isDaemon = true) {
            SystemClock.sleep(L2CAP_CONNECT_MS)
            if (!connected.get()) runCatching { socket.close() }
        }
        try {
            socket.connect()
            connected.set(true)
        } catch (error: IOException) {
            Log.i(TAG, "L2CAP to $address: ${error.message}")
            runCatching { socket.close() }
            return NOT_CONNECTED
        }
        val ours = FileDescriptor()
        val theirs = FileDescriptor()
        Os.socketpair(OsConstants.AF_UNIX, OsConstants.SOCK_SEQPACKET, 0, ours, theirs)
        val running = AtomicInteger(2)
        // Shuts both ends at once, so the core and the other pump see the end; closes ours after the last pump.
        val ended = {
            runCatching { socket.close() }
            runCatching { Os.shutdown(ours, OsConstants.SHUT_RDWR) }
            if (running.decrementAndGet() == 0) runCatching { Os.close(ours) }
        }
        thread(name = "l2cap-in", isDaemon = true) {
            val buffer = ByteArray(maxOf(socket.maxReceivePacketSize, BUFFER))
            try {
                val input = socket.inputStream
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    Os.write(ours, buffer, 0, read)
                }
            } catch (_: Exception) {
                // Either side closing ends the link; the core sees it as the connection lost.
            } finally {
                ended()
            }
        }
        thread(name = "l2cap-out", isDaemon = true) {
            val buffer = ByteArray(maxOf(socket.maxTransmitPacketSize, BUFFER))
            try {
                val output = socket.outputStream
                while (true) {
                    val read = Os.read(ours, buffer, 0, buffer.size)
                    if (read <= 0) break
                    output.write(buffer, 0, read)
                }
            } catch (_: Exception) {
                // As above.
            } finally {
                ended()
            }
        }
        Log.i(TAG, "L2CAP to $address: up, tx ${socket.maxTransmitPacketSize} rx ${socket.maxReceivePacketSize}")
        val fd = ParcelFileDescriptor.dup(theirs).detachFd()
        Os.close(theirs)
        return fd
    }

    /**
     * The encrypted socket first when the phone and desktop are already paired, since Android may refuse an
     * unencrypted one to a bonded device; else the unencrypted one, which needs no pairing because Link's TLS
     * authenticates both ends. A failure refreshes the desktop's service list, which Android caches from pairing and
     * which predates Link's profile.
     */
    private fun connect(device: BluetoothDevice): BluetoothSocket? {
        val bonded = device.bondState == BluetoothDevice.BOND_BONDED
        val attempts = if (bonded) listOf(true, false) else listOf(false)
        for (secure in attempts) {
            val socket =
                if (secure) {
                    device.createRfcommSocketToServiceRecord(SERVICE)
                } else {
                    device.createInsecureRfcommSocketToServiceRecord(SERVICE)
                }
            try {
                socket.connect()
                return socket
            } catch (error: IOException) {
                Log.i(TAG, "RFCOMM to ${device.address} (${if (secure) "encrypted" else "unencrypted"}): ${error.message}")
                runCatching { socket.close() }
            }
        }
        device.fetchUuidsWithSdp()
        return null
    }

    /**
     * Pairs with [device] unless already paired, and waits for the user to confirm it on both screens: the desktop
     * accepts connections only from devices it is paired with.
     */
    private fun bonded(device: BluetoothDevice): Boolean {
        if (device.bondState == BluetoothDevice.BOND_BONDED) return true
        if (device.bondState == BluetoothDevice.BOND_NONE && !device.createBond()) {
            Log.i(TAG, "Android did not start pairing with ${device.address}")
            return false
        }
        val deadline = SystemClock.elapsedRealtime() + BOND_TIMEOUT_MS
        while (device.bondState == BluetoothDevice.BOND_BONDING && SystemClock.elapsedRealtime() < deadline) {
            SystemClock.sleep(POLL_MS)
        }
        Log.i(TAG, "pairing with ${device.address}: ${if (device.bondState == BluetoothDevice.BOND_BONDED) "paired" else "not paired"}")
        return device.bondState == BluetoothDevice.BOND_BONDED
    }

    private fun allowed(): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
            context.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED

    private fun pump(
        name: String,
        from: InputStream,
        to: OutputStream,
        done: () -> Unit,
    ) {
        thread(name = name, isDaemon = true) {
            val buffer = ByteArray(BUFFER)
            try {
                while (true) {
                    val read = from.read(buffer)
                    if (read < 0) break
                    to.write(buffer, 0, read)
                    to.flush()
                }
            } catch (_: IOException) {
                // Either side closing ends the link; the core sees it as the connection lost.
            } finally {
                done()
            }
        }
    }

    private companion object {
        const val TAG = "RfcommLink"

        /** Long enough for the user to read the code on both screens and confirm. */
        const val BOND_TIMEOUT_MS = 60_000L
        const val POLL_MS = 250L
        const val NOT_CONNECTED = -1
        const val NO_PERMISSION = -2
        const val BLUETOOTH_OFF = -3
        const val NOT_BONDED = -4
        const val BUFFER = 16 * 1024
        const val L2CAP_CONNECT_MS = 5_000L

        /** The desktop's `bluetooth::SERVICE_UUID`. */
        val SERVICE: UUID = UUID.fromString("5c3b1e5a-7d2f-4a8e-9b61-3f0c2d4e8a17")
    }
}
