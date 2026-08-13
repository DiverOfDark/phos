package dev.phos.android.ui.review

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.data.repository.ReviewRepository
import dev.phos.android.data.repository.ShotRepository
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.FaceSuggestion
import dev.phos.android.domain.model.PendingShot
import dev.phos.android.domain.model.Person
import dev.phos.android.domain.model.ShotDetail
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import javax.inject.Inject

data class ReviewUiState(
    val queue: List<PendingShot> = emptyList(),
    val index: Int = 0,
    val detail: ShotDetail? = null,
    val isLoading: Boolean = true,
    val isLoadingDetail: Boolean = false,
    val busy: Boolean = false,
    val error: String? = null,
    val message: String? = null,
    val people: List<Person> = emptyList(),
    val peopleLoading: Boolean = false,
    /** The face whose sheet is open, if any. */
    val activeFace: Face? = null,
    val suggestions: List<FaceSuggestion> = emptyList(),
    val suggestionsLoading: Boolean = false,
    /** How many shots have been dealt with since the screen opened. */
    val reviewed: Int = 0,
) {
    val current: PendingShot? get() = queue.getOrNull(index)
    val remaining: Int get() = queue.size
    val isEmpty: Boolean get() = queue.isEmpty()
}

/**
 * The review queue: pending shots, one at a time, with the fastest path to a verdict.
 *
 * Every verdict — confirm, reassign, unsort, delete — removes the shot from the queue
 * and moves to the next one, because the queue *is* the progress bar. Skip is the one
 * action that leaves the shot in place: it means "not now", not "fine".
 *
 * Face edits are the exception. They do not advance, because correcting one face in a
 * group photo is usually the first of several, and the reviewer is still working on
 * the same picture.
 */
