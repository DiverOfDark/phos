package dev.phos.android.data.repository

import dev.phos.android.data.remote.api.FacesApi
import dev.phos.android.data.remote.api.FilesApi
import dev.phos.android.data.remote.api.PeopleApi
import dev.phos.android.data.remote.api.ShotsApi
import dev.phos.android.data.remote.callOf
import dev.phos.android.data.remote.errorCall
import dev.phos.android.data.remote.voidCall
import dev.phos.android.data.remote.model.BatchReassignPayload
import dev.phos.android.data.remote.model.CreatePersonPayload
import dev.phos.android.data.remote.model.CreatedPerson
import dev.phos.android.data.remote.model.MergeShotsPayload
import dev.phos.android.data.remote.model.SimilarShotItem
import dev.phos.android.data.remote.model.SimilarShotsGrouped
import dev.phos.android.data.remote.model.SplitShotPayload
import dev.phos.android.data.remote.model.UpdateShotPayload
import io.mockk.every
import io.mockk.mockk
import io.mockk.slot
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import retrofit2.HttpException

/**
 * The write side of organizing.
 *
 * These are the calls that delete things, so the assertions are mostly about the
 * exact payload that goes out: a merge with its two ids swapped destroys the wrong
 * shot and looks like a success from every layer above.
 */
class ShotRepositoryTest {

    private val shotsApi = mockk<ShotsApi>()
    private val peopleApi = mockk<PeopleApi>()
    private val filesApi = mockk<FilesApi>()
    private val facesApi = mockk<FacesApi>()

    private val repository = ShotRepository(shotsApi, peopleApi, filesApi, facesApi)

    // ---- reassignment -----------------------------------------------------

    @Test
    fun `moving a shot sets the person and leaves review status alone`() = runTest {
        val payload = slot<UpdateShotPayload>()
        every { shotsApi.updateShot(eq("shot-1"), capture(payload)) } returns voidCall()

        repository.moveToPerson("shot-1", "person-9")

        assertEquals("person-9", payload.captured.primaryPersonId)
        // Moving a shot to the right person is not a claim that it has been
        // reviewed; sending a status here would empty the web's review queue.
        assertNull(payload.captured.reviewStatus)
    }

    @Test
    fun `a failed move is raised, not swallowed`() = runTest {
        every { shotsApi.updateShot(any(), any()) } returns errorCall(500)

        val thrown = runCatching { repository.moveToPerson("shot-1", "person-9") }.exceptionOrNull()

        assertTrue("expected the HTTP failure to reach the caller", thrown is HttpException)
    }

    // ---- split ------------------------------------------------------------

    @Test
    fun `splitting sends exactly the files that were picked`() = runTest {
        val payload = slot<SplitShotPayload>()
        every { shotsApi.splitShot(eq("shot-1"), capture(payload)) } returns voidCall()

        repository.split("shot-1", listOf("file-a", "file-c"))

        assertEquals(listOf("file-a", "file-c"), payload.captured.fileIds)
    }

    // ---- merge ------------------------------------------------------------

    @Test
    fun `merging absorbs the source into the target`() = runTest {
        // The direction is the whole point: the target survives and keeps its
        // original, the source is deleted. Same as the web client.
        val payload = slot<MergeShotsPayload>()
        every { shotsApi.mergeShots(capture(payload)) } returns voidCall()

        repository.merge(sourceId = "duplicate", targetId = "keeper")

        assertEquals("duplicate", payload.captured.sourceId)
        assertEquals("keeper", payload.captured.targetId)
    }

    @Test
    fun `similar shots are flattened across people and ordered by distance`() = runTest {
        every { shotsApi.getSimilarShots("shot-1") } returns callOf(
            listOf(
                group("anna", "Anna", item("far", distance = 9)),
                group(null, null, item("near", distance = 1), item("mid", distance = 4)),
            )
        )

        val candidates = repository.similarShots("shot-1")

        assertEquals(listOf("near", "mid", "far"), candidates.map { it.id })
        // The group's person travels down with each candidate — it is what the
        // confirmation dialog names before deleting someone else's shot.
        assertEquals(listOf(null, null, "Anna"), candidates.map { it.personName })
        assertEquals(1, candidates.first().distance)
    }

    @Test
    fun `a person with no similar shots contributes nothing`() = runTest {
        every { shotsApi.getSimilarShots("shot-1") } returns callOf(
            listOf(SimilarShotsGrouped().personId("anna").personName("Anna").shots(emptyList()))
        )

        assertTrue(repository.similarShots("shot-1").isEmpty())
    }

    // ---- batch ------------------------------------------------------------

    @Test
    fun `batch reassign sends every id and the target person`() = runTest {
        val payload = slot<BatchReassignPayload>()
        every { shotsApi.batchReassign(capture(payload)) } returns voidCall()

        repository.batchReassign(listOf("a", "b", "c"), "person-9")

        assertEquals(listOf("a", "b", "c"), payload.captured.shotIds)
        assertEquals("person-9", payload.captured.personId)
    }

    @Test
    fun `deleting several shots reports how far it got`() = runTest {
        // No batch-delete endpoint exists, so this is N requests and the middle one
        // failing must not be reported as either a clean success or a total failure.
        every { shotsApi.deleteShot("a") } returns voidCall()
        every { shotsApi.deleteShot("b") } returns errorCall(500)
        every { shotsApi.deleteShot("c") } returns voidCall()

        val outcome = repository.deleteShots(listOf("a", "b", "c"))

        assertEquals(2, outcome.deleted)
        assertEquals(1, outcome.failed)
        assertTrue(outcome.firstError != null)
    }

    @Test
    fun `a delete that fails outright still reports every attempt`() = runTest {
        every { shotsApi.deleteShot(any()) } returns errorCall(500)

        val outcome = repository.deleteShots(listOf("a", "b"))

        assertEquals(0, outcome.deleted)
        assertEquals(2, outcome.failed)
    }

    // ---- people -----------------------------------------------------------

    @Test
    fun `creating a person returns the new id so it can be assigned immediately`() = runTest {
        val payload = slot<CreatePersonPayload>()
        every { peopleApi.createPerson(capture(payload)) } returns
            callOf(CreatedPerson().id("person-new").name("Anna"))

        val id = repository.createPerson("Anna")

        assertEquals("Anna", payload.captured.name)
        assertEquals("person-new", id)
    }

    private fun group(
        personId: String?,
        personName: String?,
        vararg shots: SimilarShotItem,
    ): SimilarShotsGrouped = SimilarShotsGrouped()
        .personId(personId)
        .personName(personName)
        .shots(shots.toList())

    private fun item(id: String, distance: Int): SimilarShotItem = SimilarShotItem()
        .id(id)
        .thumbnailUrl("/api/files/$id/thumbnail")
        .fileCount(1L)
        .distance(distance)
}
