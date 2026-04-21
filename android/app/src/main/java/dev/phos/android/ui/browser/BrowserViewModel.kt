package dev.phos.android.ui.browser

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import coil3.ImageLoader
import coil3.request.ImageRequest
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.data.repository.ShotWithFiles
import dev.phos.android.domain.model.MediaFile
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import javax.inject.Inject

data class BrowserUiState(
    val personName: String? = null,
    val shots: List<ShotWithFiles> = emptyList(),
    val isLoading: Boolean = true,
    val error: String? = null,
    val initialShotIndex: Int = 0,
    val initialFileIndex: Int = 0,
)

@HiltViewModel
class BrowserViewModel @Inject constructor(
    savedStateHandle: SavedStateHandle,
    private val browseRepository: BrowseRepository,
    private val okHttpClient: OkHttpClient,
    @ApplicationContext private val appContext: android.content.Context,
) : ViewModel() {

    private val personId: String = savedStateHandle["personId"] ?: ""
    private val _uiState = MutableStateFlow(BrowserUiState())
    val uiState: StateFlow<BrowserUiState> = _uiState.asStateFlow()

    init {
        loadBrowseData()
    }

    private fun loadBrowseData() {
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(isLoading = true, error = null)
            try {
                val data = browseRepository.fetchBrowseData(personId)
                val savedPosition = browseRepository.getViewPosition(personId)
                _uiState.value = BrowserUiState(
                    personName = data.personName,
                    shots = data.shots,
                    isLoading = false,
                    initialShotIndex = savedPosition?.shotIndex?.coerceIn(0, maxOf(0, data.shots.size - 1)) ?: 0,
                    initialFileIndex = savedPosition?.fileIndex ?: 0,
                )
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "Failed to load: ${e.message}",
                )
            }
        }
    }

    fun onShotChanged(shotIndex: Int, fileIndex: Int) {
        viewModelScope.launch {
            browseRepository.saveViewPosition(personId, shotIndex, fileIndex)
        }
        // Prefetch adjacent shots
        prefetchAround(shotIndex)
    }

    private fun prefetchAround(currentIndex: Int) {
        val shots = _uiState.value.shots
        if (shots.isEmpty()) return

        val imageLoader = ImageLoader(appContext)
        val indicesToPrefetch = listOf(
            currentIndex - 2, currentIndex - 1,
            currentIndex + 1, currentIndex + 2, currentIndex + 3,
        )

        for (idx in indicesToPrefetch) {
            if (idx < 0 || idx >= shots.size) continue
            val shot = shots[idx]
            for (file in shot.files) {
                val url = browseRepository.buildThumbnailUrl(file.id, 1080)
                val request = ImageRequest.Builder(appContext)
                    .data(url)
                    .build()
                imageLoader.enqueue(request)
            }
        }
    }

    fun deleteFile(shotIndex: Int, fileIndex: Int) {
        val shots = _uiState.value.shots
        if (shotIndex !in shots.indices) return
        val shot = shots[shotIndex]
        if (fileIndex !in shot.files.indices) return
        val file = shot.files[fileIndex]
        if (file.isOriginal) return

        viewModelScope.launch {
            try {
                browseRepository.deleteFile(file.id)
                val updatedFiles = shot.files.filterIndexed { i, _ -> i != fileIndex }
                val updatedShots = shots.toMutableList()
                updatedShots[shotIndex] = shot.copy(files = updatedFiles)
                _uiState.value = _uiState.value.copy(shots = updatedShots)
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(error = "Failed to delete: ${e.message}")
            }
        }
    }

    fun buildThumbnailUrl(fileId: String, width: Int = 1080): String {
        return browseRepository.buildThumbnailUrl(fileId, width)
    }

    fun buildOriginalUrl(fileId: String): String {
        return browseRepository.buildOriginalUrl(fileId)
    }

    fun getOkHttpClient(): OkHttpClient = okHttpClient

    fun isVideo(file: MediaFile): Boolean {
        return file.mimeType?.startsWith("video/") == true
    }
}
