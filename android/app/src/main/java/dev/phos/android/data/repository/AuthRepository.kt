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
        private const val KEY_SERVER_URL = "server_url"
        private const val KEY_OIDC_ISSUER = "oidc_issuer"
        private const val KEY_OIDC_CLIENT_ID = "oidc_client_id"
        private const val KEY_WIFI_ONLY_SYNC = "wifi_only_sync"
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

    fun saveToken(token: String, expiresInSeconds: Long) {
        prefs.edit()
            .putString(KEY_TOKEN, token)
            .putLong(KEY_EXPIRES_AT, System.currentTimeMillis() + expiresInSeconds * 1000)
            .apply()
        _authExpired.value = false
    }

    fun clearToken() {
        prefs.edit()
            .remove(KEY_TOKEN)
            .remove(KEY_EXPIRES_AT)
            .apply()
        _authExpired.value = false
    }

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

    fun isLoggedIn(): Boolean = getToken() != null && getServerUrl() != null

    fun isWifiOnlySync(): Boolean = prefs.getBoolean(KEY_WIFI_ONLY_SYNC, false)

    fun setWifiOnlySync(enabled: Boolean) {
        prefs.edit().putBoolean(KEY_WIFI_ONLY_SYNC, enabled).apply()
    }

    fun logout() {
        prefs.edit().clear().apply()
        _authExpired.value = false
    }
}
