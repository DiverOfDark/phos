package dev.phos.android

import android.app.Application
import androidx.hilt.work.HiltWorkerFactory
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.work.Configuration
import coil3.ImageLoader
import coil3.SingletonImageLoader
import coil3.disk.DiskCache
import coil3.memory.MemoryCache
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import dagger.hilt.android.HiltAndroidApp
import dev.phos.android.data.repository.TokenRefreshManager
import dev.phos.android.data.repository.UpdateRepository
import dev.phos.android.sync.UpdateCheckWorker
import dev.phos.android.update.InstallOutcomes
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import okio.Path.Companion.toOkioPath
import javax.inject.Inject

@HiltAndroidApp
class PhosApplication : Application(), Configuration.Provider, SingletonImageLoader.Factory {

    @Inject lateinit var workerFactory: HiltWorkerFactory
    @Inject lateinit var okHttpClient: OkHttpClient
    @Inject lateinit var tokenRefreshManager: TokenRefreshManager
    @Inject lateinit var updates: UpdateRepository
    @Inject lateinit var installOutcomes: InstallOutcomes

    private val appScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    override val workManagerConfiguration: Configuration
        get() = Configuration.Builder()
            .setWorkerFactory(workerFactory)
            .build()

    override fun onCreate() {
        super.onCreate()
        UpdateCheckWorker.createNotificationChannel(this)
        UpdateCheckWorker.enqueue(this)

        // PackageInstaller answers on a BroadcastReceiver, which the platform may
        // instantiate long after the screen that started the install is gone.
        // Collecting process-wide means the result lands in UpdateRepository either way.
        appScope.launch {
            installOutcomes.outcomes.collect { updates.onInstallOutcome(it) }
        }

        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                appScope.launch {
                    // Renew the session before the UI (and Coil's thumbnail requests)
                    // start hitting the API.
                    tokenRefreshManager.ensureFreshToken()
                    // Then, and least urgently: has the server shipped a newer APK?
                    // Throttled to once an hour and swallows its own failures — a
                    // self-hosted server that is down must not make the app look broken.
                    runCatching { updates.checkQuietly() }
                }
            }
        })
    }

    override fun newImageLoader(context: android.content.Context): ImageLoader {
        return ImageLoader.Builder(context)
            .components {
                add(OkHttpNetworkFetcherFactory(callFactory = { okHttpClient }))
            }
            .memoryCache {
                MemoryCache.Builder()
                    .maxSizePercent(context, 0.25)
                    .build()
            }
            .diskCache {
                DiskCache.Builder()
                    .directory(java.io.File(cacheDir, "image_cache").toOkioPath())
                    .maxSizeBytes(512L * 1024 * 1024) // 512 MB
                    .build()
            }
            .build()
    }
}
