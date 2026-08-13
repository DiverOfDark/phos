package dev.phos.android.ui.grid

import androidx.lifecycle.SavedStateHandle
import dev.phos.android.data.repository.BrowseData
import dev.phos.android.data.repository.BrowseRepository
import dev.phos.android.data.repository.ShotRepository
import dev.phos.android.data.repository.ShotWithFiles
import dev.phos.android.domain.model.Shot
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
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.IOException

/**
 * Multi-select on the person grid.
 *
 * The rule worth pinning is what happens after a batch action *fails*: the selection
 * is dropped anyway. Keeping it would leave the user one tap from retrying against
 * shots that may no longer be what they were — and a stale selection of shot ids is
 * how a retry deletes the wrong thing.
 */
class PersonGridSelectionTest {

    private val dispatcher = StandardTestDispatcher()

    private val browseRepository = mockk<BrowseRepository>(relaxed = true)
    private val shotRepository = mockk<ShotRepository>()

    @Before
    fun setUp() {
        Dispatchers.setMain(dispatcher)
        every { browseRepository.getViewPosition(any()) } returns null
        coEvery { browseRepository.fetchBrowseData(any()) } returns BrowseData(
            personName = "Anna",
            shots = listOf(shot("a"), shot("b"), shot("c")),
        )
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
    }

    private fun shot(id: String) = ShotWithFiles(
        shot = Shot(id = id, timestamp = null, primaryPersonId = "person-1", reviewStatus = "pending"),
        files = emptyList(),
    )

    private fun viewModel() = PersonGridViewModel(
        savedStateHandle = SavedStateHandle(mapOf("personId" to "person-1")),
        browseRepository = browseRepository,
        shotRepository = shotRepository,
    )

    @Test
    fun `selection mode starts empty and toggles per shot`() = runTest(dispatcher) {
        val vm = viewModel()
        runCurrent()

        assertFalse(vm.uiState.value.selectionMode)

        vm.toggleSelection("a")
        vm.toggleSelection("b")
        assertEquals(setOf("a", "b"), vm.uiState.value.selected)
        assertTrue(vm.uiState.value.selectionMode)

        vm.toggleSelection("a")
        assertEquals(setOf("b"), vm.uiState.value.selected)
    }

    @Test
    fun `select all picks every tile that is loaded`() = runTest(dispatcher) {
        val vm = viewModel()
        runCurrent()

        vm.selectAll()

        assertEquals(setOf("a", "b", "c"), vm.uiState.value.selected)
    }

    @Test
    fun `a successful batch move clears the selection and reports it`() = runTest(dispatcher) {
        coEvery { shotRepository.batchReassign(any(), any()) } returns Unit
        val vm = viewModel()
        runCurrent()
        vm.toggleSelection("a")
        vm.toggleSelection("b")

        vm.moveSelectedTo("person-9", "Bob")
        runCurrent()

        coVerify { shotRepository.batchReassign(listOf("a", "b"), "person-9") }
        assertEquals(emptySet<String>(), vm.uiState.value.selected)
        assertEquals("Moved 2 shot(s) to Bob", vm.uiState.value.message)
    }

    @Test
    fun `a failed batch move says so and still clears the selection`() = runTest(dispatcher) {
        coEvery { shotRepository.batchReassign(any(), any()) } throws IOException("no route to host")
        val vm = viewModel()
        runCurrent()
        vm.toggleSelection("a")

        vm.moveSelectedTo("person-9", "Bob")
        runCurrent()

        assertTrue(vm.uiState.value.message!!.startsWith("Failed:"))
        assertEquals(emptySet<String>(), vm.uiState.value.selected)
        assertFalse(vm.uiState.value.busy)
    }

    @Test
    fun `a half-finished delete reports how far it got`() = runTest(dispatcher) {
        coEvery { shotRepository.deleteShots(any()) } returns
            dev.phos.android.data.repository.DeleteOutcome(
                deleted = 1,
                failed = 1,
                firstError = "HTTP 500",
            )
        val vm = viewModel()
        runCurrent()
        vm.toggleSelection("a")
        vm.toggleSelection("b")

        vm.deleteSelected()
        runCurrent()

        val message = vm.uiState.value.message!!
        assertTrue(message, message.contains("Deleted 1 of 2"))
        assertTrue(message, message.contains("HTTP 500"))
    }

    @Test
    fun `batch actions on an empty selection do nothing at all`() = runTest(dispatcher) {
        // `shotRepository` is a strict mock: any call here would fail the test.
        val vm = viewModel()
        runCurrent()

        vm.confirmSelected()
        vm.deleteSelected()
        vm.moveSelectedTo("person-9", "Bob")
        runCurrent()

        assertEquals(null, vm.uiState.value.message)
    }
}
