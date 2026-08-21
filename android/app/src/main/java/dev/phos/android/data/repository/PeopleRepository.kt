package dev.phos.android.data.repository

import dev.phos.android.data.remote.api.PeopleApi
import dev.phos.android.data.remote.api.SystemApi
import dev.phos.android.data.remote.await
import dev.phos.android.domain.model.Person
import javax.inject.Inject
import javax.inject.Singleton

@Singleton
class PeopleRepository @Inject constructor(
    private val api: PeopleApi,
    private val systemApi: SystemApi,
) {
    suspend fun fetchPeople(): List<Person> {
        return api.getPeople().await().map { brief ->
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

    /**
     * How many shots belong to nobody yet.
     *
     * Read from the organize stats rather than by listing the unsorted shots:
     * the people screen only needs the number, and the pile can be the whole
     * library on a fresh scan.
     */
    suspend fun fetchUnsortedCount(): Int =
        systemApi.getOrganizeStats().await().unsorted?.toInt() ?: 0
}
