package com.ferb.app

import com.ferb.app.model.AccountModel
import com.ferb.app.model.AccountServer
import com.ferb.app.model.Answer
import com.ferb.app.model.Device
import com.ferb.app.model.Provider
import com.ferb.app.model.SealedAccountStore
import com.ferb.app.model.SignInStep
import com.ferb.app.model.SignedIn
import com.ferb.app.model.Site
import com.ferb.app.model.Tokens
import com.ferb.core.MemoryProtector
import java.io.File
import java.net.URI
import java.net.URLDecoder
import java.nio.file.Files
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** FERB's account on the phone over a fake sign-in server (kb PHN-827): no network, invented accounts and tokens. */
class AccountModelTest {
    private val dir: File = Files.createTempDirectory("ferb-account").toFile()
    private val http = FakeHttp()
    private var now = 1_000_000L

    @After
    fun tearDown() {
        dir.deleteRecursively()
    }

    private fun model(store: com.ferb.app.model.AccountStore = MemoryAccountStore()) =
        AccountModel(store, AccountServer(http, Device("invented-device-0001", "0.1.0-test")) { now }, Dispatchers.Unconfined, { now })
            .also { runBlocking { it.load() } }

    private fun query(url: String): Map<String, String> = URI(url).rawQuery.split('&').associate {
        val (k, v) = it.split('=', limit = 2)
        URLDecoder.decode(k, "UTF-8") to URLDecoder.decode(v, "UTF-8")
    }

    private fun callback(state: String, code: String? = "invented-code", error: String? = null) =
        "${Site.CALLBACK}?" + listOfNotNull(code?.let { "code=$it" }, error?.let { "error=$it" }, "state=$state").joinToString("&")

    // decision: PHN-839
    @Test
    fun the_browser_start_carries_a_pkce_challenge_and_a_state_of_its_own() = runBlocking {
        val m = model()
        val url = m.begin(Provider.APPLE)
        assertTrue(url.startsWith("${Site.URL}/api/app/start?"))
        val q = query(url)
        assertEquals("apple", q["provider"])
        assertEquals("android", q["platform"])
        assertEquals(Site.CALLBACK, q["redirect"])
        assertEquals("S256", q["code_challenge_method"])
        assertEquals(43, q["code_challenge"]!!.length)
        assertTrue(Regex("[A-Za-z0-9_-]{16,128}").matches(q["state"]!!))
        assertEquals(SignInStep.BROWSER, m.state.value.step)
        // a second sign-in has another state and challenge
        val again = query(m.begin(Provider.GOOGLE))
        assertFalse(again["state"] == q["state"])
        assertFalse(again["code_challenge"] == q["code_challenge"])
    }

    // decision: PHN-839
    @Test
    fun a_return_with_another_state_or_address_is_ignored() = runBlocking {
        val m = model()
        val state = query(m.begin(Provider.GOOGLE))["state"]!!
        http.reply("/api/app/token", 200, FakeHttp.tokens())
        assertFalse(m.returned(callback("another-state-0000000")))
        assertFalse(m.returned(callback(state).replace(Site.URL, "https://elsewhere.example.com")))
        assertFalse(m.returned(callback(state).replace("/app/callback", "/app/callback2")))
        assertFalse(m.returned("${Site.CALLBACK}?code=invented-code"))
        assertTrue("nothing was exchanged", http.requests.isEmpty())
        assertNull(m.state.value.account)
        assertEquals(SignInStep.BROWSER, m.state.value.step)
        // and a sign-in older than ten minutes is over
        now += AccountModel.PENDING_MS + 1
        assertFalse(m.returned(callback(state)))
        assertTrue(http.requests.isEmpty())
    }

    // decision: PHN-839
    @Test
    fun the_code_is_exchanged_with_its_verifier_and_the_tokens_are_kept_only_sealed() = runBlocking {
        val store = SealedAccountStore(dir, MemoryProtector())
        val m = model(store)
        val start = query(m.begin(Provider.GOOGLE))
        http.reply("/api/app/token", 200, FakeHttp.tokens(access = "fa1_INVENTEDACCESSTOKENTEXT", refresh = "fr1_INVENTEDREFRESHTOKENTEXT"))
        assertTrue(m.returned(callback(start["state"]!!)))
        assertEquals("someone@example.com", m.state.value.account?.email)
        assertEquals(SignInStep.IDLE, m.state.value.step)

        val body = http.requests.single().body!!
        assertTrue(body.contains("\"grant_type\":\"authorization_code\""))
        assertTrue(body.contains("\"code\":\"invented-code\""))
        assertTrue(body.contains("\"device\":{\"id\":\"invented-device-0001\",\"platform\":\"android\""))
        val verifier = Regex("\"code_verifier\":\"([^\"]+)\"").find(body)!!.groupValues[1]
        val s256 = AccountModel.b64(MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray()))
        assertEquals(start["code_challenge"], s256)

