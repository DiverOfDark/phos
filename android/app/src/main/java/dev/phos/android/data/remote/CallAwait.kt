package dev.phos.android.data.remote

import kotlinx.coroutines.suspendCancellableCoroutine
import retrofit2.Call
import retrofit2.Callback
import retrofit2.HttpException
import retrofit2.Response
import java.io.IOException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Bridges the generated Retrofit interfaces into coroutines.
 *
 * The OpenAPI generator's `java` generator emits blocking `Call<T>` regardless of
 * the `useCoroutines` option — that switch belongs to the `kotlin` generator, and
 * moving to it would rewrite every model in the app. `Call<T>` is also exactly what
 * the OkHttp interceptors need (they cannot suspend), so the generated interfaces
 * are used verbatim on both sides and this extension exists for the app code.
 *
 * Cancelling the calling coroutine cancels the HTTP call, so backing out of a screen
 * mid-request does not leave a socket open.
 *
 * A non-2xx response raises [HttpException] — the same exception Retrofit's own
 * coroutine support raises, so callers can inspect `code()` without caring where the
 * call came from.
 */
suspend fun <T : Any> Call<T>.await(): T = suspendCancellableCoroutine { continuation ->
    continuation.invokeOnCancellation { cancel() }

    enqueue(object : Callback<T> {
        override fun onResponse(call: Call<T>, response: Response<T>) {
            val body = response.body()
            when {
                !response.isSuccessful -> continuation.resumeWithException(HttpException(response))
                // An endpoint typed as returning something, that returned nothing.
                // Reported rather than papered over with a cast: the caller asked
                // for a value and there isn't one.
                body == null -> continuation.resumeWithException(
                    IOException("${call.request().url} answered ${response.code()} with an empty body")
                )
                else -> continuation.resume(body)
            }
        }

        override fun onFailure(call: Call<T>, t: Throwable) {
            continuation.resumeWithException(t)
        }
    })
}

/**
 * [await] for the endpoints that answer with no body at all.
 *
 * Every mutation in the API is one of these (`Call<Void>`), so it is worth its own
 * name: the alternative is an unchecked cast to make a null body look like a value,
 * which turns "this endpoint returns nothing" into a crash at the call site.
 */
suspend fun Call<Void>.awaitVoid(): Unit = suspendCancellableCoroutine { continuation ->
    continuation.invokeOnCancellation { cancel() }

    enqueue(object : Callback<Void> {
        override fun onResponse(call: Call<Void>, response: Response<Void>) {
            if (response.isSuccessful) {
                continuation.resume(Unit)
            } else {
                continuation.resumeWithException(HttpException(response))
            }
        }

        override fun onFailure(call: Call<Void>, t: Throwable) {
            continuation.resumeWithException(t)
        }
    })
}
