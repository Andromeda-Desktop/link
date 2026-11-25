package org.andromeda.link.control

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.RemoteUnit
import org.andromeda.link.core.domain.SpeakerStream

/** Plays desktops' sound on this phone's speaker; a desktop's new stream replaces its last. */
class PhoneSpeaker(
    private val repository: LinkRepository,
    private val scope: CoroutineScope,
) {
    /** Touched only by the collector, one stream at a time. */
    private val playing = mutableMapOf<String, Job>()

    fun start() {
        scope.launch { repository.speaker.collect(::play) }
    }

    private fun play(stream: SpeakerStream) {
        playing.remove(stream.desktopId)?.cancel()
        playing[stream.desktopId] =
            scope.launch(Dispatchers.IO) {
                Log.i(TAG, "${stream.desktopId} plays its sound here")
                // A full queue drops the oldest packets, so the sound never falls further behind than QUEUE.
                val packets = Channel<RemoteUnit>(QUEUE, BufferOverflow.DROP_OLDEST)
                try {
                    coroutineScope {
                        launch { playSound(packets) }
                        while (true) {
                            val unit = stream.sound.next() ?: break
                            if (unit.audio) packets.trySend(unit)
                        }
                        packets.close()
                    }
                } finally {
                    withContext(NonCancellable) { stream.sound.stop() }
                    Log.i(TAG, "${stream.desktopId}'s sound ended")
                }
            }
    }
}

private const val TAG = "PhoneSpeaker"

/** 10 ms packets: 60 ms of sound at most waits for the decoder. */
private const val QUEUE = 6
