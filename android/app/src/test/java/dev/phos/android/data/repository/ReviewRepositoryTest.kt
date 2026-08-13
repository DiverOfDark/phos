package dev.phos.android.data.repository

import dev.phos.android.data.remote.api.FacesApi
import dev.phos.android.data.remote.api.ShotsApi
import dev.phos.android.data.remote.callOf
import dev.phos.android.data.remote.model.FaceDetail
import dev.phos.android.data.remote.model.FaceSuggestion
import dev.phos.android.data.remote.model.FileDetail
import dev.phos.android.data.remote.model.ReassignFacePayload
import dev.phos.android.data.remote.model.ShotBrief
import dev.phos.android.data.remote.model.ShotDetailResponse
import dev.phos.android.data.remote.voidCall
import io.mockk.every
import io.mockk.mockk
import io.mockk.slot
import io.mockk.verify
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the review queue reads, and the face edits that fix a wrong guess. */
class ReviewRepositoryTest {

    private val shotsApi = mockk<ShotsApi>()
    private val facesApi = mockk<FacesApi>()

    private val repository = ReviewRepository(shotsApi, facesApi)

    @Test
    fun `the queue asks for pending shots across every person`() = runTest {
        every { shotsApi.getShots(any(), any(), any(), any(), any()) } returns callOf(
            listOf(
                ShotBrief().id("s1").primaryPersonName("Anna").fileCount(2L)
                    .thumbnailUrl("/api/files/f1/thumbnail"),
            )
        )

        val queue = repository.pendingShots()

        // status=pending, and every other filter left null — the queue is not scoped
        // to a person, because the point is to find mistakes without having to know
        // where they are first.
        verify { shotsApi.getShots(null, null, "pending", null, null) }
        assertEquals(1, queue.size)
        assertEquals("Anna", queue.first().personName)
        assertEquals(2, queue.first().fileCount)
    }

    @Test
    fun `shot detail carries the faces and the size their boxes are measured in`() = runTest {
        every { shotsApi.getShotDetail("s1") } returns callOf(
            ShotDetailResponse()
                .id("s1")
                .width(4000L)
                .height(3000L)
                .primaryPersonName("Anna")
                .files(listOf(FileDetail().id("f1").isOriginal(true)))
                .faces(
                    listOf(
                        FaceDetail().id("face-1").fileId("f1").personName("Anna")
                            .boxX1(100f).boxY1(200f).boxX2(300f).boxY2(400f),
                    )
                )
        )

        val detail = repository.shotDetail("s1")

        // Without width/height there is no way to place a pixel-coordinate box over
        // a scaled image, so losing them here would silently break the overlay.
        assertEquals(4000, detail.width)
        assertEquals(3000, detail.height)
        val face = detail.faces.single()
        assertEquals("f1", face.fileId)
        assertEquals(100f, face.x1, 0f)
        assertEquals(400f, face.y2, 0f)
    }

    @Test
    fun `suggestions come back nearest first`() = runTest {
        every { facesApi.getFaceSuggestions("face-1") } returns callOf(
            listOf(
                suggestion("far", 0.9f),
                suggestion("near", 0.1f),
                suggestion("mid", 0.5f),
            )
        )

        val suggestions = repository.faceSuggestions("face-1")

        assertEquals(listOf("near", "mid", "far"), suggestions.map { it.personId })
    }

    @Test
    fun `assigning a face names the person on the face endpoint`() = runTest {
        val payload = slot<ReassignFacePayload>()
        every { facesApi.reassignFace(eq("face-1"), capture(payload)) } returns voidCall()

        repository.reassignFace("face-1", "person-9")

        assertEquals("person-9", payload.captured.personId)
    }

    @Test
    fun `deleting a face touches only the face`() = runTest {
        every { facesApi.deleteFace("face-1") } returns voidCall()

        repository.deleteFace("face-1")

        verify { facesApi.deleteFace("face-1") }
        // Nothing on the shot or file APIs — "not a face" removes the box, not the
        // photo, and confusing the two would delete the user's picture.
        verify(exactly = 0) { shotsApi.deleteShot(any()) }
    }

    private fun suggestion(personId: String, distance: Float) = FaceSuggestion()
        .personId(personId)
        .personName(personId)
        .distance(distance)
}
