package dev.phos.android.data.repository

import dev.phos.android.data.local.ViewPosition
import dev.phos.android.data.local.ViewPositionStore
import dev.phos.android.data.remote.PhosApi
import dev.phos.android.data.remote.model.PersonBrowseResponse
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.domain.model.Shot
import javax.inject.Inject
import javax.inject.Singleton

data class BrowseData(
    val personName: String?,
    val shots: List<ShotWithFiles>,
)

data class ShotWithFiles(
    val shot: Shot,
    val files: List<MediaFile>,
)

@Singleton
class BrowseRepository @Inject constructor(
    private val api: PhosApi,
    private val authRepository: AuthRepository,
    private val viewPositionStore: ViewPositionStore,
) {
    suspend fun fetchBrowseData(personId: String): BrowseData {
        val response = api.getPersonBrowse(personId)
        return toBrowseData(personId, response)
    }

    private fun toBrowseData(personId: String, response: PersonBrowseResponse): BrowseData {
        return BrowseData(
            personName = response.person.name,
            shots = response.shots.map { shot ->
                ShotWithFiles(
                    shot = Shot(
                        id = shot.id,
                        timestamp = shot.timestamp,
                        primaryPersonId = personId,
                        reviewStatus = shot.reviewStatus,
                    ),
                    files = shot.files.map { file ->
                        MediaFile(
                            id = file.id,
                            shotId = shot.id,
                            mimeType = file.mimeType,
                            isOriginal = file.isOriginal ?: false,
                            fileSize = file.fileSize,
                            thumbnailUrl = file.thumbnailUrl,
                        )
                    },
                )
            },
        )
    }

    fun getViewPosition(personId: String): ViewPosition? {
        return viewPositionStore.getViewPosition(personId)
    }

    fun saveViewPosition(personId: String, shotIndex: Int, fileIndex: Int) {
        viewPositionStore.saveViewPosition(personId, shotIndex, fileIndex)
    }

    suspend fun deleteFile(fileId: String) {
        api.deleteFile(fileId)
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
