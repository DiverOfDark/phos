package dev.phos.android.ui.auth

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.qualifiers.ApplicationContext
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
    val info: String? = null,
    val isLoggedIn: Boolean = false,
)

private data class AuthConfigDto(
    val issuer: String? = null,
    val client_id: String? = null,
    val mobile_client_id: String? = null,
    val scopes: List<String>? = null,
)

@HiltViewModel
class LoginViewModel @Inject constructor(
    private val authRepository: AuthRepository,
    private val okHttpClient: OkHttpClient,
    private val phosApi: PhosApi,
    @ApplicationContext private val appContext: Context,
) : ViewModel() {

    private val _uiState = MutableStateFlow(LoginUiState())
    val uiState: StateFlow<LoginUiState> = _uiState.asStateFlow()

    private val _authIntent = MutableStateFlow<Intent?>(null)
    val authIntent: StateFlow<Intent?> = _authIntent.asStateFlow()

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
        val raw = _uiState.value.serverUrl.trim().trimEnd('/')
        if (raw.isBlank()) return
        if (!raw.startsWith("http://") && !raw.startsWith("https://")) {
            _uiState.value = _uiState.value.copy(
                error = "Server URL must start with http:// or https://",
                info = null,
            )
            return
        }

        _uiState.value = _uiState.value.copy(isFetchingConfig = true, error = null, info = null)
        viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    val request = Request.Builder()
                        .url("$raw/api/auth/config")
                        .get()
                        .build()
                    okHttpClient.newCall(request).execute().use { response ->
                        FetchResult(
                            code = response.code,
                            body = response.body?.string(),
                        )
                    }
                }
            }

            result.fold(
                onSuccess = { res ->
                    when {
                        res.code == 404 -> _uiState.value = _uiState.value.copy(
                            isFetchingConfig = false,
                            info = "Server has no OIDC configured. Leave issuer blank and press Connect.",
                        )
                        res.code !in 200..299 -> _uiState.value = _uiState.value.copy(
                            isFetchingConfig = false,
                            error = "Server returned HTTP ${res.code}",
                        )
                        else -> {
                            val config = res.body?.let {
                                runCatching { mapper.readValue<AuthConfigDto>(it) }.getOrNull()
                            }
                            if (config?.issuer.isNullOrBlank()) {
                                _uiState.value = _uiState.value.copy(
                                    isFetchingConfig = false,
                                    error = "Unexpected response from server",
                                )
                            } else {
                                _uiState.value = _uiState.value.copy(
                                    oidcIssuer = config.issuer,
                                    oidcClientId = config.mobile_client_id ?: config.client_id ?: "",
                                    isFetchingConfig = false,
                                    info = "Auth config loaded.",
                                )
                            }
                        }
                    }
                },
                onFailure = { e ->
                    _uiState.value = _uiState.value.copy(
                        isFetchingConfig = false,
                        error = "Couldn't reach server: ${e.message ?: e.javaClass.simpleName}",
                    )
                },
            )
        }
    }

    private data class FetchResult(val code: Int, val body: String?)

    fun startLogin(context: Context) {
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

                    val authService = AuthorizationService(context)
                    _authIntent.value = authService.getAuthorizationRequestIntent(authRequest)
                }
            } catch (e: Exception) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "Login failed: ${e.message}",
                )
            }
        }
    }

    fun clearAuthIntent() {
        _authIntent.value = null
    }

    fun handleAuthResult(data: Intent?) {
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

        // Exchange authorization code for tokens via AppAuth
        val authService = AuthorizationService(appContext)
        authService.performTokenRequest(response.createTokenExchangeRequest()) { tokenResponse, tokenEx ->
            if (tokenEx != null || tokenResponse == null) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "Token exchange failed: ${tokenEx?.message}",
                )
                return@performTokenRequest
            }

            val idToken = tokenResponse.idToken
            if (idToken == null) {
                _uiState.value = _uiState.value.copy(
                    isLoading = false,
                    error = "No ID token in token response",
                )
                return@performTokenRequest
            }

            // Exchange the ID token with the Phos backend
            viewModelScope.launch {
                try {
                    val request = TokenExchangeRequest().apply { this.idToken = idToken }
                    val backendResponse = phosApi.exchangeToken(request)
                    authRepository.saveToken(backendResponse.token, backendResponse.expiresIn)
                    _uiState.value = _uiState.value.copy(isLoading = false, isLoggedIn = true)
                } catch (e: Exception) {
                    _uiState.value = _uiState.value.copy(
                        isLoading = false,
                        error = "Backend token exchange failed: ${e.message}",
                    )
                }
            }
        }
    }
}
