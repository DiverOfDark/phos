package dev.phos.android.data.remote

import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Request
import okhttp3.ResponseBody.Companion.toResponseBody
import retrofit2.Call
import retrofit2.Callback
import retrofit2.Response

/**
 * A [Call] that answers from a supplier, with no network and no dispatcher.
 *
 * The generated APIs return `Call<T>`, so every fake of one has to produce these.
 * Both `execute()` and `enqueue()` are real: the interceptors call the former and
 * [dev.phos.android.data.remote.await] calls the latter, and a fake that implements
 * only one silently passes the tests that happen not to use it.
 */
internal class ImmediateCall<T>(private val supplier: () -> Response<T>) : Call<T> {
    @Volatile private var executed = false

    override fun execute(): Response<T> {
        executed = true
        return supplier()
    }

    override fun enqueue(callback: Callback<T>) {
        executed = true
        val response = try {
            supplier()
        } catch (t: Throwable) {
            callback.onFailure(this, t)
            return
        }
        callback.onResponse(this, response)
    }

    override fun isExecuted() = executed
    override fun cancel() {}
    override fun isCanceled() = false
    override fun clone(): Call<T> = ImmediateCall(supplier)
    override fun request(): Request = Request.Builder().url("http://localhost/").build()
    override fun timeout(): okio.Timeout = okio.Timeout.NONE
}

/** A call that succeeds with [value]. */
internal fun <T> callOf(value: T): Call<T> = ImmediateCall { Response.success(value) }

/** A call to a `Call<Void>` endpoint that succeeds with no body — every mutation. */
internal fun voidCall(): Call<Void> = ImmediateCall { Response.success(null) }

/** A call whose transport fails — no response at all, the offline case. */
internal fun <T> failingCall(cause: Throwable): Call<T> = ImmediateCall { throw cause }

/** A call that completes with an HTTP error status. */
internal fun <T> errorCall(code: Int): Call<T> = ImmediateCall {
    Response.error(code, "".toResponseBody("application/json".toMediaType()))
}
