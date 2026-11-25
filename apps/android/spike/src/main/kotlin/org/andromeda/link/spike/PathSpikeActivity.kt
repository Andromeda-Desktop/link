package org.andromeda.link.spike

import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants
import android.util.Log
import org.andromeda.link.ffi.runPathSpike
import java.io.FileDescriptor
import java.io.IOException
import kotlin.concurrent.thread

/**
 * The channel spike (link/.agent/TASK-reliability.md step 0), its own package beside the installed app:
 * `adb shell am start -n org.andromeda.link.spike/.PathSpikeActivity --es desktop <ip:port>
 * [--es bt <AA:BB:..> --ei psm 197]`. The report goes to logcat under `link-spike`. An activity, not a receiver:
 * Android cuts network for a cached process a few seconds after the receiver returns.
 */
class PathSpikeActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val context: Context = this
        val desktop = intent.getStringExtra("desktop") ?: return
        val bt = intent.getStringExtra("bt")
        val psm = intent.getIntExtra("psm", DEFAULT_PSM)
        thread(name = "path-spike") {
            val fd =
                try {
                    bt?.let { open(context, it, psm) } ?: -1
                } catch (error: IOException) {
                    Log.w(TAG, "l2cap: ${error.message}")
                    -1
                }
            val report = java.io.File(context.filesDir, "spike.txt")
            runPathSpike(desktop, fd, report.path).lines().forEach { Log.i(TAG, it) }
        }
    }

    /** Connects an L2CAP channel and bridges it, one SDU per message, to a SOCK_SEQPACKET pair; returns Rust's end. */
    @SuppressLint("MissingPermission")
    private fun open(
        context: Context,
        address: String,
        psm: Int,
    ): Int {
        val adapter = context.getSystemService(BluetoothManager::class.java).adapter
        val device =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                adapter.getRemoteLeDevice(address, BluetoothDevice.ADDRESS_TYPE_PUBLIC)
            } else {
                adapter.getRemoteDevice(address)
            }
        val started = System.nanoTime()
        val socket = device.createInsecureL2capChannel(psm)
        socket.connect()
        Log.i(
            TAG,
            "l2cap up in ${(System.nanoTime() - started) / 1_000_000} ms, " +
                "tx ${socket.maxTransmitPacketSize} rx ${socket.maxReceivePacketSize}",
        )
        val ours = FileDescriptor()
        val theirs = FileDescriptor()
        Os.socketpair(OsConstants.AF_UNIX, OsConstants.SOCK_SEQPACKET, 0, ours, theirs)
        pump(socket, ours)
        val fd = ParcelFileDescriptor.dup(theirs).detachFd()
        Os.close(theirs)
        return fd
    }

    private fun pump(
        socket: BluetoothSocket,
        ours: FileDescriptor,
    ) {
        thread(name = "l2cap-in", isDaemon = true) {
            val buffer = ByteArray(maxOf(socket.maxReceivePacketSize, BUFFER))
            try {
                val input = socket.inputStream
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    Os.write(ours, buffer, 0, read)
                }
            } catch (error: Exception) {
                Log.i(TAG, "l2cap in ended: ${error.message}")
            }
            socket.close()
        }
        thread(name = "l2cap-out", isDaemon = true) {
            val buffer = ByteArray(BUFFER)
            val log = java.io.File(filesDir, "spike.txt.pump").printWriter()
            val start = System.nanoTime()
            var (second, written, packets, slowest) = listOf(0L, 0L, 0L, 0L)
            try {
                val output = socket.outputStream
                while (true) {
                    val read = Os.read(ours, buffer, 0, buffer.size)
                    if (read <= 0) break
                    val before = System.nanoTime()
                    output.write(buffer, 0, read)
                    val after = System.nanoTime()
                    written += read
                    packets++
                    slowest = maxOf(slowest, after - before)
                    if ((after - start) / 1_000_000_000 > second) {
                        second = (after - start) / 1_000_000_000
                        log.println("$second written $written packets $packets slowest_write_ms ${slowest / 1_000_000}")
                        log.flush()
                        slowest = 0
                    }
                }
            } catch (error: Exception) {
                Log.i(TAG, "l2cap out ended: ${error.message}")
            }
            socket.close()
        }
    }

    private companion object {
        const val TAG = "link-spike"
        const val DEFAULT_PSM = 0xC5
        const val BUFFER = 65_536
    }
}