@HiltViewModel
class ReviewViewModel @Inject constructor(
    private val reviewRepository: ReviewRepository,
    private val shotRepository: ShotRepository,
    private val browseRepository: BrowseRepository,
) : ViewModel() {

    private val _uiState = MutableStateFlow(ReviewUiState())
    val uiState: StateFlow<ReviewUiState> = _uiState.asStateFlow()

    init {
        load()
    }

    fun load() {
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(isLoading = true, error = null)
            try {
                val queue = reviewRepository.pendingShots()
                _uiState.value = _uiState.value.copy(
                    queue = queue,
                    index = 0,
                    isLoading = false,
                )
                loadDetail()
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "Couldn't load the review queue: ${e.message}",
                )
            }
        }
    }

    /** Detail for whatever is under the cursor now. */
    private fun loadDetail() {
        val shot = _uiState.value.current ?: run {
            _uiState.value = _uiState.value.copy(detail = null)
            return
        }
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(isLoadingDetail = true)
            try {
                val detail = reviewRepository.shotDetail(shot.id)
                // Guard against a stale response: the user can step forward while a
                // detail request is still in flight, and the late answer must not
                // paint someone else's faces over the picture on screen.
                if (_uiState.value.current?.id == shot.id) {
                    _uiState.value = _uiState.value.copy(detail = detail, isLoadingDetail = false)
                }
            } catch (e: Exception) {
                if (_uiState.value.current?.id == shot.id) {
                    _uiState.value = _uiState.value.copy(
                        isLoadingDetail = false,
                        message = "Couldn't load this shot: ${e.message}",
                    )
                }
            }
        }
    }

    // ---- moving through the queue ----------------------------------------

    /** "Not now." Leaves the shot pending and steps past it. */
    fun skip() {
        val state = _uiState.value
        if (state.queue.isEmpty()) return
        val next = (state.index + 1) % state.queue.size
        _uiState.value = state.copy(index = next, detail = null)
        loadDetail()
    }

    fun previous() {
        val state = _uiState.value
        if (state.queue.isEmpty()) return
        val previous = if (state.index == 0) state.queue.lastIndex else state.index - 1
        _uiState.value = state.copy(index = previous, detail = null)
        loadDetail()
    }

    // ---- verdicts ---------------------------------------------------------

    fun confirm() {
        val shot = _uiState.value.current ?: return
        verdict("Confirmed") { shotRepository.confirm(shot.id) }
    }

    fun moveToPerson(personId: String, personName: String?) {
        val shot = _uiState.value.current ?: return
        verdict("Moved to ${personName ?: "another person"}") {
            shotRepository.moveToPerson(shot.id, personId)
        }
    }

    fun createPersonAndMove(name: String) {
        val shot = _uiState.value.current ?: return
        verdict("Moved to $name") {
            shotRepository.moveToPerson(shot.id, shotRepository.createPerson(name))
        }
    }

    /** "I can't tell who this is." Takes the person off and leaves it for later. */
    fun markUnsorted() {
        val shot = _uiState.value.current ?: return
        verdict("Left unsorted") { shotRepository.markUnsorted(shot.id) }
    }

    fun deleteShot() {
        val shot = _uiState.value.current ?: return
        verdict("Shot deleted") { shotRepository.deleteShot(shot.id) }
    }

    fun split(fileIds: List<String>) {
        val shot = _uiState.value.current ?: return
        // Splitting is not a verdict: the shot is still here and still unreviewed,
        // so the screen reloads it rather than advancing.
        act("Split ${fileIds.size} file(s) out") {
            shotRepository.split(shot.id, fileIds)
            loadDetail()
        }
    }

    /**
     * Runs an action that settles the current shot, then drops it from the queue.
     *
     * Removing locally rather than re-fetching keeps the reviewer's place: a reload
     * would renumber everything under them, and the queue is long enough that losing
     * your position is the difference between finishing and giving up.
     */
    private fun verdict(successMessage: String, block: suspend () -> Unit) {
        if (_uiState.value.busy) return
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(busy = true)
            try {
                block()
                val state = _uiState.value
                val queue = state.queue.toMutableList()
                if (state.index in queue.indices) queue.removeAt(state.index)
                val index = state.index.coerceAtMost(maxOf(0, queue.size - 1))
                _uiState.value = state.copy(
                    queue = queue,
                    index = index,
                    detail = null,
                    busy = false,
                    reviewed = state.reviewed + 1,
                    message = successMessage,
                )
                loadDetail()
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(busy = false, message = "Failed: ${e.message}")
            }
        }
    }

    /** An action that changes the shot but does not settle it. */
    private fun act(successMessage: String, block: suspend () -> Unit) {
        if (_uiState.value.busy) return
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(busy = true)
            try {
                block()
                _uiState.value = _uiState.value.copy(busy = false, message = successMessage)
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(busy = false, message = "Failed: ${e.message}")
            }
        }
    }

    // ---- faces ------------------------------------------------------------

    fun openFace(face: Face) {
        _uiState.value = _uiState.value.copy(
            activeFace = face,
            suggestions = emptyList(),
            suggestionsLoading = true,
        )
        loadPeople()
        viewModelScope.launch {
            try {
                val suggestions = reviewRepository.faceSuggestions(face.id)
                if (_uiState.value.activeFace?.id == face.id) {
                    _uiState.value = _uiState.value.copy(
                        suggestions = suggestions,
                        suggestionsLoading = false,
                    )
                }
            } catch (e: Exception) {
                // Suggestions are an accelerator, not a requirement — the full
                // people list is still right there, so this failure is not worth a
                // message on top of an already-open sheet.
                if (_uiState.value.activeFace?.id == face.id) {
                    _uiState.value = _uiState.value.copy(suggestionsLoading = false)
                }
            }
        }
    }

    fun closeFace() {
        _uiState.value = _uiState.value.copy(activeFace = null, suggestions = emptyList())
    }

    fun assignFace(personId: String, personName: String?) {
        val face = _uiState.value.activeFace ?: return
        closeFace()
        act("Face assigned to ${personName ?: "another person"}") {
            reviewRepository.reassignFace(face.id, personId)
            loadDetail()
        }
    }

    fun createPersonAndAssignFace(name: String) {
        val face = _uiState.value.activeFace ?: return
        closeFace()
        act("Face assigned to $name") {
            reviewRepository.reassignFace(face.id, shotRepository.createPerson(name))
            loadDetail()
        }
    }

    fun deleteActiveFace() {
        val face = _uiState.value.activeFace ?: return
        closeFace()
        act("Face removed") {
            reviewRepository.deleteFace(face.id)
            loadDetail()
        }
    }

    // ---- shared -----------------------------------------------------------

    fun loadPeople() {
        if (_uiState.value.peopleLoading || _uiState.value.people.isNotEmpty()) return
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

    fun consumeMessage() {
        _uiState.value = _uiState.value.copy(message = null)
    }

    fun buildThumbnailUrl(fileId: String, width: Int = 1080): String =
        browseRepository.buildThumbnailUrl(fileId, width)

    fun absoluteUrl(path: String): String = browseRepository.absoluteUrl(path)
}
