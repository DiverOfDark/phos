package dev.phos.android.ui.settings

import android.content.Context
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.phos.android.data.repository.AuthRepository
import dev.phos.android.data.repository.ShotRepository
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
    /** A duplicate-box sweep is in flight. */
    val dedupeBusy: Boolean = false,
    /**
     * How many duplicate boxes the last dry run found, and so how many the
     * confirm button is about to delete. Zero means nothing is pending.
     */
    val dedupePending: Int = 0,
    val dedupeMessage: String? = null,
)

@HiltViewModel
class SettingsViewModel @Inject constructor(
    private val authRepository: AuthRepository,
    private val shotRepository: ShotRepository,
    @ApplicationContext private val context: Context,
) : ViewModel() {

    private val _uiState = MutableStateFlow(SettingsUiState())
    val uiState: StateFlow<SettingsUiState> = _uiState.asStateFlow()

    init {
        _uiState.value = SettingsUiState(serverUrl = authRepository.getServerUrl() ?: "")
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

    /**
     * Finds duplicate face boxes, then — on a second tap — removes them.
     *
     * Two steps because the delete is irreversible: the first tap only counts, and
     * the button that follows names the exact number it will remove.
     */
    fun findDuplicateFaces() = dedupe(dryRun = true)

    fun removeDuplicateFaces() = dedupe(dryRun = false)

    private fun dedupe(dryRun: Boolean) {
        if (_uiState.value.dedupeBusy) return
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(dedupeBusy = true, dedupeMessage = null)
            _uiState.value = try {
                val count = shotRepository.dedupeFaces(dryRun)
                if (dryRun) {
                    _uiState.value.copy(
                        dedupeBusy = false,
                        dedupePending = count,
                        dedupeMessage = if (count == 0) "No duplicate boxes found." else null,
                    )
                } else {
                    _uiState.value.copy(
                        dedupeBusy = false,
                        dedupePending = 0,
                        dedupeMessage = "Removed $count duplicate box(es).",
                    )
                }
            } catch (e: Exception) {
                _uiState.value.copy(
                    dedupeBusy = false,
                    dedupeMessage = "Failed: ${e.message}",
                )
            }
        }
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
