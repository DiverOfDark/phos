package dev.phos.android.data.repository

import android.content.SharedPreferences
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
    }

    fun getToken(): String? {
        val token = prefs.getString(KEY_TOKEN, null) ?: return null
        val expiresAt = prefs.getLong(KEY_EXPIRES_AT, 0)
        if (expiresAt > 0 && System.currentTimeMillis() > expiresAt) {
            // Token expired, but don't clear — let AuthInterceptor handle 401
            // so offline browsing still works
            return token
        }
        return token
    }

    fun saveToken(token: String, expiresInSeconds: Long) {
        prefs.edit()
            .putString(KEY_TOKEN, token)
            .putLong(KEY_EXPIRES_AT, System.currentTimeMillis() + expiresInSeconds * 1000)
            .apply()
    }

    fun clearToken() {
        prefs.edit()
            .remove(KEY_TOKEN)
            .remove(KEY_EXPIRES_AT)
            .apply()
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

    fun logout() {
        prefs.edit().clear().apply()
    }
}
