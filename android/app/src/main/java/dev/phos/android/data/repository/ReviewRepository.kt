package dev.phos.android.data.repository

import dev.phos.android.data.remote.api.FacesApi
import dev.phos.android.data.remote.api.ShotsApi
import dev.phos.android.data.remote.await
import dev.phos.android.data.remote.awaitVoid
import dev.phos.android.data.remote.model.ReassignFacePayload
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.FaceSuggestion
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.domain.model.PendingShot
import dev.phos.android.domain.model.ShotDetail
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The review backlog: shots the clustering has guessed at but nobody has confirmed,
 * and the face-level edits that fix a wrong guess.
 *
 * Separate from [ShotRepository] because it is the only thing that needs faces. The
 * browse endpoint the rest of the app runs on returns none, so reviewing means
 * fetching each shot's detail — which is also the only call that reports the image
 * dimensions the face boxes are drawn against.
 */
@Singleton
class ReviewRepository @Inject constructor(
    private val shotsApi: ShotsApi,
    private val facesApi: FacesApi,
) {

    /**
     * Everything still awaiting review, across every person.
     *
     * Deliberately not scoped to one person: the point of the queue is to clear a
     * backlog, and making the user pick a person first would mean knowing where the
     * mistakes are before finding them.
     */
    suspend fun pendingShots(): List<PendingShot> =
        shotsApi.getShots(null, null, PENDING, null, null).await().map { brief ->
            PendingShot(
                id = brief.id,
                thumbnailUrl = brief.thumbnailUrl,
                personName = brief.primaryPersonName,
                fileCount = brief.fileCount?.toInt() ?: 0,
                timestamp = brief.timestamp,
            )
        }

    /** Full detail for one shot, including faces and the size their boxes refer to. */
    suspend fun shotDetail(shotId: String): ShotDetail {
        val response = shotsApi.getShotDetail(shotId).await()
        return ShotDetail(
            id = response.id,
            width = response.width?.toInt(),
            height = response.height?.toInt(),
            primaryPersonId = response.primaryPersonId,
            primaryPersonName = response.primaryPersonName,
            reviewStatus = response.reviewStatus,
            timestamp = response.timestamp,
            files = response.files.orEmpty().map { file ->
                MediaFile(
                    id = file.id,
                    shotId = response.id,
                    mimeType = file.mimeType,
                    isOriginal = file.isOriginal ?: false,
                    fileSize = file.fileSize,
                    thumbnailUrl = file.thumbnailUrl,
                )
            },
            faces = response.faces.orEmpty().map { face ->
                Face(
                    id = face.id,
                    fileId = face.fileId,
                    personId = face.personId,
                    personName = face.personName,
                    x1 = face.boxX1,
                    y1 = face.boxY1,
                    x2 = face.boxX2,
                    y2 = face.boxY2,
                )
            },
            alsoContains = response.alsoContains.orEmpty().mapNotNull { it.name },
        )
    }

    /**
     * Who this face might be, nearest first.
     *
     * Sorted here rather than trusted from the wire: the whole value of the list is
     * that the first row is the likeliest answer.
     */
    suspend fun faceSuggestions(faceId: String): List<FaceSuggestion> =
        facesApi.getFaceSuggestions(faceId).await()
            .map {
                FaceSuggestion(
                    personId = it.personId,
                    personName = it.personName,
                    distance = it.distance ?: Float.MAX_VALUE,
                    thumbnailUrl = it.thumbnailUrl,
                )
            }
            .sortedBy { it.distance }

    /**
     * Reassigns one face.
     *
     * Face-level, not shot-level: a shot with two people in it is assigned to one of
     * them, and telling the server "this *face* is Anna" is what actually corrects
     * the clustering. The server recomputes the shot's primary person from the faces
     * afterwards, so this can change the shot's owner as a side effect.
     */
    suspend fun reassignFace(faceId: String, personId: String) {
        facesApi.reassignFace(faceId, ReassignFacePayload().personId(personId)).awaitVoid()
    }

    /** Drops a face the detector got wrong — a pattern on a shirt, a face in a poster. */
    suspend fun deleteFace(faceId: String) {
        facesApi.deleteFace(faceId).awaitVoid()
    }

    private companion object {
        const val PENDING = "pending"
    }
}
