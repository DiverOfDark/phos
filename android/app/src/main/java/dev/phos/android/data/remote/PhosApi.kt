package dev.phos.android.data.remote

import dev.phos.android.data.remote.model.PersonBrief
import dev.phos.android.data.remote.model.PersonBrowseResponse
import dev.phos.android.data.remote.model.SyncResponse
import dev.phos.android.data.remote.model.TokenExchangeRequest
import retrofit2.http.Body
import retrofit2.http.DELETE
import retrofit2.http.GET
import retrofit2.http.POST
import retrofit2.http.Path
import retrofit2.http.Query

/**
 * Retrofit interface using generated OpenAPI models with Kotlin coroutines.
 */
interface PhosApi {
    @GET("api/people")
    suspend fun getPeople(): List<PersonBrief>

    @GET("api/people/{id}/browse")
    suspend fun getPersonBrowse(@Path("id") id: String): PersonBrowseResponse

    @GET("api/sync")
    suspend fun getSync(@Query("since") since: String? = null): SyncResponse

    @GET("api/files/{id}/thumbnail")
    suspend fun getFileThumbnail(
        @Path("id") id: String,
        @Query("w") width: Int? = null,
    ): okhttp3.ResponseBody

    @DELETE("api/files/{id}")
    suspend fun deleteFile(@Path("id") id: String): okhttp3.ResponseBody

    @POST("api/auth/token")
    suspend fun exchangeToken(@Body request: TokenExchangeRequest): TokenExchangeResponse

    @GET("api/auth/config")
    suspend fun getAuthConfig(): dev.phos.android.data.remote.model.AuthConfigResponse

    @GET("api/version")
    suspend fun getVersion(): VersionResponse
}

// These two aren't in the OpenAPI spec (token response is inline JSON, version is simple)
data class TokenExchangeResponse(
    val token: String = "",
    @com.fasterxml.jackson.annotation.JsonProperty("expires_in")
    val expiresIn: Long = 0,
)

data class VersionResponse(
    val version: String = "",
)
