package com.ferb.app.model

import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.core.KeyGone
import com.ferb.core.Protector
import com.ferb.core.SealedFiles
import com.ferb.core.Unreadable
import java.io.File
import java.net.URI
import java.net.URLDecoder
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** A sign-in begun in the browser: its state and PKCE verifier. Not a data class: the verifier is never logged. */
class PendingSignIn(val state: String, val verifier: String, val atMs: Long) {
    override fun toString() = "PendingSignIn(at $atMs)"
}

/** Where the account and its tokens are kept: only sealed (kb PHN-827), never a file in the clear, a log or prefs. */
interface AccountStore {
    fun load(): SignedIn?
    fun save(s: SignedIn)
    fun clear()
    fun loadPending(): PendingSignIn?
    fun savePending(p: PendingSignIn)
    fun clearPending()
}

/**
 * [AccountStore] in two small files of [dir] (on the phone `noBackupFilesDir/account`: app-private and outside every
 * backup), each AES-256-GCM-sealed by [protector] (on the phone a non-exportable Android Keystore key of its own,
 * `ferb.key.account`, so a pairing wiped for a lost link key keeps the account): `account` for the account and tokens,
 * `signin` for a sign-in under way (the browser may outlive this process). A file that can't be opened counts as none.
 */
class SealedAccountStore(private val dir: File, private val protector: Protector) : AccountStore {
    private val accountFile get() = File(dir, "account")
    private val pendingFile get() = File(dir, "signin")

    override fun load(): SignedIn? = open(accountFile, ACCOUNT)?.let(::readAccount)
    override fun save(s: SignedIn) = SealedFiles.write(accountFile, protector.seal(writeAccount(s).toByteArray(), ACCOUNT))
    override fun clear() { accountFile.delete(); pendingFile.delete() }

    override fun loadPending(): PendingSignIn? = open(pendingFile, PENDING)?.let { o ->
        val state = o.str("state") ?: return null
        val verifier = o.str("verifier") ?: return null
        PendingSignIn(state, verifier, (o["at"] as? Double)?.toLong() ?: return null)
    }

    override fun savePending(p: PendingSignIn) = SealedFiles.write(pendingFile, protector.seal(
        """{"state":${Json.quote(p.state)},"verifier":${Json.quote(p.verifier)},"at":${p.atMs}}""".toByteArray(), PENDING))

    override fun clearPending() { pendingFile.delete() }

    @Suppress("UNCHECKED_CAST")
    private fun open(file: File, purpose: String): Map<String, Any?>? {
        val sealed = SealedFiles.read(file) ?: return null
        val plain = try {
            protector.open(sealed, purpose)
        } catch (_: KeyGone) {
            return null
        } catch (_: Unreadable) {
            return null
        }
        return try {
            Json.parse(plain.toString(Charsets.UTF_8)) as? Map<String, Any?>
        } catch (_: Exception) {
            null
        } finally {
            plain.fill(0)
        }
    }

    private fun readAccount(o: Map<String, Any?>): SignedIn? {
        val a = o.obj("account") ?: return null
        val info = AccountInfo(a.str("id") ?: return null, a.str("email") ?: return null, a.str("name"), a.strings("providers"))
        val t = Tokens(o.str("access") ?: return null, (o["access_expires_at"] as? Double)?.toLong() ?: 0L, o.str("refresh") ?: return null)
        return SignedIn(info, t)
    }

    private fun writeAccount(s: SignedIn): String {
        val a = s.account
        val providers = a.providers.joinToString(",", "[", "]") { Json.quote(it) }
        return """{"v":1,"account":{"id":${Json.quote(a.id)},"email":${Json.quote(a.email)},"name":${a.name?.let { Json.quote(it) } ?: "null"},""" +
            """"providers":$providers},"access":${Json.quote(s.tokens.access)},"access_expires_at":${s.tokens.accessExpiresAtMs},""" +
            """"refresh":${Json.quote(s.tokens.refresh)}}"""
    }

    private companion object {
        const val ACCOUNT = "account"
        const val PENDING = "signin"
    }
}

/** Where a sign-in stands on the sign-in page. */
enum class SignInStep { IDLE, BROWSER, FINISHING }

/**
 * The account as the screens see it. [known]: the store was read (until then the app shows its start). [account]:
 * null when signed out. [note]: the sign-in page's line. [checkFailed]: the last check of the sign-in couldn't reach
 * the server (FERB keeps working, the account page says so).
 */
data class AccountUi(
    val known: Boolean = false,
    val account: AccountInfo? = null,
    val step: SignInStep = SignInStep.IDLE,
    val note: Note? = null,
    val checkFailed: Boolean = false,
)

