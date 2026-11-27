package org.andromeda.link.webcam

import android.content.Context
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import org.andromeda.link.R
import org.andromeda.link.capture.CaptureAsk
import org.andromeda.link.capture.CaptureRequests
import org.andromeda.link.core.data.LinkRepository
import org.andromeda.link.core.domain.CameraCommand
import org.andromeda.link.notifications.Channels
import org.andromeda.link.notifications.NotificationId

/** Desktops' webcam requests, through [CaptureRequests], and their ends and keyframe asks to [CameraService]. */
class PhoneWebcam(
    private val context: Context,
    private val repository: LinkRepository,
    private val scope: CoroutineScope,
) {
    val requests =
        CaptureRequests(
            context,
            repository,
            scope,
            CaptureAsk(
                tag = TAG,
                channel = Channels.WEBCAM,
                notification = NotificationId.WebcamRequest.id,
                refuseAction = "org.andromeda.link.WEBCAM_REFUSE",
                title = R.string.webcam_ask_title,
                text = R.string.webcam_ask_text,
                start = R.string.webcam_start,
                notNow = R.string.webcam_not_now,
            ),
            { CameraService.running },
        ) { desktopId, reason -> repository.stopCamera(desktopId, reason) }

    fun start() {
        requests.start()
        scope.launch { repository.camera.collect(::on) }
    }

    private fun on(command: CameraCommand) {
        when (command) {
            is CameraCommand.Requested -> {
                Log.i(TAG, "${command.desktopId} asks for the ${if (command.front) "front" else "back"} camera")
                // A desktop already streaming asks for the other camera: the user agreed once, so it just switches.
                if (CameraService.switch(command.desktopId, command.front)) return
                requests.request(command.desktopId) { asked -> CameraStartActivity.intent(context, command, asked) }
            }
            is CameraCommand.Stopped -> {
                requests.stopped(command.desktopId)
                CameraService.stop(command.desktopId)
            }
            is CameraCommand.Keyframe -> CameraService.keyframe(command.desktopId)
        }
    }

    private companion object {
        const val TAG = "PhoneWebcam"
    }
}
