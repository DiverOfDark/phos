package dev.phos.android.data.repository

import dev.phos.android.data.remote.PhosApi
import dev.phos.android.domain.model.Person
import javax.inject.Inject
import javax.inject.Singleton

@Singleton
class PeopleRepository @Inject constructor(
    private val api: PhosApi,
) {
    suspend fun fetchPeople(): List<Person> {
        return api.getPeople().map { brief ->
            Person(
                id = brief.id,
                name = brief.name,
                faceCount = brief.faceCount?.toInt() ?: 0,
                thumbnailUrl = brief.thumbnailUrl,
                shotCount = brief.shotCount?.toInt() ?: 0,
                pendingCount = brief.pendingCount?.toInt() ?: 0,
                updatedAt = brief.updatedAt,
                coverShotThumbnailUrl = brief.coverShotThumbnailUrl,
            )
        }
    }
}
