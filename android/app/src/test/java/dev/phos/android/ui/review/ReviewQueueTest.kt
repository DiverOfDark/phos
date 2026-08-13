package dev.phos.android.ui.review

import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.data.repository.ReviewRepository
import dev.phos.android.data.repository.ShotRepository
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.PendingShot
import dev.phos.android.domain.model.ShotDetail
import io.mockk.coEvery
import io.mockk.coVerify
import io.mockk.mockk
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.IOException

/**
 * How the queue moves.
 *
 * The rule that matters is which actions *settle* a shot and which do not. A verdict
 * takes it out of the queue and moves on; skip leaves it pending; a face edit leaves
 * the reviewer on the same picture, because fixing one face in a group photo is
 * usually the first of several.
 */
class ReviewQueueTest {

    private val dispatcher = StandardTestDispatcher()

    private val reviewRepository = mockk<ReviewRepository>()
    private val shotRepository = mockk<ShotRepository>()
    private val browseRepository = mockk<BrowseRepository>(relaxed = true)

    @Before
    fun setUp() {
        Dispatchers.setMain(dispatcher)
        coEvery { reviewRepository.pendingShots() } returns listOf(
            pending("s1"), pending("s2"), pending("s3"),
        )
        coEvery { reviewRepository.shotDetail(any()) } answers { detail(firstArg()) }
    }

    @After
    fun tearDown() = Dispatchers.resetMain()

    private fun pending(id: String) = PendingShot(
        id = id,
        thumbnailUrl = "/api/files/$id/thumbnail",
        personName = "Anna",
        fileCount = 1,
        timestamp = null,
    )

    private fun detail(id: String) = ShotDetail(
        id = id,
        width = 4000,
        height = 3000,
        primaryPersonId = "person-1",
        primaryPersonName = "Anna",
        reviewStatus = "pending",
        timestamp = null,
        files = emptyList(),
        faces = listOf(face(id)),
        alsoContains = emptyList(),
    )

    private fun face(shotId: String) = Face(
        id = "face-$shotId",
        fileId = "file-$shotId",
        personId = "person-1",
        personName = "Anna",
        x1 = 0f, y1 = 0f, x2 = 10f, y2 = 10f,
    )

    private fun viewModel() = ReviewViewModel(reviewRepository, shotRepository, browseRepository)

    @Test
    fun `the queue loads and opens on the first shot`() = runTest(dispatcher) {
        val vm = viewModel()
        runCurrent()

        assertEquals(3, vm.uiState.value.remaining)
        assertEquals("s1", vm.uiState.value.current?.id)
        assertEquals("s1", vm.uiState.value.detail?.id)
    }

    @Test
    fun `confirming settles the shot and moves to the next`() = runTest(dispatcher) {
        coEvery { shotRepository.confirm("s1") } returns Unit
        val vm = viewModel()
        runCurrent()

        vm.confirm()
        runCurrent()

        coVerify { shotRepository.confirm("s1") }
        assertEquals(2, vm.uiState.value.remaining)
        assertEquals("s2", vm.uiState.value.current?.id)
        assertEquals(1, vm.uiState.value.reviewed)
    }

    @Test
    fun `skip leaves the shot pending`() = runTest(dispatcher) {
        val vm = viewModel()
        runCurrent()

        vm.skip()
        runCurrent()

        // Still three: "not now" is not a verdict, and a skipped shot has to come
        // back or it is silently lost.
        assertEquals(3, vm.uiState.value.remaining)
        assertEquals("s2", vm.uiState.value.current?.id)
        assertEquals(0, vm.uiState.value.reviewed)
    }

    @Test
    fun `skip wraps around at the end of the queue`() = runTest(dispatcher) {
        val vm = viewModel()
        runCurrent()

        repeat(3) { vm.skip(); runCurrent() }

        assertEquals("s1", vm.uiState.value.current?.id)
    }

    @Test
    fun `a failed verdict keeps the shot in the queue`() = runTest(dispatcher) {
        coEvery { shotRepository.confirm("s1") } throws IOException("no route to host")
        val vm = viewModel()
        runCurrent()

        vm.confirm()
        runCurrent()

        assertEquals(3, vm.uiState.value.remaining)
        assertEquals("s1", vm.uiState.value.current?.id)
        assertTrue(vm.uiState.value.message!!.startsWith("Failed:"))
        assertEquals(0, vm.uiState.value.reviewed)
    }

    @Test
    fun `settling the last shot empties the queue rather than running off the end`() =
        runTest(dispatcher) {
            coEvery { reviewRepository.pendingShots() } returns listOf(pending("only"))
            coEvery { shotRepository.confirm("only") } returns Unit
            val vm = viewModel()
            runCurrent()

            vm.confirm()
            runCurrent()

            assertTrue(vm.uiState.value.isEmpty)
            assertEquals(null, vm.uiState.value.current)
            assertEquals(null, vm.uiState.value.detail)
        }

    @Test
    fun `moving to a person settles the shot too`() = runTest(dispatcher) {
        coEvery { shotRepository.moveToPerson("s1", "person-9") } returns Unit
        val vm = viewModel()
        runCurrent()

        vm.moveToPerson("person-9", "Bob")
        runCurrent()

        assertEquals(2, vm.uiState.value.remaining)
        assertEquals("Moved to Bob", vm.uiState.value.message)
    }

    @Test
    fun `assigning a face stays on the same shot`() = runTest(dispatcher) {
        coEvery { reviewRepository.faceSuggestions(any()) } returns emptyList()
        coEvery { reviewRepository.reassignFace(any(), any()) } returns Unit
        coEvery { shotRepository.people() } returns emptyList()
        val vm = viewModel()
        runCurrent()

        vm.openFace(face("s1"))
        runCurrent()
        vm.assignFace("person-9", "Bob")
        runCurrent()

        coVerify { reviewRepository.reassignFace("face-s1", "person-9") }
        // The reviewer is still working on this picture — a group photo usually
        // needs more than one correction.
        assertEquals(3, vm.uiState.value.remaining)
        assertEquals("s1", vm.uiState.value.current?.id)
        assertEquals(null, vm.uiState.value.activeFace)
    }

    @Test
    fun `splitting reloads the shot instead of settling it`() = runTest(dispatcher) {
        coEvery { shotRepository.split(any(), any()) } returns Unit
        val vm = viewModel()
        runCurrent()

        vm.split(listOf("file-a"))
        runCurrent()

        // The shot is still unreviewed, and now it is a different shot than it was.
        assertEquals(3, vm.uiState.value.remaining)
        assertEquals("s1", vm.uiState.value.current?.id)
    }
}
