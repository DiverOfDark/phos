package dev.phos.android.data.repository

import android.content.Context
import android.content.SharedPreferences
import dev.phos.android.data.remote.PhosApi
import dev.phos.android.data.remote.VersionResponse
import dev.phos.android.data.remote.model.AuthConfigResponse
import dev.phos.android.data.remote.model.PersonBrief
import dev.phos.android.data.remote.model.PersonBrowseResponse
import dev.phos.android.data.remote.model.TokenExchangeRequest
import dev.phos.android.data.remote.model.TokenResponse
import io.mockk.mockk
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

class TokenRefreshManagerTest {

    // --- shouldRefresh threshold math -------------------------------------

    private val hourMs = 60 * 60 * 1000L
    private val dayMs = 24 * hourMs

    @Test
    fun `no token means no refresh`() {
        assertFalse(TokenRefreshManager.shouldRefresh(nowMs = 1000, expiresAtMs = 0, ttlSeconds = 3600))
    }

    @Test
    fun `long ttl refreshes at half life`() {
        val ttlSeconds = 14 * 24 * 3600L // 14 days -> threshold is 7 days
        val expiresAt = 100 * dayMs
        assertFalse(TokenRefreshManager.shouldRefresh(expiresAt - 8 * dayMs, expiresAt, ttlSeconds))
        assertTrue(TokenRefreshManager.shouldRefresh(expiresAt - 6 * dayMs, expiresAt, ttlSeconds))
    }

    @Test
    fun `short ttl refreshes below one hour floor`() {
        val ttlSeconds = 1800L // threshold is max(1h, 15min) = 1h
        val expiresAt = 100 * dayMs
        assertTrue(TokenRefreshManager.shouldRefresh(expiresAt - 30 * 60 * 1000, expiresAt, ttlSeconds))
    }

    @Test
    fun `expired token still triggers refresh`() {
        val expiresAt = 100 * dayMs
        assertTrue(TokenRefreshManager.shouldRefresh(expiresAt + hourMs, expiresAt, 3600))
    }

    // --- single-flight -----------------------------------------------------

    @Test
    fun `concurrent callers trigger exactly one backend refresh`() {
        val repo = AuthRepository(FakeSharedPreferences())
        // 30-minute TTL puts the token under the 1h refresh floor immediately,
        // while still being unexpired.
        repo.saveToken("old-token", 1800)

        val refreshCalls = AtomicInteger(0)
        val api = FakePhosApi(onRefresh = {
            refreshCalls.incrementAndGet()
            Thread.sleep(50) // widen the race window
            TokenResponse().token("new-token").expiresIn(14 * 24 * 3600L)
        })
        val manager = TokenRefreshManager(repo, { api }, mockk<Context>(relaxed = true))

        val threads = 8
        val ready = CountDownLatch(threads)
        val go = CountDownLatch(1)
        val executor = Executors.newFixedThreadPool(threads)
        val results = (1..threads).map {
            executor.submit<String?> {
                ready.countDown()
                go.await()
                manager.ensureFreshToken()
            }
        }
        ready.await()
        go.countDown()
        executor.shutdown()
        assertTrue(executor.awaitTermination(10, TimeUnit.SECONDS))

        assertEquals(1, refreshCalls.get())
        results.forEach { assertEquals("new-token", it.get()) }
        assertEquals("new-token", repo.getToken())
        assertFalse(repo.isTokenExpired())
    }

    @Test
    fun `fresh token is returned without any network call`() {
        val repo = AuthRepository(FakeSharedPreferences())
        repo.saveToken("current-token", 14 * 24 * 3600L)

        val refreshCalls = AtomicInteger(0)
        val api = FakePhosApi(onRefresh = {
            refreshCalls.incrementAndGet()
            TokenResponse().token("unexpected").expiresIn(3600L)
        })
        val manager = TokenRefreshManager(repo, { api }, mockk<Context>(relaxed = true))

        assertEquals("current-token", manager.ensureFreshToken())
        assertEquals(0, refreshCalls.get())
    }
}

// --- fakes ------------------------------------------------------------------

