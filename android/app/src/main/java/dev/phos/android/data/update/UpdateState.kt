package dev.phos.android.data.update

import java.io.File

sealed interface UpdateState {
    data object Idle : UpdateState
    data object Checking : UpdateState
    data class Available(
        val version: String,
        val downloadUrl: String,
        val assetSize: Long,
    ) : UpdateState
    data object UpToDate : UpdateState
    data class Downloading(val progress: Float) : UpdateState
    data class ReadyToInstall(val apkFile: File) : UpdateState
    data class Error(val message: String) : UpdateState
}
