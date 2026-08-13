package dev.phos.android.di

import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import dev.phos.android.data.remote.api.AuthApi
import dev.phos.android.data.remote.api.ClientApi
import dev.phos.android.data.remote.api.FilesApi
import dev.phos.android.data.remote.api.PeopleApi
import dev.phos.android.data.remote.api.ShotsApi
import retrofit2.Retrofit
import javax.inject.Singleton

/**
 * The API surface, generated from `android/openapi.json` by the OpenAPI generator.
 *
 * Nothing here is hand-written on purpose: a hand-maintained Retrofit method that
 * drifts from the backend fails at runtime, on a device, with a 404 — where a
 * generated one fails at compile time the moment the spec is regenerated. The
 * interfaces are split by spec tag, which is why there are five of them.
 */
@Module
@InstallIn(SingletonComponent::class)
object ApiModule {

    @Provides
    @Singleton
    fun provideAuthApi(retrofit: Retrofit): AuthApi = retrofit.create(AuthApi::class.java)

    @Provides
    @Singleton
    fun providePeopleApi(retrofit: Retrofit): PeopleApi = retrofit.create(PeopleApi::class.java)

    @Provides
    @Singleton
    fun provideShotsApi(retrofit: Retrofit): ShotsApi = retrofit.create(ShotsApi::class.java)

    @Provides
    @Singleton
    fun provideFilesApi(retrofit: Retrofit): FilesApi = retrofit.create(FilesApi::class.java)

    @Provides
    @Singleton
    fun provideClientApi(retrofit: Retrofit): ClientApi = retrofit.create(ClientApi::class.java)
}
