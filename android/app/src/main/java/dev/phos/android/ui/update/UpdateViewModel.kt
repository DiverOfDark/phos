package dev.phos.android.ui.update

import android.content.Intent
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.phos.android.data.repository.UpdateRepository
import dev.phos.android.update.InstallState
import dev.phos.android.update.UpdateState
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject

/** Everything the update row needs to draw itself. */
data class UpdateUiState(
    val runningVersionName: String,
    val runningVersionCode: Int,
    val update: UpdateState,
    val install: InstallState,
) {
    /** True while a check or a download is in flight, so the button can be disabled. */
    val busy: Boolean
        get() = update is UpdateState.Checking ||
            install is InstallState.Downloading ||
            install is InstallState.Verifying
}

/**
 * The "Check for updates" row.
 *
 * Reads shared state from [UpdateRepository] rather than checking on its own, so the
 * app-start check has usually already answered by the time this screen opens and the
 * row is populated instantly.
 */
@HiltViewModel
class UpdateViewModel @Inject constructor(
    private val updates: UpdateRepository,
) : ViewModel() {

    val uiState: StateFlow<UpdateUiState> = combine(
        updates.state,
        updates.install,
    ) { update, install ->
        UpdateUiState(
            runningVersionName = updates.runningVersion.versionName,
            runningVersionCode = updates.runningVersion.versionCode,
            update = update,
            install = install,
        )
    }.stateIn(
        scope = viewModelScope,
        started = SharingStarted.WhileSubscribed(5_000),
        initialValue = UpdateUiState(
            runningVersionName = updates.runningVersion.versionName,
            runningVersionCode = updates.runningVersion.versionCode,
            update = updates.state.value,
            install = updates.install.value,
        ),
    )

    init {
        // Cheap when the app-start check already answered — checkQuietly is throttled
        // to once an hour and returns immediately otherwise.
        viewModelScope.launch { updates.checkQuietly() }
    }

    /** The manual check. Reports failure, unlike the app-start one. */
    fun check() {
        viewModelScope.launch { updates.check() }
    }

    /**
     * Accepts the update: download, verify, then Android's own confirmation.
     *
     * Handed to the repository's own scope rather than [viewModelScope] — navigating
     * away mid-download must not cancel a transfer the user asked for.
     */
    fun install(available: UpdateState.Available) = updates.startInstall(available)

    fun dismiss() = updates.dismissInstall()

    /** Where to send the user when "install unknown apps" is off. */
    fun permissionSettingsIntent(): Intent = updates.permissionSettingsIntent()
}
