package dev.phos.android.data.repository

import dev.phos.android.data.local.dao.PersonDao
import dev.phos.android.data.local.entity.PersonEntity
import dev.phos.android.data.remote.PhosApi
import kotlinx.coroutines.flow.Flow
import javax.inject.Inject
import javax.inject.Singleton

@Singleton
class PeopleRepository @Inject constructor(
    private val personDao: PersonDao,
    private val api: PhosApi,
) {
    fun observePeople(): Flow<List<PersonEntity>> = personDao.getAll()

    suspend fun refreshPeople() {
        val people = api.getPeople()
        personDao.upsertAll(people.map { brief ->
            PersonEntity(
                id = brief.id,
                name = brief.name,
                faceCount = brief.faceCount?.toInt() ?: 0,
                thumbnailUrl = brief.thumbnailUrl,
                shotCount = brief.shotCount?.toInt() ?: 0,
                pendingCount = brief.pendingCount?.toInt() ?: 0,
                updatedAt = brief.updatedAt,
                coverShotThumbnailUrl = brief.coverShotThumbnailUrl,
            )
        })
    }
}
