package dev.phos.android.data.remote

import dev.phos.android.data.repository.AuthRepository
import okhttp3.Interceptor
import okhttp3.Response
import javax.inject.Inject
import javax.inject.Singleton

@Singleton
class AuthInterceptor @Inject constructor(
    private val authRepository: dagger.Lazy<AuthRepository>,
) : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val original = chain.request()
        val token = authRepository.get().getToken()

        val request = if (token != null) {
            original.newBuilder()
                .header("Authorization", "Bearer $token")
                .build()
        } else {
            original
        }

        val response = chain.proceed(request)

        // Mark token as expired on 401 but don't clear it — keeps offline browsing working
        if (response.code == 401) {
            authRepository.get().markTokenExpired()
        }

        return response
    }
}
