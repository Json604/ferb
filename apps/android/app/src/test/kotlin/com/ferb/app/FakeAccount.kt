package com.ferb.app

import com.ferb.app.model.AccountInfo
import com.ferb.app.model.AccountModel
import com.ferb.app.model.AccountServer
import com.ferb.app.model.AccountStore
import com.ferb.app.model.Device
import com.ferb.app.model.Http
import com.ferb.app.model.HttpReply
import com.ferb.app.model.HttpRequest
import com.ferb.app.model.PendingSignIn
import com.ferb.app.model.SignedIn
import com.ferb.app.model.Tokens
import java.io.IOException
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking

// FERB's sign-in server and the account for the app's tests (kb PHN-827): invented data only, no network.

/** A fake sign-in server: answers by path, [offline] throws as an unreachable network does; records every request. */
class FakeHttp : Http {
    val requests = CopyOnWriteArrayList<HttpRequest>()

    /** Replies by path (`/api/app/token`, …), each used in turn; the last one stays. */
    val replies = HashMap<String, ArrayDeque<HttpReply>>()
    @Volatile var offline = false

    /** Holds each request until the test lets it go (one refresh at a time). */
    @Volatile var gate: java.util.concurrent.CountDownLatch? = null

    fun reply(path: String, status: Int, body: String, retryAfterS: Long? = null) {
        replies.getOrPut(path) { ArrayDeque() }.addLast(HttpReply(status, body, retryAfterS))
    }

    fun paths(): List<String> = requests.map { it.url.removePrefix(com.ferb.app.model.Site.URL) }

    override fun send(request: HttpRequest): HttpReply {
        requests.add(request)
        gate?.await()
        if (offline) throw IOException("no network (invented)")
        val path = request.url.removePrefix(com.ferb.app.model.Site.URL).substringBefore('?')
        val q = replies[path] ?: return HttpReply(404, """{"error":"not_found"}""")
        return if (q.size > 1) q.removeFirst() else q.first()
    }

    companion object {
        fun tokens(access: String = "fa1_invented-access-2", refresh: String = "fr1_invented-refresh-2", email: String = "someone@example.com",
            id: String = "acct-0001", providers: String = "\"google\""): String =
            """{"access_token":"$access","token_type":"Bearer","expires_in":3600,"refresh_token":"$refresh",""" +
                """"account":{"id":"$id","email":"$email","name":"Some One","providers":[$providers]}}"""
    }
}

/** The account kept in memory (the sealed store is held by its own test). */
class MemoryAccountStore(var kept: SignedIn? = null) : AccountStore {
    var pending: PendingSignIn? = null
    override fun load() = kept
    override fun save(s: SignedIn) { kept = s }
    override fun clear() { kept = null; pending = null }
    override fun loadPending() = pending
    override fun savePending(p: PendingSignIn) { pending = p }
    override fun clearPending() { pending = null }
}

val INVENTED_ACCOUNT = AccountInfo("acct-0001", "someone@example.com", "Some One", listOf("google"))

/**
 * An account model over [http] and [store], read before it is handed over. By default signed in with an access token
 * good for an hour of [now], and a server that gives a pairing statement.
 */
fun testAccount(
    http: FakeHttp = FakeHttp().apply { reply("/api/app/assertion", 200, """{"assertion":"fpa1.invented.statement","expires_in":300}""") },
    store: MemoryAccountStore = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 3_600_000L, "fr1_invented-refresh"))),
    now: () -> Long = { 0L },
    io: CoroutineDispatcher = Dispatchers.Unconfined,
): AccountModel = AccountModel(store, AccountServer(http, Device("invented-device-0001", "0.1.0-test"), now), io, now).also { runBlocking { it.load() } }

/** Signed out: nothing kept. */
fun signedOutAccount(http: FakeHttp = FakeHttp(), now: () -> Long = { 0L }): AccountModel = testAccount(http, MemoryAccountStore(), now)