/** The browser's return to `https://ferb.kartikey.xyz/app/callback`: exactly that address, with a state. */
class SignInCallback(val state: String, val code: String?, val error: String?) {
    override fun toString() = "SignInCallback(error=$error)"

    companion object {
        fun parse(url: String): SignInCallback? {
            val u = runCatching { URI(url) }.getOrNull() ?: return null
            val site = URI(Site.CALLBACK)
            if (u.scheme != "https" || u.host != site.host || u.port != -1 || u.path != site.path || u.rawFragment != null) return null
            val q = (u.rawQuery ?: return null).split('&').mapNotNull { kv ->
                val p = kv.split('=', limit = 2)
                if (p.size != 2) null else runCatching { URLDecoder.decode(p[0], "UTF-8") to URLDecoder.decode(p[1], "UTF-8") }.getOrNull()
            }.toMap()
            return SignInCallback(q["state"]?.takeIf { it.isNotEmpty() } ?: return null, q["code"]?.takeIf { it.isNotEmpty() }, q["error"])
        }
    }
}

/**
 * FERB's account on this phone (kb PHN-827): the sign-in through the site's web flow (PKCE S256 and a state of its
 * own; the browser's return is used only when its state is this sign-in's), the tokens kept sealed in [store], the
 * access token refreshed when it has under a minute left, one refresh at a time. A refresh that can't reach the server
 * keeps the person signed in (never a lockout); the server's `invalid_grant` signs out. One per process (the
 * Application's); every blocking step runs on [io].
 */
