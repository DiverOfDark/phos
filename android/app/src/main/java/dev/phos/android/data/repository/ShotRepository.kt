package dev.phos.android.data.repository

import dev.phos.android.data.remote.api.FilesApi
import dev.phos.android.data.remote.api.PeopleApi
import dev.phos.android.data.remote.api.ShotsApi
import dev.phos.android.data.remote.await
import dev.phos.android.data.remote.awaitVoid
import dev.phos.android.data.remote.model.BatchConfirmPayload
import dev.phos.android.data.remote.model.BatchReassignPayload
import dev.phos.android.data.remote.model.CreatePersonPayload
import dev.phos.android.data.remote.model.MergeShotsPayload
import dev.phos.android.data.remote.model.SplitShotPayload
import dev.phos.android.data.remote.model.UpdateShotPayload
import dev.phos.android.domain.model.Person
import dev.phos.android.domain.model.SimilarShot
import javax.inject.Inject
import javax.inject.Singleton

/**
 * What happened to a batch of shots deleted one by one.
 *
 * There is no batch-delete endpoint, so a multi-select delete is N requests and can
 * genuinely half-succeed. Reporting "deleted 3 of 5" beats both a bare "done" and a
 * bare "failed", either of which would leave the user guessing what is still there.
 */
data class DeleteOutcome(val deleted: Int, val failed: Int, val firstError: String?)

/**
 * Everything the app can *change* about a shot.
 *
 * Reading is [BrowseRepository]'s job; this is the write side, and it is kept
 * separate because the two have different failure rules. A failed read falls back
 * to whatever is on screen; a failed write must be told to the user, because they
 * asked for something and it did not happen. Nothing here retries or queues — the
 * calls throw, the ViewModel turns that into a message, and the user decides.
 */
@Singleton
class ShotRepository @Inject constructor(
    private val shotsApi: ShotsApi,
    private val peopleApi: PeopleApi,
    private val filesApi: FilesApi,
) {

    /** Deletes a shot and every file in it. Irreversible, server-side. */
    suspend fun deleteShot(shotId: String) {
        shotsApi.deleteShot(shotId).awaitVoid()
    }

    /** Deletes one file out of a shot. The shot's original cannot be deleted this way. */
    suspend fun deleteFile(fileId: String) {
        filesApi.deleteFile(fileId).awaitVoid()
    }

    /**
     * Reassigns a shot to [personId].
     *
     * `review_status` is left alone deliberately: moving a shot to the right person
     * is not the same as saying "I have reviewed this", and folding the two together
     * would quietly empty the review queue on the web.
     */
    suspend fun moveToPerson(shotId: String, personId: String) {
        shotsApi.updateShot(shotId, UpdateShotPayload().primaryPersonId(personId)).awaitVoid()
    }

    /**
     * Splits [fileIds] out of [shotId] into a new shot of their own.
     *
     * The server refuses to split out every file (that would leave an empty shot),
     * so the caller must keep at least one behind.
     */
    suspend fun split(shotId: String, fileIds: List<String>) {
        shotsApi.splitShot(shotId, SplitShotPayload().fileIds(fileIds)).awaitVoid()
    }

    /**
     * Merge candidates for [shotId], nearest first.
     *
     * The API groups them by person; the groups are flattened and re-sorted by
     * distance, so the most likely duplicate is the first thing the user sees
     * regardless of who it currently belongs to.
     */
    suspend fun similarShots(shotId: String): List<SimilarShot> =
        shotsApi.getSimilarShots(shotId).await()
            .flatMap { group ->
                group.shots.orEmpty().map { item ->
                    SimilarShot(
                        id = item.id,
                        thumbnailUrl = item.thumbnailUrl,
                        fileCount = item.fileCount.toInt(),
                        distance = item.distance,
                        // The group's person, not the item's: the item's own
                        // `primary_person_name` is only populated for some rows.
                        personName = group.personName ?: item.primaryPersonName,
                        reviewStatus = item.reviewStatus,
                    )
                }
            }
            .sortedBy { it.distance }

    /**
     * Folds [sourceId] into [targetId]: the source's files move across and the
     * source shot is deleted.
     *
     * Callers pass the shot the user is *looking at* as [targetId] — it keeps its
     * original file and its identity, and the thing under the user's finger does not
     * vanish. Same direction the web client uses, so the two cannot disagree about
     * which of two duplicates survives.
     */
    suspend fun merge(sourceId: String, targetId: String) {
        shotsApi.mergeShots(
            MergeShotsPayload().sourceId(sourceId).targetId(targetId)
        ).awaitVoid()
    }

    /** Marks shots reviewed, leaving their person assignment untouched. */
    suspend fun batchConfirm(shotIds: List<String>) {
        shotsApi.batchConfirm(BatchConfirmPayload().shotIds(shotIds)).awaitVoid()
    }

    /** Moves several shots to one person in a single request. */
    suspend fun batchReassign(shotIds: List<String>, personId: String) {
        shotsApi.batchReassign(
            BatchReassignPayload().shotIds(shotIds).personId(personId)
        ).awaitVoid()
    }

    /**
     * Deletes each of [shotIds], continuing past failures.
     *
     * Sequential rather than concurrent: these are deletes against one SQLite
     * database, and the ordering makes the partial-failure count meaningful.
     */
    suspend fun deleteShots(shotIds: List<String>): DeleteOutcome {
        var deleted = 0
        var failed = 0
        var firstError: String? = null
        for (id in shotIds) {
            try {
                deleteShot(id)
                deleted++
            } catch (e: Exception) {
                failed++
                if (firstError == null) firstError = e.message ?: e.javaClass.simpleName
            }
        }
        return DeleteOutcome(deleted, failed, firstError)
    }

    /** People to choose from when reassigning, most recently touched first. */
    suspend fun people(): List<Person> =
        peopleApi.getPeople().await().map { brief ->
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

    /**
     * Creates a person and returns their id, so "new person" and "assign to them"
     * can be one action for the user instead of two.
     */
    suspend fun createPerson(name: String): String =
        peopleApi.createPerson(CreatePersonPayload().name(name)).await().id
}
