package com.ferb.app.model

import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/**
 * FERB's sign-in server (the site, kb PHN-827): one account for the site, FERB on the Mac and the phones. Its address
 * is fixed in the app and never set at run time.
 */
object Site {
    const val URL = "https://ferb.kartikey.xyz"

    /** Where the server sends the browser back: a verified Android App Link to this app (the manifest's filter). */
    const val CALLBACK = "$URL/app/callback"

    /** The download page the phone-first screen names and the e-mailed link opens. */
    const val SHOWN = "ferb.kartikey.xyz"
}

/**
 * One HTTP request. Not a data class: its [bearer] must never reach a log through toString.
 */
class HttpRequest(val method: String, val url: String, val body: String? = null, val bearer: String? = null) {
    override fun toString() = "$method ${url.substringBefore('?')}"
}

class HttpReply(val status: Int, val body: String, val retryAfterS: Long? = null) {
    override fun toString() = "HTTP $status"
}

/** The app's one way to the sign-in server, so the tests answer with a fake server (no network in tests). */
fun interface Http {
    /** Blocking; never on the main thread. @throws IOException when the server can't be reached at all. */
    @Throws(IOException::class)
    fun send(request: HttpRequest): HttpReply
}

/** [Http] by the platform's HttpURLConnection: HTTPS only, no caches, no redirects followed, bounded waits. */
class UrlConnectionHttp(private val connectMs: Int = 15_000, private val readMs: Int = 20_000) : Http {
    override fun send(request: HttpRequest): HttpReply {
        require(request.url.startsWith("https://")) { "the sign-in server is reached over HTTPS only" }
        val c = URL(request.url).openConnection() as HttpURLConnection
        try {
            c.requestMethod = request.method
            c.connectTimeout = connectMs
            c.readTimeout = readMs
            c.useCaches = false
            c.instanceFollowRedirects = false
            c.setRequestProperty("Accept", "application/json")
            request.bearer?.let { c.setRequestProperty("Authorization", "Bearer $it") }
            if (request.body != null) {
                c.doOutput = true
                c.setRequestProperty("Content-Type", "application/json")
                c.outputStream.use { it.write(request.body.toByteArray(Charsets.UTF_8)) }
            }
            val status = c.responseCode
            val stream = if (status >= 400) c.errorStream else c.inputStream
            val body = stream?.use { it.readBytes().toString(Charsets.UTF_8) }.orEmpty()
            return HttpReply(status, body, c.getHeaderField("retry-after")?.trim()?.toLongOrNull())
        } finally {
            c.disconnect()
        }
    }
}

/** What the server said: the value, unreachable (no network), or a refusal with its status and error code. */
sealed interface Answer<out T> {
    data class Ok<T>(val value: T) : Answer<T>
    data object Offline : Answer<Nothing>

    /** [error]: the body's `error` (`invalid_grant`, `invalid_token`, `slow_down`, `unavailable`, …). */
    data class Refused(val status: Int, val error: String?, val retryAfterS: Long? = null) : Answer<Nothing>
}

/** The account as the server describes it: its id (matched by the Mac, never the e-mail text), e-mail, providers. */
data class AccountInfo(val id: String, val email: String, val name: String?, val providers: List<String>)

/** A token pair. Not a data class: the token values must never reach a log through toString. */
class Tokens(val access: String, val accessExpiresAtMs: Long, val refresh: String) {
    override fun toString() = "Tokens(expires at $accessExpiresAtMs)"
}

class SignedIn(val account: AccountInfo, val tokens: Tokens) {
    override fun toString() = "SignedIn(${account.id})"
}

/** This install as the server records it: a random id (not secret), the platform, the app's version. */
class Device(val id: String, val version: String)

/** The sign-in server's API (the spec's /api/app/ calls), each call blocking: callers run it off the main thread. */
class AccountServer(private val http: Http, private val device: Device, private val now: () -> Long) {
    /** The browser's first page: the provider's sign-in, with this sign-in's state and PKCE challenge. */
    fun startUrl(provider: Provider, state: String, challenge: String): String =
        "${Site.URL}/api/app/start?provider=${provider.word}&platform=android&redirect=${enc(Site.CALLBACK)}" +
            "&state=${enc(state)}&code_challenge=${enc(challenge)}&code_challenge_method=S256"

    fun exchange(code: String, verifier: String): Answer<SignedIn> =
        tokens("""{"grant_type":"authorization_code","code":${Json.quote(code)},"code_verifier":${Json.quote(verifier)},"device":${deviceJson()}}""")

    fun refresh(refresh: String): Answer<SignedIn> =
        tokens("""{"grant_type":"refresh_token","refresh_token":${Json.quote(refresh)},"device":${deviceJson()}}""")

    /** Revokes this sign-in's tokens on the server (always 200 there). */
    fun signOut(tokens: Tokens): Answer<Unit> =
        call("POST", "/api/app/signout", """{"refresh_token":${Json.quote(tokens.refresh)}}""", tokens.access).map { }

    fun delete(access: String): Answer<Unit> = call("POST", "/api/app/delete", "{}", access).map { }

    /** The server's signed statement that this account is signed in on the phone whose link id is [endpoint]. */
    fun assertion(access: String, endpoint: String): Answer<String> =
        call("POST", "/api/app/assertion", """{"endpoint":${Json.quote(endpoint)}}""", access).then { o ->
            o.str("assertion")?.let { Answer.Ok(it) } ?: Answer.Refused(200, "bad_reply")
        }

    /** E-mails the download link to the account's address; the address it went to. */
    fun sendLink(access: String): Answer<String> =
        call("POST", "/api/app/send-link", "{}", access).then { o -> Answer.Ok(o.str("email").orEmpty()) }

    private fun tokens(body: String): Answer<SignedIn> = call("POST", "/api/app/token", body, null).then { o ->
        val access = o.str("access_token")
        val refresh = o.str("refresh_token")
        val expires = (o["expires_in"] as? Double)?.toLong()
        val a = o.obj("account")
        val id = a?.str("id")
        val email = a?.str("email")
        if (access == null || refresh == null || expires == null || id == null || email == null) {
            Answer.Refused(200, "bad_reply")
        } else {
            Answer.Ok(SignedIn(AccountInfo(id, email, a.str("name"), a.strings("providers")), Tokens(access, now() + expires * 1000, refresh)))
        }
    }

    private fun deviceJson() =
        """{"id":${Json.quote(device.id)},"platform":"android","version":${Json.quote(device.version.take(32))}}"""

    private fun call(method: String, path: String, body: String?, bearer: String?): Answer<Map<String, Any?>> {
        val reply = try {
            http.send(HttpRequest(method, Site.URL + path, body, bearer))
        } catch (e: IOException) {
            return Answer.Offline
        }
        @Suppress("UNCHECKED_CAST")
        val o = runCatching { Json.parse(reply.body) as? Map<String, Any?> }.getOrNull()
        return if (reply.status in 200..299 && o != null) Answer.Ok(o) else Answer.Refused(reply.status, o?.str("error"), reply.retryAfterS)
    }

    private fun enc(s: String) = URLEncoder.encode(s, "UTF-8")
}

/** The two ways to sign in, both through the site's web flow. */
enum class Provider(val word: String) { GOOGLE("google"), APPLE("apple") }

private inline fun <T, R> Answer<T>.then(f: (T) -> Answer<R>): Answer<R> = when (this) {
    is Answer.Ok -> f(value)
    is Answer.Offline -> this
    is Answer.Refused -> this
}

private inline fun <T, R> Answer<T>.map(f: (T) -> R): Answer<R> = then { Answer.Ok(f(it)) }
