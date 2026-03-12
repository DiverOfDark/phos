package dev.phos.android.data.repository

import dev.phos.android.data.local.dao.FileDao
import dev.phos.android.data.local.dao.PersonDao
import dev.phos.android.data.local.dao.ShotDao
import dev.phos.android.data.local.dao.SyncStateDao
import dev.phos.android.data.local.entity.FileEntity
import dev.phos.android.data.local.entity.PersonEntity
import dev.phos.android.data.local.entity.ShotEntity
import dev.phos.android.data.local.entity.SyncStateEntity
import dev.phos.android.data.remote.PhosApi
import dev.phos.android.data.remote.model.SyncResponse
import javax.inject.Inject
import javax.inject.Singleton

@Singleton
class SyncRepository @Inject constructor(
    private val personDao: PersonDao,
    private val shotDao: ShotDao,
    private val fileDao: FileDao,
    private val syncStateDao: SyncStateDao,
    private val api: PhosApi,
) {
    suspend fun performSync(): Result<Unit> = runCatching {
        val syncState = syncStateDao.getSyncState()
        val since = syncState?.syncToken

        val response = api.getSync(since)
        applySync(response)

        syncStateDao.upsertSyncState(
            SyncStateEntity(
                syncToken = response.syncToken,
                lastSyncTime = System.currentTimeMillis(),
            )
        )
    }

    private suspend fun applySync(response: SyncResponse) {
        // Process people
        val (deletedPeople, upsertPeople) = response.people.partition { it.deleted ?: false }
        for (p in deletedPeople) {
            personDao.deleteById(p.id)
        }
        if (upsertPeople.isNotEmpty()) {
            personDao.upsertAll(upsertPeople.map { p ->
                PersonEntity(
                    id = p.id,
                    name = p.name,
                    faceCount = 0,
                    thumbnailUrl = p.thumbnailUrl,
                    shotCount = p.shotCount?.toInt() ?: 0,
                    pendingCount = 0,
                    updatedAt = p.updatedAt,
                    coverShotThumbnailUrl = null,
                )
            })
        }

        // Process shots
        val (deletedShots, upsertShots) = response.shots.partition { it.deleted ?: false }
        for (s in deletedShots) {
            shotDao.deleteById(s.id)
        }
        if (upsertShots.isNotEmpty()) {
            shotDao.upsertAll(upsertShots.map { s ->
                ShotEntity(
                    id = s.id,
                    timestamp = s.timestamp,
                    primaryPersonId = s.primaryPersonId,
                    reviewStatus = s.reviewStatus,
                    updatedAt = s.updatedAt,
                )
            })
        }

        // Process files
        val (deletedFiles, upsertFiles) = response.files.partition { it.deleted ?: false }
        for (f in deletedFiles) {
            fileDao.deleteById(f.id)
        }
        if (upsertFiles.isNotEmpty()) {
            fileDao.upsertAll(upsertFiles.map { f ->
                FileEntity(
                    id = f.id,
                    shotId = f.shotId,
                    mimeType = f.mimeType,
                    isOriginal = f.isOriginal ?: false,
                    fileSize = f.fileSize,
                    width = null,
                    height = null,
                    durationMs = null,
                    thumbnailUrl = null,
                    updatedAt = f.updatedAt,
                )
            })
        }
    }
}
