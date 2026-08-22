package dev.phos.android.ui.people

import dev.phos.android.data.repository.AuthRepository
import dev.phos.android.data.repository.PeopleRepository
import dev.phos.android.domain.model.Person
import io.mockk.coEvery
import io.mockk.every
import io.mockk.mockk
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import java.io.IOException

/**
 * What the people grid shows.
 *
 * The server keeps a person after their last shot has been moved or deleted, so
 * the list it returns is not the list worth drawing — an empty person is a tile
 * that opens onto nothing.
 */
class PeopleListTest {

    private val dispatcher = StandardTestDispatcher()

    private val peopleRepository = mockk<PeopleRepository>()
    private val authRepository = mockk<AuthRepository>(relaxed = true)

    @Before
    fun setUp() {
        Dispatchers.setMain(dispatcher)
        every { authRepository.authExpired } returns MutableStateFlow(false)
        coEvery { peopleRepository.fetchUnsortedCount() } returns 7
    }

    @After
    fun tearDown() = Dispatchers.resetMain()

    private fun person(id: String, shots: Int) = Person(
        id = id,
        name = id,
        faceCount = 3,
        thumbnailUrl = null,
        shotCount = shots,
        pendingCount = 0,
        updatedAt = null,
        coverShotThumbnailUrl = null,
    )

    @Test
    fun people_with_no_shots_are_left_out() = runTest(dispatcher) {
        coEvery { peopleRepository.fetchPeople() } returns listOf(
            person("anna", 4),
            person("emptied", 0),
            person("bea", 1),
        )

        val vm = PeopleViewModel(peopleRepository, authRepository)
        runCurrent()

        assertEquals(listOf("anna", "bea"), vm.people.value.map { it.id })
        assertEquals(7, vm.unsortedCount.value)
    }

    /** The Unsorted tile is not a person, so a filtered-out list keeps its count. */
    @Test
    fun the_unsorted_count_survives_an_all_empty_library() = runTest(dispatcher) {
        coEvery { peopleRepository.fetchPeople() } returns listOf(person("emptied", 0))

        val vm = PeopleViewModel(peopleRepository, authRepository)
        runCurrent()

        assertEquals(emptyList<String>(), vm.people.value.map { it.id })
        assertEquals(7, vm.unsortedCount.value)
    }

    /** A failed people call still lets the Unsorted tile load, and says so. */
    @Test
    fun a_failed_people_call_is_reported_without_losing_the_unsorted_count() =
        runTest(dispatcher) {
            coEvery { peopleRepository.fetchPeople() } throws IOException("offline")

            val vm = PeopleViewModel(peopleRepository, authRepository)
            runCurrent()

            assertEquals(emptyList<Person>(), vm.people.value)
            assertEquals(7, vm.unsortedCount.value)
            assertEquals("Failed to refresh: offline", vm.error.value)
        }
}
