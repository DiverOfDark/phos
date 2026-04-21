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
                _people.value = peopleRepository.fetchPeople()
            } catch (e: Exception) {
                _people.value = emptyList()
                _error.value = "Failed to refresh: ${e.message}"
            } finally {
                _isRefreshing.value = false
            }
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
