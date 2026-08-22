package dev.phos.android.ui.grid

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.data.repository.ShotRepository
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.domain.model.Person
import dev.phos.android.domain.model.Shot
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import javax.inject.Inject

data class GridTile(
    val shot: Shot,
    val cover: MediaFile?,
    /**
     * How many files the shot holds.
     *
     * A tile is one *shot*, and a shot can hold several near-identical files —
     * the scanner groups them. Without this the grid looks like it has lost the
     * variants; with it the tile says how many are inside.
     */
    val fileCount: Int,
)

data class PersonGridUiState(
    val personName: String? = null,
    val tiles: List<GridTile> = emptyList(),
    val isLoading: Boolean = true,
    val error: String? = null,
    val lastViewedShotIndex: Int = 0,
    /** Shot ids the user has picked out for a batch action. */
    val selected: Set<String> = emptySet(),
    val busy: Boolean = false,
    val message: String? = null,
    val people: List<Person> = emptyList(),
    val peopleLoading: Boolean = false,
) {
    /** The grid is in selection mode exactly while something is selected. */
    val selectionMode: Boolean get() = selected.isNotEmpty()
}

@HiltViewModel
class PersonGridViewModel @Inject constructor(
    savedStateHandle: SavedStateHandle,
    private val browseRepository: BrowseRepository,
    private val shotRepository: ShotRepository,
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
                val tiles = data.shots.map { s ->
                    GridTile(shot = s.shot, cover = s.files.firstOrNull(), fileCount = s.files.size)
                }
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

    // ---- multi-select ----------------------------------------------------

    /** Long-press starts a selection; tapping while in one adds and removes. */
    fun toggleSelection(shotId: String) {
        val current = _uiState.value.selected
        _uiState.value = _uiState.value.copy(
            selected = if (shotId in current) current - shotId else current + shotId,
        )
    }

    fun clearSelection() {
        _uiState.value = _uiState.value.copy(selected = emptySet())
    }

    fun selectAll() {
        _uiState.value = _uiState.value.copy(
            selected = _uiState.value.tiles.map { it.shot.id }.toSet(),
        )
    }

    fun loadPeople() {
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(peopleLoading = true)
            try {
                _uiState.value = _uiState.value.copy(
                    people = shotRepository.people(),
                    peopleLoading = false,
                )
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    peopleLoading = false,
                    message = "Couldn't load people: ${e.message}",
                )
            }
        }
    }

    fun confirmSelected() {
        val ids = _uiState.value.selected.toList()
        if (ids.isEmpty()) return
        batch("Marked ${ids.size} shot(s) reviewed") { shotRepository.batchConfirm(ids) }
    }

    fun moveSelectedTo(personId: String, personName: String?) {
        val ids = _uiState.value.selected.toList()
        if (ids.isEmpty()) return
        batch("Moved ${ids.size} shot(s) to ${personName ?: "another person"}") {
            shotRepository.batchReassign(ids, personId)
        }
    }

    /** Creates a person and moves the selection to them in one action. */
    fun moveSelectedToNewPerson(name: String) {
        val ids = _uiState.value.selected.toList()
        if (ids.isEmpty()) return
        batch("Moved ${ids.size} shot(s) to $name") {
            shotRepository.batchReassign(ids, shotRepository.createPerson(name))
        }
    }

    /**
     * Deletes the selection.
     *
     * There is no batch-delete endpoint, so this can half-succeed; the message says
     * exactly how far it got rather than claiming a clean result.
     */
    fun deleteSelected() {
        val ids = _uiState.value.selected.toList()
        if (ids.isEmpty()) return
        if (_uiState.value.busy) return
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(busy = true)
            val outcome = shotRepository.deleteShots(ids)
            val message = when {
                outcome.failed == 0 -> "Deleted ${outcome.deleted} shot(s)"
                outcome.deleted == 0 -> "Delete failed: ${outcome.firstError}"
                else -> "Deleted ${outcome.deleted} of ${ids.size} — " +
                    "${outcome.failed} failed (${outcome.firstError})"
            }
            finishBatch(message)
        }
    }

    /**
     * Runs one batch action, then clears the selection and reloads.
     *
     * The selection is dropped even on failure: the shots it named may or may not
     * still be what the user thinks they are, and a stale selection is the thing
     * standing between a retry and deleting the wrong shot.
     */
    private fun batch(successMessage: String, block: suspend () -> Unit) {
        if (_uiState.value.busy) return
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(busy = true)
            val message = try {
                block()
                successMessage
            } catch (e: Exception) {
                "Failed: ${e.message}"
            }
            finishBatch(message)
        }
    }

    private suspend fun finishBatch(message: String) {
        try {
            val data = browseRepository.fetchBrowseData(personId)
            _uiState.value = _uiState.value.copy(
                personName = data.personName,
                tiles = data.shots.map {
                    GridTile(shot = it.shot, cover = it.files.firstOrNull(), fileCount = it.files.size)
                },
                selected = emptySet(),
                busy = false,
                message = message,
            )
        } catch (e: Exception) {
            _uiState.value = _uiState.value.copy(
                selected = emptySet(),
                busy = false,
                message = "$message, but reloading failed: ${e.message}",
            )
        }
    }

    fun consumeMessage() {
        _uiState.value = _uiState.value.copy(message = null)
    }
}
