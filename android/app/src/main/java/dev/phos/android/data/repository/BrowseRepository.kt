package dev.phos.android.data.repository

import dev.phos.android.data.local.dao.FileDao
import dev.phos.android.data.local.dao.ShotDao
import dev.phos.android.data.local.dao.SyncStateDao
import dev.phos.android.data.local.entity.FileEntity
import dev.phos.android.data.local.entity.ShotEntity
import dev.phos.android.data.local.entity.ViewPositionEntity
import dev.phos.android.data.remote.model.PersonBrowseResponse
import dev.phos.android.data.remote.PhosApi
import kotlinx.coroutines.flow.first
import javax.inject.Inject
import javax.inject.Singleton

data class BrowseData(
    val personName: String?,
    val shots: List<ShotWithFiles>,
)

data class ShotWithFiles(
    val shot: ShotEntity,
    val files: List<FileEntity>,
)

@Singleton
class BrowseRepository @Inject constructor(
    private val shotDao: ShotDao,
    private val fileDao: FileDao,
    private val syncStateDao: SyncStateDao,
    private val api: PhosApi,
    private val authRepository: AuthRepository,
) {
    suspend fun fetchBrowseData(personId: String): BrowseData {
        return try {
            val response = api.getPersonBrowse(personId)
            upsertBrowseData(personId, response)
            toBrowseData(personId, response)
        } catch (e: Exception) {
            loadLocalBrowseData(personId)
        }
    }

    private suspend fun upsertBrowseData(personId: String, response: PersonBrowseResponse) {
        val shots = response.shots.map { shot ->
            ShotEntity(
                id = shot.id,
                timestamp = shot.timestamp,
                primaryPersonId = personId,
                reviewStatus = shot.reviewStatus,
                updatedAt = null,
            )
        }
        shotDao.upsertAll(shots)

        val files = response.shots.flatMap { shot ->
            shot.files.map { file ->
                FileEntity(
                    id = file.id,
                    shotId = shot.id,
                    mimeType = file.mimeType,
                    isOriginal = file.isOriginal ?: false,
                    fileSize = file.fileSize,
                    width = null,
                    height = null,
                    durationMs = null,
                    thumbnailUrl = file.thumbnailUrl,
                    updatedAt = null,
                )
            }
        }
        fileDao.upsertAll(files)
    }

    private fun toBrowseData(personId: String, response: PersonBrowseResponse): BrowseData {
        return BrowseData(
            personName = response.person.name,
            shots = response.shots.map { shot ->
                ShotWithFiles(
                    shot = ShotEntity(
                        id = shot.id,
                        timestamp = shot.timestamp,
                        primaryPersonId = personId,
                        reviewStatus = shot.reviewStatus,
                        updatedAt = null,
                    ),
                    files = shot.files.map { file ->
                        FileEntity(
                            id = file.id,
                            shotId = shot.id,
                            mimeType = file.mimeType,
                            isOriginal = file.isOriginal ?: false,
                            fileSize = file.fileSize,
                            width = null,
                            height = null,
                            durationMs = null,
                            thumbnailUrl = file.thumbnailUrl,
                            updatedAt = null,
                        )
                    },
                )
            },
        )
    }

    private suspend fun loadLocalBrowseData(personId: String): BrowseData {
        val shots = mutableListOf<ShotWithFiles>()
        val collectedShots = shotDao.getShotsByPersonId(personId).first()
        for (shot in collectedShots) {
            val files = fileDao.getFilesByShotIdOnce(shot.id)
            shots.add(ShotWithFiles(shot = shot, files = files))
        }
        return BrowseData(personName = null, shots = shots)
    }

    suspend fun getViewPosition(personId: String): ViewPositionEntity? {
        return syncStateDao.getViewPosition(personId)
    }

    suspend fun saveViewPosition(personId: String, shotIndex: Int, fileIndex: Int) {
        syncStateDao.upsertViewPosition(
            ViewPositionEntity(personId = personId, shotIndex = shotIndex, fileIndex = fileIndex)
        )
    }

    fun buildThumbnailUrl(fileId: String, width: Int = 1080): String {
        val baseUrl = authRepository.getServerUrl()?.trimEnd('/') ?: ""
        return "$baseUrl/api/files/$fileId/thumbnail?w=$width"
    }

    fun buildOriginalUrl(fileId: String): String {
        val baseUrl = authRepository.getServerUrl()?.trimEnd('/') ?: ""
        return "$baseUrl/api/files/$fileId"
    }
}