        // on disk: sealed bytes only, never the tokens' text nor the e-mail
        val files = dir.listFiles()!!.filter { it.isFile }
        assertTrue(files.isNotEmpty())
        for (f in files) {
            val bytes = String(f.readBytes(), Charsets.ISO_8859_1)
            for (secret in listOf("INVENTEDACCESSTOKENTEXT", "INVENTEDREFRESHTOKENTEXT", "someone@example.com", verifier)) {
                assertFalse("${f.name} holds $secret in the clear", bytes.contains(secret))
            }
        }
        // a new process reads it back
        val again = model(SealedAccountStore(dir, MemoryProtector())).state.value
        assertNull("another key can't open it: signed out, never a crash", again.account)
        val same = model(store).state.value
        assertEquals("acct-0001", same.account?.id)
        // the sign-in in the browser is used once
        assertFalse(m.returned(callback(start["state"]!!)))
    }

    // decision: PHN-839
    @Test
    fun the_browsers_errors_are_said_in_plain_words() = runBlocking {
        val m = model()
        assertTrue(m.returned(callback(query(m.begin(Provider.GOOGLE))["state"]!!, code = null, error = "cancelled")))
        assertEquals(R.string.signin_cancelled, m.state.value.note?.res)
        assertTrue(m.returned(callback(query(m.begin(Provider.GOOGLE))["state"]!!, code = null, error = "no_email")))
        assertEquals(R.string.signin_no_email, m.state.value.note?.res)
        assertTrue(m.returned(callback(query(m.begin(Provider.GOOGLE))["state"]!!, code = null, error = "failed")))
        assertEquals(R.string.signin_failed, m.state.value.note?.res)
        http.offline = true
        assertTrue(m.returned(callback(query(m.begin(Provider.GOOGLE))["state"]!!)))
        assertEquals(R.string.signin_unreachable, m.state.value.note?.res)
        http.offline = false
        http.reply("/api/app/token", 503, """{"error":"unavailable"}""")
        assertTrue(m.returned(callback(query(m.begin(Provider.GOOGLE))["state"]!!)))
        assertEquals(R.string.signin_not_answering, m.state.value.note?.res)
        assertNull(m.state.value.account)
        assertEquals(SignInStep.IDLE, m.state.value.step)
    }

    private fun signedIn(expiresAt: Long) = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_old-access", expiresAt, "fr1_old-refresh")))

    // decision: PHN-839
    @Test
    fun a_refresh_that_cant_reach_the_server_keeps_the_person_signed_in_and_invalid_grant_signs_out() = runBlocking {
        val store = signedIn(expiresAt = now + 30_000) // under a minute left: refreshed at the next use
        val m = model(store)
        http.offline = true
        assertEquals(Answer.Offline, m.access())
        assertNotNull("still signed in", m.state.value.account)
        assertTrue(m.state.value.checkFailed)
        http.offline = false
        http.reply("/api/app/token", 503, """{"error":"unavailable"}""")
        assertTrue(m.access() is Answer.Refused)
        assertNotNull("a server that isn't answering never signs out", m.state.value.account)
        http.replies.clear()
        http.reply("/api/app/token", 200, FakeHttp.tokens(access = "fa1_new-access"))
        assertEquals(Answer.Ok("fa1_new-access"), m.access())
        assertFalse(m.state.value.checkFailed)
        assertTrue(http.requests.last().body!!.contains("\"refresh_token\":\"fr1_old-refresh\""))
        // good for an hour now: no more requests
        val asked = http.requests.size
        assertEquals(Answer.Ok("fa1_new-access"), m.access())
        assertEquals(asked, http.requests.size)

        now += 3_600_000
        http.replies.clear()
        http.reply("/api/app/token", 401, """{"error":"invalid_grant"}""")
        val r = m.access()
        assertEquals(AccountModel.SIGNED_OUT, (r as Answer.Refused).error)
        assertNull(m.state.value.account)
        assertNull("nothing kept", store.kept)
    }

    // decision: PHN-839
    @Test
    fun one_refresh_at_a_time() = runBlocking {
        val m = AccountModel(signedIn(expiresAt = now), AccountServer(http, Device("invented-device-0001", "0.1.0-test")) { now }, Dispatchers.IO, { now })
        m.load()
        http.reply("/api/app/token", 200, FakeHttp.tokens(access = "fa1_new-access"))
        val gate = CountDownLatch(1)
        http.gate = gate
        val a = async(Dispatchers.IO) { m.access() }
        val b = async(Dispatchers.IO) { m.access() }
        Thread.sleep(200)
        gate.countDown()
        assertEquals(Answer.Ok("fa1_new-access"), a.await())
        assertEquals(Answer.Ok("fa1_new-access"), b.await())
        assertEquals(1, http.paths().count { it == "/api/app/token" })
    }

    // decision: PHN-839
    @Test
    fun an_access_token_the_server_refuses_is_refreshed_once_and_the_call_tried_again() = runBlocking {
        val m = model(signedIn(expiresAt = now + 3_600_000))
        http.reply("/api/app/assertion", 401, """{"error":"invalid_token"}""")
        http.reply("/api/app/assertion", 200, """{"assertion":"fpa1.invented-payload.invented-signature","expires_in":300}""")
        http.reply("/api/app/token", 200, FakeHttp.tokens(access = "fa1_new-access"))
        assertEquals(Answer.Ok("fpa1.invented-payload.invented-signature"), m.assertion("invented-endpoint"))
        assertEquals(listOf("/api/app/assertion", "/api/app/token", "/api/app/assertion"), http.paths())
        assertEquals("fa1_new-access", http.requests.last().bearer)
    }

    // decision: PHN-839, PHN-843
    @Test
    fun sign_out_tells_the_server_first_then_forgets_the_account_here_whatever_it_said() = runBlocking {
        // the server answers: its tokens revoked there, everything here gone, nothing to say
        val store = signedIn(expiresAt = now + 3_600_000)
        val m = model(store)
        http.reply("/api/app/signout", 200, """{"ok":true}""")
        m.signOut()
        assertNull(m.state.value.account)
        assertNull(store.kept)
        assertEquals(listOf("/api/app/signout"), http.paths())
        val sent = http.requests.single()
        assertTrue(sent.body!!.contains("fr1_old-refresh"))
        assertEquals("fa1_old-access", sent.bearer)
        assertNull(m.state.value.note)

        // unreachable, or the server failing: signed out here all the same, and the sign-in page says so
        for (offline in listOf(true, false)) {
            http.requests.clear()
            http.replies.clear()
            http.offline = offline
            if (!offline) http.reply("/api/app/signout", 503, """{"error":"unavailable"}""")
            val kept = signedIn(expiresAt = now + 3_600_000)
            val m2 = model(kept)
            m2.signOut()
            assertEquals(listOf("/api/app/signout"), http.paths())
            assertNull(m2.state.value.account)
            assertNull(kept.kept)
            assertEquals(Note(R.string.signout_unreachable), m2.state.value.note)
        }
    }

    // decision: PHN-843
    @Test
    fun the_tokens_stay_until_the_server_was_asked() = runBlocking {
        val store = signedIn(expiresAt = now + 3_600_000)
        val m = AccountModel(store, AccountServer(http, Device("invented-device-0001", "0.1.0-test")) { now }, Dispatchers.IO, { now })
            .also { it.load() }
        http.reply("/api/app/signout", 200, """{"ok":true}""")
        val held = CountDownLatch(1)
        http.gate = held
        val out = async(Dispatchers.Default) { m.signOut() }
        while (http.requests.isEmpty()) Thread.sleep(5)
        assertNotNull("kept while the server is asked", store.kept)
        assertNotNull(m.state.value.account)
        held.countDown()
        out.await()
        assertNull(store.kept)
        assertNull(m.state.value.account)
    }

    // decision: PHN-839
    @Test
    fun delete_keeps_everything_until_the_server_deleted_the_account() = runBlocking {
        val store = signedIn(expiresAt = now + 3_600_000)
        val m = model(store)
        http.reply("/api/app/delete", 503, """{"error":"unavailable"}""")
        assertFalse(m.delete())
        assertNotNull(m.state.value.account)
        assertNotNull(store.kept)
        http.replies.clear()
        http.reply("/api/app/delete", 200, """{"deleted":true}""")
        assertTrue(m.delete())
        assertNull(m.state.value.account)
        assertNull(store.kept)
    }

    // decision: PHN-839
    @Test
    fun nothing_secret_is_in_what_a_log_would_print() {
        val t = Tokens("fa1_SECRET-ACCESS", 1L, "fr1_SECRET-REFRESH")
        val r = com.ferb.app.model.HttpRequest("POST", "${Site.URL}/api/app/token?x=1", """{"refresh_token":"fr1_SECRET-REFRESH"}""", "fa1_SECRET-ACCESS")
        for (shown in listOf(t.toString(), r.toString(), SignedIn(INVENTED_ACCOUNT, t).toString())) {
            assertFalse(shown, shown.contains("SECRET"))
        }
    }
}
