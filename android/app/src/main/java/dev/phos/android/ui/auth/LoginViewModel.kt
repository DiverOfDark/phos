package dev.phos.android.ui.auth

import android.app.Activity
import android.content.Intent
import android.net.Uri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.fasterxml.jackson.databind.DeserializationFeature
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper
import com.fasterxml.jackson.module.kotlin.readValue
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.phos.android.data.remote.PhosApi
import dev.phos.android.data.remote.model.TokenExchangeRequest
import dev.phos.android.data.repository.AuthRepository
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import net.openid.appauth.AuthorizationException
import net.openid.appauth.AuthorizationRequest
import net.openid.appauth.AuthorizationResponse
import net.openid.appauth.AuthorizationService
import net.openid.appauth.AuthorizationServiceConfiguration
import net.openid.appauth.ResponseTypeValues
import okhttp3.OkHttpClient
import okhttp3.Request
import javax.inject.Inject

data class LoginUiState(
    val serverUrl: String = "",
    val oidcIssuer: String = "",
    val oidcClientId: String = "",
    val isLoading: Boolean = false,
    val isFetchingConfig: Boolean = false,
    val error: String? = null,
    val isLoggedIn: Boolean = false,
)

private data class AuthConfigDto(
    val issuer: String? = null,
    val client_id: String? = null,
    val scopes: List<String>? = null,
)

@HiltViewModel
class LoginViewModel @Inject constructor(
    private val authRepository: AuthRepository,
    private val okHttpClient: OkHttpClient,
) : ViewModel() {

    private val _uiState = MutableStateFlow(LoginUiState())
    val uiState: StateFlow<LoginUiState> = _uiState.asStateFlow()

    private val mapper = jacksonObjectMapper().apply {
        configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false)
    }

    init {
        // Restore saved values
        val serverUrl = authRepository.getServerUrl() ?: ""
        val issuer = authRepository.getOidcIssuer() ?: ""
        val clientId = authRepository.getOidcClientId() ?: ""
        val isLoggedIn = authRepository.isLoggedIn()
        _uiState.value = LoginUiState(
            serverUrl = serverUrl,
            oidcIssuer = issuer,
            oidcClientId = clientId,
            isLoggedIn = isLoggedIn,
        )
    }

    fun updateServerUrl(url: String) {
        _uiState.value = _uiState.value.copy(serverUrl = url)
    }

    fun updateOidcIssuer(issuer: String) {
        _uiState.value = _uiState.value.copy(oidcIssuer = issuer)
    }

    fun updateOidcClientId(clientId: String) {
        _uiState.value = _uiState.value.copy(oidcClientId = clientId)
    }

    fun fetchAuthConfig() {
        val serverUrl = _uiState.value.serverUrl.trimEnd('/')
        if (serverUrl.isBlank()) return

        _uiState.value = _uiState.value.copy(isFetchingConfig = true)
        viewModelScope.launch {
            try {
                val config = withContext(Dispatchers.IO) {
                    val request = Request.Builder()
                        .url("$serverUrl/api/auth/config")
                        .get()
                        .build()
                    val response = okHttpClient.newCall(request).execute()
                    if (response.isSuccessful) {
                        response.body?.string()?.let { body ->
                            mapper.readValue<AuthConfigDto>(body)
                        }
                    } else null
                }
                if (config != null) {
                    _uiState.value = _uiState.value.copy(
                        oidcIssuer = config.issuer ?: "",
                        oidcClientId = config.client_id ?: "",
                        isFetchingConfig = false,
                    )
                } else {
                    _uiState.value = _uiState.value.copy(isFetchingConfig = false)
                }
            } catch (_: Exception) {
                // Server may not have auth configured (single-user mode) — that's fine
                _uiState.value = _uiState.value.copy(isFetchingConfig = false)
            }
        }
    }

    fun startLogin(activity: Activity) {
        val state = _uiState.value
        if (state.serverUrl.isBlank()) {
            _uiState.value = state.copy(error = "Server URL is required")
            return
        }

        _uiState.value = state.copy(isLoading = true, error = null)

        // Save server config
        authRepository.saveServerConfig(state.serverUrl, state.oidcIssuer, state.oidcClientId)

        if (state.oidcIssuer.isBlank()) {
            // No OIDC — try connecting directly (single-user mode)
            viewModelScope.launch {
                try {
                    // In single-user mode, no auth needed. Just verify the server is reachable.
                    _uiState.value = _uiState.value.copy(isLoading = false, isLoggedIn = true)
                } catch (e: Exception) {
                    _uiState.value = _uiState.value.copy(
                        isLoading = false,
                        error = "Failed to connect: ${e.message}",
                    )
                }
            }
            return
        }

        // OIDC flow
        viewModelScope.launch {
            try {
                val issuerUri = Uri.parse(state.oidcIssuer)
                AuthorizationServiceConfiguration.fetchFromIssuer(issuerUri) { config, ex ->
                    if (config == null || ex != null) {
                        _uiState.value = _uiState.value.copy(
                            isLoading = false,
                            error = "OIDC discovery failed: ${ex?.message}",
                        )
                        return@fetchFromIssuer
                    }

                    val authRequest = AuthorizationRequest.Builder(
                        config,
                        state.oidcClientId,
                        ResponseTypeValues.CODE,
                        Uri.parse("dev.phos.android://callback"),
                    )
                        .setScopes("openid", "profile", "email")
                        .build()

                    val authService = AuthorizationService(activity)
                    val authIntent = authService.getAuthorizationRequestIntent(authRequest)
                    activity.startActivityForResult(authIntent, RC_AUTH)
                }
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "Login failed: ${e.message}",
                )
            }
        }
    }

    fun handleAuthResult(data: Intent?, api: PhosApi) {
        val response = data?.let { AuthorizationResponse.fromIntent(it) }
        val exception = data?.let { AuthorizationException.fromIntent(it) }

        if (exception != null) {
            _uiState.value = _uiState.value.copy(
                isLoading = false,
                error = "Auth failed: ${exception.message}",
            )
            return
        }

        if (response == null) {
            _uiState.value = _uiState.value.copy(
                isLoading = false,
                error = "No auth response received",
            )
            return
        }

        viewModelScope.launch {
            try {
                // Exchange the ID token with the Phos backend
                val idToken = response.idToken
                if (idToken != null) {
                    val request = TokenExchangeRequest().apply { this.idToken = idToken }
                    val tokenResponse = api.exchangeToken(request)
                    authRepository.saveToken(tokenResponse.token, tokenResponse.expiresIn)
                    _uiState.value = _uiState.value.copy(isLoading = false, isLoggedIn = true)
                } else {
                    _uiState.value = _uiState.value.copy(
                        isLoading = false,
                        error = "No ID token in auth response",
                    )
                }
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "Token exchange failed: ${e.message}",
                )
            }
        }
    }

    companion object {
        const val RC_AUTH = 1001
    }
}
