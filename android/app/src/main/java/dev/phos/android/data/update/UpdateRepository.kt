package dev.phos.android.data.update

import android.content.Context
import android.content.Intent
import android.os.Environment
import androidx.core.content.FileProvider
import dagger.hilt.android.qualifiers.ApplicationContext
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import javax.inject.Inject
import javax.inject.Named
import javax.inject.Singleton

@Singleton
class UpdateRepository @Inject constructor(
    @ApplicationContext private val context: Context,
    private val gitHubApi: GitHubApi,
    @Named("github") private val githubOkHttpClient: OkHttpClient,
) {

    suspend fun checkForUpdate(): UpdateState {
        return try {
            val release = gitHubApi.getLatestRelease()
            val apkAsset = release.assets.firstOrNull { it.name.endsWith(".apk") }
                ?: return UpdateState.Error("No APK found in latest release")

            val currentVersion = getCurrentVersion()
            if (isNewerVersion(release.tagName, currentVersion)) {
                UpdateState.Available(
                    version = release.tagName,
                    downloadUrl = apkAsset.browserDownloadUrl,
                    assetSize = apkAsset.size,
                )
            } else {
                UpdateState.UpToDate
            }
        } catch (e: Exception) {
            UpdateState.Error(e.message ?: "Failed to check for updates")
        }
    }

    suspend fun downloadApk(url: String, onProgress: (Float) -> Unit): File {
        val downloadDir = context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
            ?: throw IllegalStateException("External files directory not available")
        val apkFile = File(downloadDir, "phos-update.apk")
        if (apkFile.exists()) apkFile.delete()

        val request = Request.Builder().url(url).build()
        val response = githubOkHttpClient.newCall(request).execute()
        if (!response.isSuccessful) throw Exception("Download failed: ${response.code}")

        val body = response.body ?: throw Exception("Empty response body")
        val totalBytes = body.contentLength()
        var downloadedBytes = 0L

        body.byteStream().use { input ->
            apkFile.outputStream().use { output ->
                val buffer = ByteArray(8192)
                var bytesRead: Int
                while (input.read(buffer).also { bytesRead = it } != -1) {
                    output.write(buffer, 0, bytesRead)
                    downloadedBytes += bytesRead
                    if (totalBytes > 0) {
                        onProgress(downloadedBytes.toFloat() / totalBytes.toFloat())
                    }
                }
            }
        }

        return apkFile
    }

    fun installApk(context: Context, file: File) {
        val uri = FileProvider.getUriForFile(
            context,
            "${context.packageName}.fileprovider",
            file,
        )
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_GRANT_READ_URI_PERMISSION
        }
        context.startActivity(intent)
    }

    fun getCurrentVersion(): String {
        val packageInfo = context.packageManager.getPackageInfo(context.packageName, 0)
        return packageInfo.versionName ?: "0.0.0"
    }

    internal fun isNewerVersion(remote: String, current: String): Boolean {
        val remoteParts = remote.removePrefix("v").split(".").mapNotNull { it.toIntOrNull() }
        val currentParts = current.removePrefix("v").split(".").mapNotNull { it.toIntOrNull() }

        for (i in 0 until maxOf(remoteParts.size, currentParts.size)) {
            val r = remoteParts.getOrElse(i) { 0 }
            val c = currentParts.getOrElse(i) { 0 }
            if (r > c) return true
            if (r < c) return false
        }
        return false
    }
}