private class FakePhosApi(private val onRefresh: () -> TokenResponse) : PhosApi {
    override fun refreshTokenCall(): retrofit2.Call<TokenResponse> =
        ImmediateCall { retrofit2.Response.success(onRefresh()) }

    override fun exchangeTokenCall(request: TokenExchangeRequest): retrofit2.Call<TokenResponse> =
        throw UnsupportedOperationException()

    override suspend fun getPeople(): List<PersonBrief> = throw UnsupportedOperationException()
    override suspend fun getPersonBrowse(id: String): PersonBrowseResponse = throw UnsupportedOperationException()
    override suspend fun getFileThumbnail(id: String, width: Int?): okhttp3.ResponseBody = throw UnsupportedOperationException()
    override suspend fun deleteFile(id: String): okhttp3.ResponseBody = throw UnsupportedOperationException()
    override suspend fun exchangeToken(request: TokenExchangeRequest): TokenResponse = throw UnsupportedOperationException()
    override suspend fun getAuthConfig(): AuthConfigResponse = throw UnsupportedOperationException()
    override suspend fun getVersion(): VersionResponse = throw UnsupportedOperationException()
}

private class ImmediateCall<T>(private val supplier: () -> retrofit2.Response<T>) : retrofit2.Call<T> {
    @Volatile private var executed = false
    override fun execute(): retrofit2.Response<T> {
        executed = true
        return supplier()
    }
    override fun enqueue(callback: retrofit2.Callback<T>) = throw UnsupportedOperationException()
    override fun isExecuted() = executed
    override fun cancel() {}
    override fun isCanceled() = false
    override fun clone(): retrofit2.Call<T> = ImmediateCall(supplier)
    override fun request(): okhttp3.Request = okhttp3.Request.Builder().url("http://localhost/").build()
    override fun timeout(): okio.Timeout = okio.Timeout.NONE
}

private class FakeSharedPreferences : SharedPreferences {
    private val map = mutableMapOf<String, Any?>()

    override fun getAll(): MutableMap<String, *> = synchronized(map) { HashMap(map) }
    override fun getString(key: String?, defValue: String?): String? =
        synchronized(map) { map[key] as? String ?: defValue }
    @Suppress("UNCHECKED_CAST")
    override fun getStringSet(key: String?, defValues: MutableSet<String>?): MutableSet<String>? =
        synchronized(map) { map[key] as? MutableSet<String> ?: defValues }
    override fun getInt(key: String?, defValue: Int): Int =
        synchronized(map) { map[key] as? Int ?: defValue }
    override fun getLong(key: String?, defValue: Long): Long =
        synchronized(map) { map[key] as? Long ?: defValue }
    override fun getFloat(key: String?, defValue: Float): Float =
        synchronized(map) { map[key] as? Float ?: defValue }
    override fun getBoolean(key: String?, defValue: Boolean): Boolean =
        synchronized(map) { map[key] as? Boolean ?: defValue }
    override fun contains(key: String?): Boolean = synchronized(map) { map.containsKey(key) }
    override fun edit(): SharedPreferences.Editor = FakeEditor()
    override fun registerOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener?) {}
    override fun unregisterOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener?) {}

    private inner class FakeEditor : SharedPreferences.Editor {
        private val pending = mutableMapOf<String, Any?>()
        private val removals = mutableSetOf<String>()
        private var clearAll = false

        override fun putString(key: String, value: String?) = also { pending[key] = value }
        override fun putStringSet(key: String, values: MutableSet<String>?) = also { pending[key] = values }
        override fun putInt(key: String, value: Int) = also { pending[key] = value }
        override fun putLong(key: String, value: Long) = also { pending[key] = value }
        override fun putFloat(key: String, value: Float) = also { pending[key] = value }
        override fun putBoolean(key: String, value: Boolean) = also { pending[key] = value }
        override fun remove(key: String) = also { removals.add(key) }
        override fun clear() = also { clearAll = true }
        override fun commit(): Boolean {
            apply()
            return true
        }
        override fun apply() {
            synchronized(map) {
                if (clearAll) map.clear()
                removals.forEach { map.remove(it) }
                map.putAll(pending)
            }
        }
    }
}
