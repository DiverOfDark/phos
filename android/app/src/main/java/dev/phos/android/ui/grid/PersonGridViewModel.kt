package dev.phos.android.ui.grid

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.domain.model.Shot
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import javax.inject.Inject

data class GridTile(
    val shot: Shot,
    val cover: MediaFile?,
)

data class PersonGridUiState(
    val personName: String? = null,
    val tiles: List<GridTile> = emptyList(),
    val isLoading: Boolean = true,
    val error: String? = null,
    val lastViewedShotIndex: Int = 0,
)

@HiltViewModel
class PersonGridViewModel @Inject constructor(
    savedStateHandle: SavedStateHandle,
    private val browseRepository: BrowseRepository,
) : ViewModel() {

    private val personId: String = savedStateHandle["personId"] ?: ""

    private val _uiState = MutableStateFlow(PersonGridUiState())
    val uiState: StateFlow<PersonGridUiState> = _uiState.asStateFlow()

    init {
        load()
    }

    fun load() {
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(isLoading = true, error = null)
            try {
                val data = browseRepository.fetchBrowseData(personId)
                val tiles = data.shots.map { s -> GridTile(shot = s.shot, cover = s.files.firstOrNull()) }
                val saved = browseRepository.getViewPosition(personId)
                _uiState.value = PersonGridUiState(
                    personName = data.personName,
                    tiles = tiles,
                    isLoading = false,
                    lastViewedShotIndex = saved?.shotIndex?.coerceIn(0, maxOf(0, tiles.size - 1)) ?: 0,
                )
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "Failed to load: ${e.message}",
                )
            }
        }
    }

    /** Re-read the last-viewed shot index (updated by Browser while user was swiping there). */
    fun refreshLastViewedPosition() {
        val size = _uiState.value.tiles.size
        if (size == 0) return
        val saved = browseRepository.getViewPosition(personId) ?: return
        _uiState.value = _uiState.value.copy(
            lastViewedShotIndex = saved.shotIndex.coerceIn(0, size - 1),
        )
    }

    fun buildThumbnailUrl(fileId: String, width: Int = 320): String {
        return browseRepository.buildThumbnailUrl(fileId, width)
    }
}
