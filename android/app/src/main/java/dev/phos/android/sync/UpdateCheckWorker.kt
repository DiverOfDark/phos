package dev.phos.android.sync

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.hilt.work.HiltWorker
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import dev.phos.android.R
import dev.phos.android.data.repository.UpdateRepository
import dev.phos.android.update.UpdateState
import java.util.concurrent.TimeUnit

/**
 * The background half of the in-app updater: notices a newer build on the configured
 * server while the app is closed, and says so once.
 *
 * The foreground half lives in the settings screen, which reads the same
 * [UpdateRepository] state. This worker only notifies — downloading and installing
 * always needs a person, because Android's own install confirmation is part of the
 * flow by design.
 */
@HiltWorker
class UpdateCheckWorker @AssistedInject constructor(
    @Assisted private val appContext: Context,
    @Assisted workerParams: WorkerParameters,
    private val updateRepository: UpdateRepository,
) : CoroutineWorker(appContext, workerParams) {

    override suspend fun doWork(): Result {
        // Before the first login there is nothing to ask; retrying would just burn
        // wakeups until the user gets around to configuring a server.
        if (!updateRepository.hasServer) return Result.success()

        return when (val state = updateRepository.check()) {
            is UpdateState.Available -> {
                showUpdateNotification(state.versionName)
                Result.success()
            }
            // A server that is briefly unreachable, or one whose advertisement this
            // build can't verify, is worth one retry — not a notification.
            is UpdateState.Failed -> Result.retry()
            else -> Result.success()
        }
    }

    private fun showUpdateNotification(versionName: String) {
        val notificationManager =
            appContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

        val notification = NotificationCompat.Builder(appContext, CHANNEL_ID)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("Phos update available")
            .setContentText("Version $versionName is on your server. Open Settings to install it.")
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setAutoCancel(true)
            .build()

        notificationManager.notify(NOTIFICATION_ID, notification)
    }

    companion object {
        private const val WORK_NAME = "phos_update_check"
        const val CHANNEL_ID = "phos_updates"
        private const val NOTIFICATION_ID = 1001

        fun createNotificationChannel(context: Context) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val channel = NotificationChannel(
                    CHANNEL_ID,
                    "App Updates",
                    NotificationManager.IMPORTANCE_LOW,
                ).apply {
                    description = "Notifications for new Phos app versions"
                }
                val notificationManager =
                    context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
                notificationManager.createNotificationChannel(channel)
            }
        }

        fun enqueue(context: Context) {
            val constraints = Constraints.Builder()
                .setRequiredNetworkType(NetworkType.CONNECTED)
                .build()

            val request = PeriodicWorkRequestBuilder<UpdateCheckWorker>(24, TimeUnit.HOURS)
                .setConstraints(constraints)
                .build()

            WorkManager.getInstance(context).enqueueUniquePeriodicWork(
                WORK_NAME,
                ExistingPeriodicWorkPolicy.KEEP,
                request,
            )
        }
    }
}
