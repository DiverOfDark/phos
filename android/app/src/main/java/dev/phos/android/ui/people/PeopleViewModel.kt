package dev.phos.android.ui.people

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.phos.android.data.local.entity.PersonEntity
import dev.phos.android.data.repository.AuthRepository
import dev.phos.android.data.repository.PeopleRepository
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject

@HiltViewModel
class PeopleViewModel @Inject constructor(
    private val peopleRepository: PeopleRepository,
    private val authRepository: AuthRepository,
) : ViewModel() {

    val people: StateFlow<List<PersonEntity>> = peopleRepository.observePeople()
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5000), emptyList())

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
                peopleRepository.refreshPeople()
            } catch (e: Exception) {
                _error.value = "Failed to refresh: ${e.message}"
            } finally {
                _isRefreshing.value = false
            }
        }
    }

    fun buildCoverUrl(person: PersonEntity): String? {
        val thumbnailUrl = person.coverShotThumbnailUrl ?: person.thumbnailUrl ?: return null
        val baseUrl = authRepository.getServerUrl()?.trimEnd('/') ?: return null
        return if (thumbnailUrl.startsWith("/")) "$baseUrl$thumbnailUrl" else thumbnailUrl
    }
}
