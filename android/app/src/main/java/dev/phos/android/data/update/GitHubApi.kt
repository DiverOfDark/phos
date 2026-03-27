package dev.phos.android.data.update

import retrofit2.http.GET

interface GitHubApi {
    @GET("repos/DiverOfDark/phos/releases/latest")
    suspend fun getLatestRelease(): GitHubRelease
}
