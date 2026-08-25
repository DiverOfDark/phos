package dev.phos.android.ui.settings

import android.content.Context
import dev.phos.android.data.repository.AuthRepository
import dev.phos.android.data.repository.ShotRepository
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
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.IOException
import kotlin.io.path.createTempDirectory

/**
 * Removing duplicate face boxes from Settings.
 *
 * The rule worth pinning is that the first tap never deletes: a face cannot be
 * brought back, so the user has to see the count and agree to that exact number.
 */
class DuplicateFaceCleanupTest {

    private val dispatcher = StandardTestDispatcher()

    private val authRepository = mockk<AuthRepository>(relaxed = true)
    private val shotRepository = mockk<ShotRepository>()
    private val context = mockk<Context>(relaxed = true)

    @Before
    fun setUp() {
        Dispatchers.setMain(dispatcher)
        every { authRepository.getServerUrl() } returns "http://phos.local"
        // The ViewModel sizes the image cache on construction, so the fake context
        // has to hand it a real directory to walk.
        every { context.cacheDir } returns createTempDirectory("phos-cache").toFile()
    }

    @After
    fun tearDown() = Dispatchers.resetMain()

    private fun viewModel() = SettingsViewModel(authRepository, shotRepository, context)

    @Test
    fun the_first_tap_only_counts() = runTest(dispatcher) {
        coEvery { shotRepository.dedupeFaces(true) } returns 4
        val vm = viewModel()
        runCurrent()

        vm.findDuplicateFaces()
        runCurrent()

        assertEquals(4, vm.uiState.value.dedupePending)
        coVerify(exactly = 1) { shotRepository.dedupeFaces(true) }
        coVerify(exactly = 0) { shotRepository.dedupeFaces(false) }
    }

    @Test
    fun the_second_tap_removes_and_clears_the_pending_count() = runTest(dispatcher) {
        coEvery { shotRepository.dedupeFaces(true) } returns 4
        coEvery { shotRepository.dedupeFaces(false) } returns 4
        val vm = viewModel()
        runCurrent()
        vm.findDuplicateFaces()
        runCurrent()

        vm.removeDuplicateFaces()
        runCurrent()

        coVerify(exactly = 1) { shotRepository.dedupeFaces(false) }
        assertEquals(0, vm.uiState.value.dedupePending)
        assertEquals("Removed 4 duplicate box(es).", vm.uiState.value.dedupeMessage)
    }

    /** Nothing found says so, and leaves no armed delete behind. */
    @Test
    fun finding_nothing_arms_nothing() = runTest(dispatcher) {
        coEvery { shotRepository.dedupeFaces(true) } returns 0
        val vm = viewModel()
        runCurrent()

        vm.findDuplicateFaces()
        runCurrent()

        assertEquals(0, vm.uiState.value.dedupePending)
        assertEquals("No duplicate boxes found.", vm.uiState.value.dedupeMessage)
    }

    @Test
    fun a_failure_is_reported_and_nothing_is_armed() = runTest(dispatcher) {
        coEvery { shotRepository.dedupeFaces(any()) } throws IOException("offline")
        val vm = viewModel()
        runCurrent()

        vm.findDuplicateFaces()
        runCurrent()

        assertEquals(0, vm.uiState.value.dedupePending)
        assertTrue(vm.uiState.value.dedupeMessage!!.contains("offline"))
    }
}
