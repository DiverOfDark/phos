package dev.phos.android.ui.browser

import androidx.lifecycle.SavedStateHandle
import dev.phos.android.data.repository.BrowseData
import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.data.repository.ReviewRepository
import dev.phos.android.data.repository.ShotRepository
import dev.phos.android.data.repository.ShotWithFiles
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.domain.model.Shot
import dev.phos.android.domain.model.ShotDetail
import io.mockk.coEvery
import io.mockk.coVerify
import io.mockk.every
import io.mockk.mockk
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.IOException

/**
 * Face edits from the browsing screen — the one place a user is actually looking
 * at the photo when they notice the detector boxed a poster or named the wrong
 * person.
 *
 * The rule being pinned is that a face edit is not a local change: the server
 * recomputes a shot's primary person from its faces, so every edit is followed by
 * a reload of the shot list, or the screen starts showing photos that no longer
 * belong to the person being browsed.
 */
class BrowserFacesTest {

    private val dispatcher = StandardTestDispatcher()

    private val browseRepository = mockk<BrowseRepository>(relaxed = true)
    private val shotRepository = mockk<ShotRepository>()
    private val reviewRepository = mockk<ReviewRepository>()

    @Before
    fun setUp() {
        Dispatchers.setMain(dispatcher)
        every { browseRepository.getViewPosition(any()) } returns null
        coEvery { browseRepository.fetchBrowseData(any()) } returns BrowseData(
            personName = "Anna",
            shots = listOf(shotWithFiles()),
        )
        coEvery { reviewRepository.shotDetail("shot-1") } returns detail
        coEvery { reviewRepository.faceSuggestions(any()) } returns emptyList()
        coEvery { shotRepository.people() } returns emptyList()
    }

    @After
    fun tearDown() = Dispatchers.resetMain()

    private val poster = Face(
        id = "face-poster",
        fileId = "file-1",
        personId = null,
        personName = null,
        x1 = 0f, y1 = 0f, x2 = 10f, y2 = 10f,
    )

    private val detail = ShotDetail(
        id = "shot-1",
        width = 4000,
        height = 3000,
        primaryPersonId = "person-1",
        primaryPersonName = "Anna",
        reviewStatus = "confirmed",
        timestamp = null,
        files = emptyList(),
        faces = listOf(poster),
        alsoContains = emptyList(),
    )

    private fun shotWithFiles() = ShotWithFiles(
        shot = Shot(
            id = "shot-1",
            timestamp = null,
            primaryPersonId = "person-1",
            reviewStatus = "confirmed",
        ),
        files = listOf(
            MediaFile(
                id = "file-1",
                shotId = "shot-1",
                mimeType = "image/jpeg",
                isOriginal = true,
                fileSize = 1,
                thumbnailUrl = null,
            ),
        ),
    )

    private fun viewModel() = BrowserViewModel(
        savedStateHandle = SavedStateHandle(mapOf("personId" to "person-1", "shot" to -1)),
        browseRepository = browseRepository,
        shotRepository = shotRepository,
        reviewRepository = reviewRepository,
        okHttpClient = mockk(relaxed = true),
        appContext = mockk(relaxed = true),
    )

    /** The browse endpoint returns no faces, so the sheet has to fetch them. */
    @Test
    fun faces_are_fetched_for_the_shot_on_screen() = runTest(dispatcher) {
        val vm = viewModel()
        runCurrent()

        vm.loadFaces("shot-1")
        runCurrent()

        assertEquals(listOf(poster), vm.uiState.value.faces)
        assertEquals("shot-1", vm.uiState.value.facesShotId)
    }

    /** "Not a face" deletes the box and nothing else, then re-reads both lists. */
    @Test
    fun deleting_a_face_reloads_the_shots_and_the_face_list() = runTest(dispatcher) {
        coEvery { reviewRepository.deleteFace("face-poster") } returns Unit
        val vm = viewModel()
        runCurrent()
        vm.loadFaces("shot-1")
        runCurrent()
        vm.openFace(poster)
        runCurrent()

        vm.deleteActiveFace()
        runCurrent()

        coVerify(exactly = 1) { reviewRepository.deleteFace("face-poster") }
        // Once for the initial load, once after the edit: the shot may have
        // changed owner, or stopped being this person's shot at all.
        coVerify(exactly = 2) { browseRepository.fetchBrowseData("person-1") }
        assertNull(vm.uiState.value.activeFace)
        assertEquals("Face removed", vm.uiState.value.message)
    }

    /** Naming a face is a face-level correction, not a shot move. */
    @Test
    fun assigning_a_face_reassigns_that_face() = runTest(dispatcher) {
        coEvery { reviewRepository.reassignFace(any(), any()) } returns Unit
        val vm = viewModel()
        runCurrent()
        vm.loadFaces("shot-1")
        runCurrent()
        vm.openFace(poster)
        runCurrent()

        vm.assignFace("person-2", "Bea")
        runCurrent()

        coVerify(exactly = 1) { reviewRepository.reassignFace("face-poster", "person-2") }
        assertEquals("Face is Bea", vm.uiState.value.message)
    }

    /** A failed edit says so and changes nothing — there is no retry queue. */
    @Test
    fun a_failed_delete_is_reported() = runTest(dispatcher) {
        coEvery { reviewRepository.deleteFace(any()) } throws IOException("offline")
        val vm = viewModel()
        runCurrent()
        vm.loadFaces("shot-1")
        runCurrent()
        vm.openFace(poster)
        runCurrent()

        vm.deleteActiveFace()
        runCurrent()

        assertTrue(vm.uiState.value.message!!.contains("offline"))
        assertEquals(listOf(poster), vm.uiState.value.faces)
    }
}
