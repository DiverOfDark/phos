package dev.phos.android.di

import android.content.Context
import androidx.room.Room
import com.fasterxml.jackson.databind.DeserializationFeature
import com.fasterxml.jackson.databind.ObjectMapper
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.phos.android.data.local.PhosDatabase
import dev.phos.android.data.local.dao.FileDao
import dev.phos.android.data.local.dao.PersonDao
import dev.phos.android.data.local.dao.ShotDao
import dev.phos.android.data.local.dao.SyncStateDao
import dev.phos.android.data.remote.AuthInterceptor
import dev.phos.android.data.remote.BaseUrlInterceptor
import dev.phos.android.data.remote.PhosApi
import okhttp3.OkHttpClient
import okhttp3.logging.HttpLoggingInterceptor
import retrofit2.Retrofit
import retrofit2.converter.jackson.JacksonConverterFactory
import java.util.concurrent.TimeUnit
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
object AppModule {

    @Provides
    @Singleton
    fun provideObjectMapper(): ObjectMapper = jacksonObjectMapper().apply {
        configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false)
    }

    @Provides
    @Singleton
    fun provideOkHttpClient(
        baseUrlInterceptor: BaseUrlInterceptor,
        authInterceptor: AuthInterceptor,
    ): OkHttpClient {
        return OkHttpClient.Builder()
            .addInterceptor(baseUrlInterceptor)
            .addInterceptor(authInterceptor)
            .addInterceptor(HttpLoggingInterceptor().apply {
                level = HttpLoggingInterceptor.Level.BASIC
            })
            .connectTimeout(30, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .build()
    }

    @Provides
    @Singleton
    fun provideRetrofit(
        okHttpClient: OkHttpClient,
        objectMapper: ObjectMapper,
    ): Retrofit {
        return Retrofit.Builder()
            .baseUrl(BaseUrlInterceptor.PLACEHOLDER_BASE_URL)
            .client(okHttpClient)
            .addConverterFactory(JacksonConverterFactory.create(objectMapper))
            .build()
    }

    @Provides
    @Singleton
    fun providePhosApi(retrofit: Retrofit): PhosApi {
        return retrofit.create(PhosApi::class.java)
    }

    @Provides
    @Singleton
    fun provideDatabase(@ApplicationContext context: Context): PhosDatabase {
        return Room.databaseBuilder(
            context,
            PhosDatabase::class.java,
            "phos.db",
        )
            .fallbackToDestructiveMigration()
            .build()
    }

    @Provides fun providePersonDao(db: PhosDatabase): PersonDao = db.personDao()
    @Provides fun provideShotDao(db: PhosDatabase): ShotDao = db.shotDao()
    @Provides fun provideFileDao(db: PhosDatabase): FileDao = db.fileDao()
    @Provides fun provideSyncStateDao(db: PhosDatabase): SyncStateDao = db.syncStateDao()
}
