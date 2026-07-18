package dev.phos.android.data.repository

import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import javax.inject.Inject
import javax.inject.Named
import javax.inject.Singleton

@Singleton
class AuthRepository @Inject constructor(
    @Named("auth") private val prefs: SharedPreferences,
) {
    companion object {
        private const val KEY_TOKEN = "phos_jwt"
        private const val KEY_EXPIRES_AT = "expires_at"
        private const val KEY_TTL_SECONDS = "ttl_seconds"
        private const val KEY_SERVER_URL = "server_url"
        private const val KEY_OIDC_ISSUER = "oidc_issuer"
        private const val KEY_OIDC_CLIENT_ID = "oidc_client_id"
        private const val KEY_OIDC_SCOPES = "oidc_scopes"
        private const val KEY_APPAUTH_STATE = "appauth_state"

        // Pre-upgrade installs saved no TTL; assume the old server default (1h)
        private const val DEFAULT_TTL_SECONDS = 3600L
    }

    private val _authExpired = MutableStateFlow(false)
    val authExpired: StateFlow<Boolean> = _authExpired.asStateFlow()

    fun getToken(): String? {
        val token = prefs.getString(KEY_TOKEN, null) ?: return null
        val expiresAt = prefs.getLong(KEY_EXPIRES_AT, 0)
        if (expiresAt > 0 && System.currentTimeMillis() > expiresAt) {
            _authExpired.value = true
            // Return expired token so offline browsing still works
            return token
        }
        return token
    }

    fun isTokenExpired(): Boolean {
        val expiresAt = prefs.getLong(KEY_EXPIRES_AT, 0)
        return expiresAt > 0 && System.currentTimeMillis() > expiresAt
    }

    fun markTokenExpired() {
        _authExpired.value = true
    }

    fun clearAuthExpired() {
        _authExpired.value = false
    }

    fun getExpiresAt(): Long = prefs.getLong(KEY_EXPIRES_AT, 0)

    fun getTtlSeconds(): Long = prefs.getLong(KEY_TTL_SECONDS, DEFAULT_TTL_SECONDS)

    fun saveToken(token: String, expiresInSeconds: Long) {
        prefs.edit()
            .putString(KEY_TOKEN, token)
            .putLong(KEY_EXPIRES_AT, System.currentTimeMillis() + expiresInSeconds * 1000)
            .putLong(KEY_TTL_SECONDS, expiresInSeconds)
            .apply()
        _authExpired.value = false
    }

    // Clears only the Phos session JWT; the AppAuth state (IdP refresh token) is
    // kept so the session can still be renewed silently. Only logout() drops it.
    fun clearToken() {
        prefs.edit()
            .remove(KEY_TOKEN)
            .remove(KEY_EXPIRES_AT)
            .remove(KEY_TTL_SECONDS)
            .apply()
        _authExpired.value = false
    }

    fun saveAppAuthState(json: String) {
        prefs.edit().putString(KEY_APPAUTH_STATE, json).apply()
    }

    fun getAppAuthState(): String? = prefs.getString(KEY_APPAUTH_STATE, null)

    fun getServerUrl(): String? = prefs.getString(KEY_SERVER_URL, null)

    fun saveServerConfig(serverUrl: String, issuer: String?, clientId: String?) {
        prefs.edit()
            .putString(KEY_SERVER_URL, serverUrl)
            .putString(KEY_OIDC_ISSUER, issuer)
            .putString(KEY_OIDC_CLIENT_ID, clientId)
            .apply()
    }

    fun getOidcIssuer(): String? = prefs.getString(KEY_OIDC_ISSUER, null)
    fun getOidcClientId(): String? = prefs.getString(KEY_OIDC_CLIENT_ID, null)

    fun saveOidcScopes(scopes: List<String>?) {
        if (scopes.isNullOrEmpty()) return
        prefs.edit().putString(KEY_OIDC_SCOPES, scopes.joinToString(" ")).apply()
    }

    fun getOidcScopes(): List<String>? =
        prefs.getString(KEY_OIDC_SCOPES, null)?.split(" ")?.filter { it.isNotBlank() }

    fun isLoggedIn(): Boolean = getToken() != null && getServerUrl() != null

    fun logout() {
        prefs.edit().clear().apply()
        _authExpired.value = false
    }
}
