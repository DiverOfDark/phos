package dev.phos.android.ui.people

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.phos.android.data.repository.AuthRepository
import dev.phos.android.data.repository.PeopleRepository
import dev.phos.android.domain.model.Person
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import javax.inject.Inject

@HiltViewModel
class PeopleViewModel @Inject constructor(
    private val peopleRepository: PeopleRepository,
    private val authRepository: AuthRepository,
) : ViewModel() {

    private val _people = MutableStateFlow<List<Person>>(emptyList())
    val people: StateFlow<List<Person>> = _people.asStateFlow()

    private val _isRefreshing = MutableStateFlow(false)
    val isRefreshing: StateFlow<Boolean> = _isRefreshing.asStateFlow()

    /**
     * Shots with no person yet — the phone's copy of the web's "Unsorted" tile.
     *
     * Kept out of [people] because the server does not return it as a person; it
     * is a filter over shots, and the card the screen draws for it navigates to
     * the same grid with the reserved `unsorted` id.
     */
    private val _unsortedCount = MutableStateFlow(0)
    val unsortedCount: StateFlow<Int> = _unsortedCount.asStateFlow()

    private val _error = MutableStateFlow<String?>(null)
    val error: StateFlow<String?> = _error.asStateFlow()

    val authExpired: StateFlow<Boolean> = authRepository.authExpired

    init {
        refresh()
    }

    fun refresh() {
        viewModelScope.launch {
            _isRefreshing.value = true
            _error.value = null
            try {
                // People with nothing left in them are dropped rather than shown
                // as empty tiles: the server keeps a person around after their
                // last shot is moved or deleted, and a grid of names that open
                // onto nothing is worse than a shorter grid.
                _people.value = peopleRepository.fetchPeople().filter { it.shotCount > 0 }
            } catch (e: Exception) {
                _people.value = emptyList()
                _error.value = "Failed to refresh: ${e.message}"
            }
            // A stats call that fails does not cost the user the people list, so
            // it gets its own try: the worst case is a missing Unsorted card.
            try {
                _unsortedCount.value = peopleRepository.fetchUnsortedCount()
            } catch (e: Exception) {
                _unsortedCount.value = 0
            }
            _isRefreshing.value = false
        }
    }

    fun reLogin() {
        authRepository.clearToken()
    }

    fun buildCoverUrl(person: Person): String? {
        val thumbnailUrl = person.coverShotThumbnailUrl ?: person.thumbnailUrl ?: return null
        val baseUrl = authRepository.getServerUrl()?.trimEnd('/') ?: return null
        return if (thumbnailUrl.startsWith("/")) "$baseUrl$thumbnailUrl" else thumbnailUrl
    }
}
