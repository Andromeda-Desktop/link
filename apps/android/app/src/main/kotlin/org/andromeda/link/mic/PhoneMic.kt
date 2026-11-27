package org.andromeda.link.mic

import android.content.Context
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import org.andromeda.link.R
import org.andromeda.link.capture.CaptureAsk
import org.andromeda.link.capture.CaptureRequests
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.MicCommand
import org.andromeda.link.notifications.Channels
import org.andromeda.link.notifications.NotificationId

/** Desktops' microphone requests, through [CaptureRequests], and their ends to [MicService]. */
class PhoneMic(
    private val context: Context,
    private val repository: LinkRepository,
    private val scope: CoroutineScope,
) {
    // mic-stop carries no reason, so a refusal reaches the desktop as a plain stop.
    val requests =
        CaptureRequests(
            context,
            repository,
            scope,
            CaptureAsk(
                tag = TAG,
                channel = Channels.MIC,
                notification = NotificationId.MicRequest.id,
                refuseAction = "org.andromeda.link.MIC_REFUSE",
                title = R.string.mic_ask_title,
                text = R.string.mic_ask_text,
                start = R.string.mic_start,
                notNow = R.string.mic_not_now,
            ),
            { MicService.running },
        ) { desktopId, _ -> repository.stopMic(desktopId) }

    fun start() {
        requests.start()
        scope.launch { repository.mic.collect(::on) }
    }

    private fun on(command: MicCommand) {
        when (command) {
            is MicCommand.Requested -> {
                Log.i(TAG, "${command.desktopId} asks for the microphone")
                requests.request(command.desktopId) { MicStartActivity.intent(context, command.desktopId) }
            }
            is MicCommand.Stopped -> {
                requests.stopped(command.desktopId)
                MicService.stop(context, command.desktopId)
            }
        }
    }

    private companion object {
        const val TAG = "PhoneMic"
    }
}
