package org.andromeda.link.update

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.launch
import java.util.concurrent.TimeUnit

/** The daily release check: Android runs it in its own batch windows, on any network, never on low battery. */
class UpdateJob : JobService() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    override fun onStartJob(params: JobParameters): Boolean {
        scope.launch {
            AppUpdate.checkAndNotify(applicationContext)
            jobFinished(params, false)
        }
        return true
    }

    override fun onStopJob(params: JobParameters): Boolean {
        scope.coroutineContext.cancelChildren()
        return true
    }

    companion object {
        private const val ID = 7301

        fun schedule(context: Context) {
            val jobs = context.getSystemService(JobScheduler::class.java)
            if (jobs.getPendingJob(ID) != null) return
            jobs.schedule(
                JobInfo
                    .Builder(ID, ComponentName(context, UpdateJob::class.java))
                    .setPeriodic(TimeUnit.DAYS.toMillis(1))
                    .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                    .setRequiresBatteryNotLow(true)
                    .setPersisted(true)
                    .build(),
            )
        }
    }
}
