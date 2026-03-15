package dev.phos.android.ui.settings

import android.content.Context
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.phos.android.data.local.PhosDatabase
import dev.phos.android.data.repository.AuthRepository
import dev.phos.android.sync.SyncWorker
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.io.File
import javax.inject.Inject

data class SettingsUiState(
    val serverUrl: String = "",
    val cacheSize: String = "Calculating...",
    val isClearing: Boolean = false,
    val isClearingMetadata: Boolean = false,
    val wifiOnlySync: Boolean = false,
)

@HiltViewModel
class SettingsViewModel @Inject constructor(
    private val authRepository: AuthRepository,
    private val database: PhosDatabase,
    @ApplicationContext private val context: Context,
) : ViewModel() {

    private val _uiState = MutableStateFlow(SettingsUiState())
    val uiState: StateFlow<SettingsUiState> = _uiState.asStateFlow()

    init {
        _uiState.value = SettingsUiState(
            serverUrl = authRepository.getServerUrl() ?: "",
            wifiOnlySync = authRepository.isWifiOnlySync(),
        )
        calculateCacheSize()
    }

    private fun calculateCacheSize() {
        viewModelScope.launch {
            val cacheDir = File(context.cacheDir, "image_cache")
            val size = if (cacheDir.exists()) {
                cacheDir.walkTopDown().filter { it.isFile }.sumOf { it.length() }
            } else 0L
            _uiState.value = _uiState.value.copy(
                cacheSize = formatSize(size)
            )
        }
    }

    fun clearCache() {
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(isClearing = true)
            val cacheDir = File(context.cacheDir, "image_cache")
            if (cacheDir.exists()) {
                cacheDir.deleteRecursively()
            }
            _uiState.value = _uiState.value.copy(isClearing = false, cacheSize = "0 B")
        }
    }

    fun clearMetadataCache() {
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(isClearingMetadata = true)
            database.clearAllTables()
            _uiState.value = _uiState.value.copy(isClearingMetadata = false)
        }
    }

    fun setWifiOnlySync(enabled: Boolean) {
        authRepository.setWifiOnlySync(enabled)
        _uiState.value = _uiState.value.copy(wifiOnlySync = enabled)
        // Re-enqueue sync worker with updated constraints
        SyncWorker.enqueue(context, wifiOnly = enabled)
    }

    fun logout() {
        authRepository.logout()
    }

    private fun formatSize(bytes: Long): String = when {
        bytes >= 1_073_741_824 -> "%.1f GB".format(bytes / 1_073_741_824.0)
        bytes >= 1_048_576 -> "%.1f MB".format(bytes / 1_048_576.0)
        bytes >= 1024 -> "%.1f KB".format(bytes / 1024.0)
        else -> "$bytes B"
    }
}
