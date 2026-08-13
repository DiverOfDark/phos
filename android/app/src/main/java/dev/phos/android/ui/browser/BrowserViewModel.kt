package dev.phos.android.ui.browser

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import coil3.ImageLoader
import coil3.request.ImageRequest
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.data.repository.ShotRepository
import dev.phos.android.data.repository.ShotWithFiles
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.domain.model.Person
import dev.phos.android.domain.model.SimilarShot
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
    /** An organizing call is in flight; the sheets disable their buttons on it. */
    val busy: Boolean = false,
    /** One-shot text for the snackbar — both failures and confirmations. */
    val message: String? = null,
    val people: List<Person> = emptyList(),
    val peopleLoading: Boolean = false,
    val similar: List<SimilarShot> = emptyList(),
    val similarLoading: Boolean = false,
)

@HiltViewModel
class BrowserViewModel @Inject constructor(
    savedStateHandle: SavedStateHandle,
    private val browseRepository: BrowseRepository,
    private val shotRepository: ShotRepository,
    private val okHttpClient: OkHttpClient,
    @ApplicationContext private val appContext: android.content.Context,
) : ViewModel() {

    private val personId: String = savedStateHandle["personId"] ?: ""
    private val requestedShotIndex: Int = savedStateHandle["shot"] ?: -1
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
                val maxShotIndex = maxOf(0, data.shots.size - 1)
                val initialShot = if (requestedShotIndex >= 0) {
                    requestedShotIndex.coerceIn(0, maxShotIndex)
                } else {
                    savedPosition?.shotIndex?.coerceIn(0, maxShotIndex) ?: 0
                }
                val initialFile = if (requestedShotIndex >= 0) 0 else (savedPosition?.fileIndex ?: 0)
                _uiState.value = BrowserUiState(
                    personName = data.personName,
                    shots = data.shots,
                    isLoading = false,
                    initialShotIndex = initialShot,
                    initialFileIndex = initialFile,
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

    /**
     * Deletes the variant at [fileIndex]. The shot's original is not deletable this
     * way — deleting that means deleting the shot.
     */
    fun deleteFile(shotIndex: Int, fileIndex: Int) {
        val shots = _uiState.value.shots
        if (shotIndex !in shots.indices) return
        val shot = shots[shotIndex]
        if (fileIndex !in shot.files.indices) return
        val file = shot.files[fileIndex]
        if (file.isOriginal) return

        organize("Variant deleted") { browseRepository.deleteFile(file.id) }
    }

    // ---- organizing ------------------------------------------------------
    //
    // Every one of these follows the same shape: run, then re-read the list from
    // the server. Local patching would be faster, but a merge or a split changes
    // which shots this person even has, and a list that disagrees with the server
    // is how you delete the wrong thing next.

    /** Loads the people list for the reassign picker; cheap enough to re-ask each time. */
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

    /** Loads merge candidates for [shotId]. */
    fun loadSimilar(shotId: String) {
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(similarLoading = true, similar = emptyList())
            try {
                _uiState.value = _uiState.value.copy(
                    similar = shotRepository.similarShots(shotId),
                    similarLoading = false,
                )
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    similarLoading = false,
                    message = "Couldn't look for similar shots: ${e.message}",
                )
            }
        }
    }

    fun moveToPerson(shotId: String, personId: String, personName: String?) {
        organize("Moved to ${personName ?: "another person"}") {
            shotRepository.moveToPerson(shotId, personId)
        }
    }

    /** Creates a person and moves the shot to them, as one action. */
    fun createPersonAndMove(shotId: String, name: String) {
        organize("Moved to $name") {
            val personId = shotRepository.createPerson(name)
            shotRepository.moveToPerson(shotId, personId)
        }
    }

    fun splitShot(shotId: String, fileIds: List<String>) {
        organize("Split ${fileIds.size} file(s) into a new shot") {
            shotRepository.split(shotId, fileIds)
        }
    }

    /** Folds [sourceShotId] into the shot on screen, which survives. */
    fun mergeInto(targetShotId: String, sourceShotId: String) {
        organize("Merged") {
            shotRepository.merge(sourceId = sourceShotId, targetId = targetShotId)
        }
    }

    fun deleteShot(shotId: String) {
        organize("Shot deleted") {
            shotRepository.deleteShot(shotId)
        }
    }

    /**
     * Runs one organizing call, then reloads.
     *
     * There is no offline queue and no retry: if the call fails the user is told
     * what happened and nothing has changed, which is the honest outcome and the
     * one they can act on.
     */
    private fun organize(successMessage: String, block: suspend () -> Unit) {
        if (_uiState.value.busy) return
        viewModelScope.launch {
            _uiState.value = _uiState.value.copy(busy = true)
            try {
                block()
                reload(successMessage)
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    busy = false,
                    message = "Failed: ${e.message}",
                )
            }
        }
    }

    private suspend fun reload(message: String?) {
        try {
            val data = browseRepository.fetchBrowseData(personId)
            _uiState.value = _uiState.value.copy(
                personName = data.personName,
                shots = data.shots,
                busy = false,
                message = message,
            )
        } catch (e: Exception) {
            // The change itself went through — say so, and say that the screen is
            // now the stale half of the story.
            _uiState.value = _uiState.value.copy(
                busy = false,
                message = "Done, but reloading failed: ${e.message}",
            )
        }
    }

    /** Clears the snackbar text once it has been shown. */
    fun consumeMessage() {
        _uiState.value = _uiState.value.copy(message = null)
    }

    /** Absolute URL for a server-relative thumbnail path (merge candidates). */
    fun absoluteUrl(path: String): String = browseRepository.absoluteUrl(path)

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