class AccountModel(
    private val store: AccountStore,
    private val server: AccountServer,
    private val io: CoroutineDispatcher,
    private val now: () -> Long,
    private val random: SecureRandom = SecureRandom(),
) {
    private val _state = MutableStateFlow(AccountUi())
    val state: StateFlow<AccountUi> = _state.asStateFlow()

    @Volatile private var saved: SignedIn? = null
    @Volatile private var pending: PendingSignIn? = null
    private val refreshing = Mutex()
    private val returning = Mutex()

    private val _ended = kotlinx.coroutines.flow.MutableSharedFlow<Unit>(extraBufferCapacity = 1)

    /**
     * The server ended this sign-in (`invalid_grant` on a refresh, or a 401 after a fresh token): a sign-out (kb
     * PHN-828), so the app unpairs as Sign out does. The account here is already gone when this fires.
     */
    val ended: kotlinx.coroutines.flow.SharedFlow<Unit> = _ended

    /** Reads the kept account (once, at the start). Signed out when there is none or it can't be opened. */
    suspend fun load() {
        val s = withContext(io) { runCatching { store.load() }.getOrNull() }
        saved = s
        _state.update { it.copy(known = true, account = s?.account) }
    }

    /** Starts a sign-in: the address to open in the browser. The verifier is kept sealed while the browser has it. */
    suspend fun begin(provider: Provider): String = withContext(io) {
        val verifier = b64(ByteArray(32).also(random::nextBytes))
        val state = b64(ByteArray(16).also(random::nextBytes))
        val challenge = b64(MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.US_ASCII)))
        val p = PendingSignIn(state, verifier, now())
        pending = p
        runCatching { store.savePending(p) }
        _state.update { it.copy(step = SignInStep.BROWSER, note = null) }
        server.startUrl(provider, state, challenge)
    }

    /** The person came back from the browser without its answer (closed it): the buttons work again. */
    fun browserLeft() {
        _state.update { if (it.step == SignInStep.BROWSER) it.copy(step = SignInStep.IDLE) else it }
    }

    /**
     * The browser's return. Ignored (false) unless it is the callback address and carries this sign-in's state; then
     * used once: an error is said, a code is exchanged for the tokens.
     */
    suspend fun returned(url: String): Boolean = returning.withLock {
        val back = SignInCallback.parse(url) ?: return false
        val p = pending ?: withContext(io) { runCatching { store.loadPending() }.getOrNull() } ?: return false
        if (now() - p.atMs > PENDING_MS || !MessageDigest.isEqual(back.state.toByteArray(), p.state.toByteArray())) return false
        pending = null
        val code = back.code?.takeIf { back.error == null }
        // at once, before anything waits: the page shows the sign-in finishing, never its buttons again meanwhile
        if (code != null) _state.update { it.copy(step = SignInStep.FINISHING, note = null) }
        withContext(io) { runCatching { store.clearPending() } }
        when {
            back.error == "cancelled" -> said(Note(R.string.signin_cancelled))
            back.error == "no_email" -> said(Note(R.string.signin_no_email))
            code == null -> said(Note(R.string.signin_failed))
            else -> {
                when (val r = withContext(io) { server.exchange(code, p.verifier) }) {
                    is Answer.Ok -> keep(r.value)
                    Answer.Offline -> said(Note(R.string.signin_unreachable))
                    is Answer.Refused -> said(Note(if (r.status == 429 || r.status >= 500) R.string.signin_not_answering else R.string.signin_failed))
                }
            }
        }
        true
    }

    /** The access token, refreshed first when under a minute is left. [force]: the server refused it; refresh now. */
    suspend fun access(force: Boolean = false): Answer<String> = refreshing.withLock {
        val s = saved ?: return Answer.Refused(401, SIGNED_OUT)
        if (!force && s.tokens.accessExpiresAtMs - now() > REFRESH_BEFORE_MS) return Answer.Ok(s.tokens.access)
        when (val r = withContext(io) { server.refresh(s.tokens.refresh) }) {
            is Answer.Ok -> {
                keep(r.value)
                Answer.Ok(r.value.tokens.access)
            }
            Answer.Offline -> { _state.update { it.copy(checkFailed = true) }; Answer.Offline }
            is Answer.Refused -> if (r.status == 401 && r.error == "invalid_grant") {
                endedByServer()
                Answer.Refused(401, SIGNED_OUT)
            } else {
                _state.update { it.copy(checkFailed = true) }
                r
            }
        }
    }

    /** At each open: the sign-in is still good (a refresh when due; the server's no signs out). */
    suspend fun check() { if (saved != null) access() }

    /** The server's signed statement of this account for the phone whose link id is [endpoint] (kb PHN-828). */
    suspend fun assertion(endpoint: String): Answer<String> = authed { server.assertion(it, endpoint) }

    /**
     * Signs out (kb PHN-843): the server is told first, so this sign-in's tokens are revoked there (bounded by the
     * HTTP waits), then what is kept here goes whatever it answered, so nobody is ever left signed in. When the server
     * couldn't be told, the sign-in page says so.
     */
    suspend fun signOut() {
        val s = saved
        val told = s == null || withContext(io) { runCatching { server.signOut(s.tokens) }.getOrNull() } is Answer.Ok
        clearLocal()
        if (!told) _state.update { it.copy(note = Note(R.string.signout_unreachable)) }
    }

    /** Deletes the account on the server; only when that succeeded, everything here goes too. */
    suspend fun delete(): Boolean = when (authed { server.delete(it) }) {
        is Answer.Ok -> { clearLocal(); true }
        else -> false
    }

    /** Signed in as this account now (its id; never compared by e-mail). */
    val accountId: String? get() = saved?.account?.id

    private suspend fun <T> authed(call: (String) -> Answer<T>): Answer<T> {
        var force = false
        repeat(2) {
            val token = when (val a = access(force)) {
                is Answer.Ok -> a.value
                Answer.Offline -> return Answer.Offline
                is Answer.Refused -> return a
            }
            val r = withContext(io) { call(token) }
            // the server no longer takes this access token (expired early, revoked): refresh once and try again
            if (r !is Answer.Refused || r.status != 401) return r
            force = true
        }
        // a fresh token refused too: the server ended this sign-in
        endedByServer()
        return Answer.Refused(401, SIGNED_OUT)
    }

    private suspend fun keep(s: SignedIn) {
        withContext(io) { runCatching { store.save(s) } }
        saved = s
        _state.update { it.copy(known = true, account = s.account, step = SignInStep.IDLE, note = null, checkFailed = false) }
    }

    private suspend fun endedByServer() {
        clearLocal()
        _ended.tryEmit(Unit)
    }

    private suspend fun clearLocal() {
        saved = null
        pending = null
        withContext(io) { runCatching { store.clear() } }
        _state.value = AccountUi(known = true)
    }

    private fun said(n: Note) = _state.update { it.copy(step = SignInStep.IDLE, note = n) }

    companion object {
        /** A sign-in in the browser is good for ten minutes (the server's own cookie lasts as long). */
        const val PENDING_MS = 10 * 60_000L

        /** The access token is refreshed when less than this is left. */
        const val REFRESH_BEFORE_MS = 60_000L

        /** [access]'s refusal when there is no account (or the server ended it). */
        const val SIGNED_OUT = "signed_out"

        fun b64(bytes: ByteArray): String = Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)

        /** The phone's install id for the server: random, not secret (kept in the app's plain preferences). */
        fun newDeviceId(random: SecureRandom = SecureRandom()): String = b64(ByteArray(16).also(random::nextBytes))
    }
}
