package com.ferb.core

import android.content.Context
import android.os.Build
import android.provider.Settings
import android.util.Log
import com.ferb.mobile.Change
import com.ferb.mobile.Clock
import com.ferb.mobile.FingerprintUpdate
import com.ferb.mobile.FerbException
import com.ferb.mobile.Ids
import com.ferb.mobile.LinkListener
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.Listener
import com.ferb.mobile.Mobile
import com.ferb.mobile.MobileConfig
import com.ferb.mobile.Signer
import java.io.Closeable
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.TimeZone
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** Where the phone is. */
sealed interface PhoneState {
    data object Closed : PhoneState
    data object Starting : PhoneState

    /**
     * The store and the link run. [keys]: which signing keys work now ([KeyHealth.pairAgain]: the Mac can't be
     * unlocked from here any more). [wiped]: the kept pairing was given up at this start; the app pairs again.
     */
    data class Open(val keys: KeyHealth, val wiped: Lost? = null) : PhoneState

    /** The start failed with the library's error (rare: the store's folder can't be written, the link won't start). */
    data class Failed(val error: FerbException) : PhoneState
}

/** The names the Mac's phone list shows. */
data class PhoneIdentity(val name: String, val app: String) {
    companion object {
        /** The phone's own name (Settings → About), else its model; the app's id and version. */
        fun of(context: Context): PhoneIdentity {
            val name = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)?.takeIf { it.isNotBlank() }
                ?: "${Build.MANUFACTURER} ${Build.MODEL}"
            val version = runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull()
            return PhoneIdentity(name, "${context.packageName} ${version ?: "?"}")
        }
    }
}

/** The time as the engine writes it: `2026-11-16T09:30:00+05:30` (an offset, never `Z`). */
object SystemClockIso : Clock {
    private val format = DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HH:mm:ssxxx")
    override fun nowIso(): String = OffsetDateTime.now().truncatedTo(ChronoUnit.SECONDS).format(format)
}

object RandomIds : Ids {
    override fun newId(): String = UUID.randomUUID().toString()
}

/**
 * The phone's one object for the app (S11): the encrypted store (ferb-mobile `Mobile`) and the link (`Link`), started
 * from the [Vault], with the library's callbacks delivered as flows on [main]. One per process, owned by the
 * Application; [close] ends both and frees their native objects (the library then drops every callback it held, so
 * nothing of the app is kept alive by it). Every blocking call runs on [io]. kb PHN-446, PHN-493, PHN-498.
 */
class FerbPhone(
    private val vault: Vault,
    private val uplinks: UplinkFactory,
    private val identity: PhoneIdentity,
    private val clock: Clock = SystemClockIso,
    private val ids: Ids = RandomIds,
    private val main: CoroutineDispatcher = Dispatchers.Main,
    private val io: CoroutineDispatcher = Dispatchers.IO,
    private val utcOffsetS: () -> Int = { TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 1000 },
    /** Android only ([create]): hands the app's context to the library before the link starts ([NativeInit]). */
    private val nativeInit: () -> Unit = {},
) : Closeable {
    private val scope = CoroutineScope(SupervisorJob() + main)
    private val serial = Mutex()

    /**
     * What the app tells the link (in front or not, open or not, the network changed, dial now) goes to it one at a
     * time, in the order said, never on the caller's thread (kb PHN-682): the main thread never enters the library.
     */
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    private val linkLane = io.limitedParallelism(1)
    private val closed = AtomicBoolean(false)

    private val _state = MutableStateFlow<PhoneState>(PhoneState.Closed)
    val state: StateFlow<PhoneState> = _state.asStateFlow()

    private val _link = MutableStateFlow<LinkStatus?>(null)

    /** The link's status as it last said it (null before it started). Delivered on [main]. */
    val link: StateFlow<LinkStatus?> = _link.asStateFlow()

    private val _changes = MutableSharedFlow<Change>(extraBufferCapacity = 64, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    /** What changed in the core: read it again. Delivered on [main]. */
    val changes: SharedFlow<Change> = _changes.asSharedFlow()

    @Volatile private var mobile: Mobile? = null
    @Volatile private var uplink: Uplink? = null
    private var pubs: PublicKeys? = null
    private var wiped: Lost? = null

    private val heard = object : Listener {
        override fun changed(change: Change) {
            scope.launch { _changes.emit(change) }
        }
    }

    /** Each link started gets the next number; a link stopped since never writes the state again (PHN-646). */
    private val linkGen = java.util.concurrent.atomic.AtomicLong(0)

    private fun linkHeard(gen: Long) = object : LinkListener {
        override fun statusChanged(status: LinkStatus) {
            if (gen != linkGen.get()) return
            scope.launch { _link.value = status }
            // the Mac took the new fingerprint key: it is now the one the Mac holds
            if (status.fingerprintUpdate is FingerprintUpdate.Done) scope.launch(io) { fingerprintTaken() }
        }

        override fun persist(state: ByteArray) {
            // a link stopped by "Forget this Mac" (or restarted) has nothing more to say: its late save is dropped,
            // so a forgotten Mac's pairing and the old link identity never come back
            if (gen != linkGen.get()) { state.fill(0); return }
            // on the link's thread, at once: the state holds the phone's link key; never kept unsealed
            try {
                vault.saveLinkState(state)
            } catch (e: Exception) {
                Log.e(Keystore.TAG, "the link state could not be kept: ${e.javaClass.simpleName}")
            } finally {
                state.fill(0)
            }
        }
    }

    private val deviceSigner = DeviceKeySigner(vault.keys)

    /** Opens the store and starts the link (blocking work on [io]; the link's start takes up to 20 s). */
    suspend fun start(): PhoneState = withContext(io) {
        serial.withLock {
            if (closed.get()) return@withLock PhoneState.Closed
            if (mobile != null) return@withLock _state.value
            _state.value = PhoneState.Starting
            try {
                nativeInit()
                bringUp()
                _state.value = PhoneState.Open(health(), wiped)
            } catch (e: FerbException) {
                _state.value = PhoneState.Failed(e)
            }
            _state.value
        }
    }

    /** Runs [block] with the core on [io]; the library's error comes back typed. */
    suspend fun <T> call(block: (Mobile) -> T): Outcome<T> = withContext(io) {
        val m = mobile ?: return@withContext Outcome.Failed(FerbException.Closed("FERB is not open.", null))
        // kb PHN-682: every call into the library can wait (its store, its link); never from the main thread
        MainGuard.offMain("a call into the phone library")
        try {
            Outcome.Ok(block(m))
        } catch (e: FerbException) {
            Outcome.Failed(e)
        } catch (e: IllegalStateException) {
            // the native object was freed by close() while this call waited
            Outcome.Failed(FerbException.Closed("FERB was closed.", null))
        }
    }

    /**
     * Pairs from a scanned or typed code. Keys Android destroyed are made anew first (and missing ones the phone can
     * now hold, such as a lock key after a screen lock was set), and the link restarts to carry them (the same link
     * identity: the kept state, so [endpointId] still names it). [account]: the server's signed statement of this
     * phone's FERB account for [endpointId], which the Mac's daemon checks before it asks the owner (kb PHN-828).
     */
    suspend fun pair(code: String, account: String): Outcome<Unit> = withContext(io) {
        serial.withLock {
            try {
                val fresh = vault.keys.renew()
                if (fresh != pubs) restartLink(fresh)
                val u = uplink ?: throw FerbException.Closed("FERB is not open.", null)
                u.pair(code, account)
                // the keys this pairing's hello carries: what the Mac will hold (registerKeys adds any made later)
                vault.saveRegisteredKeys(fresh)
                wiped = null
                publishOpen()
                Outcome.Ok(Unit)
            } catch (e: FerbException) {
                Outcome.Failed(e)
            }
        }
    }

    /** The fingerprint-only key offered in the update under way (null: none), recorded as the Mac's once it says Done. */
    @Volatile private var offered: String? = null

    /**
     * Update fingerprint (PHN-133's replacement without pairing again): the fingerprint-only key Android destroyed when
     * the fingerprints changed is made anew (a usable one the Mac doesn't hold yet is offered as it is) and handed to the
     * paired Mac with [code], the code FERB on the Mac shows after Update fingerprint (scanned or typed). Returns once
     * the link took it; [link]'s `fingerprintUpdate` then says Waiting (with the six digits), Done or Failed. The phone
     * stays paired whatever the answer; on Done the key is recorded as the Mac's and [state] says the keys work again.
     * Refused here with [NO_FINGERPRINT] when this phone has no strong biometric enrolled.
     */
    suspend fun updateFingerprint(code: String): Outcome<Unit> = withContext(io) {
        serial.withLock {
            try {
                val u = uplink ?: throw FerbException.Closed("FERB is not open.", null)
                val pub = vault.keys.strongForUpdate()
                    ?: throw FerbException.NeedsSigner("This phone has no fingerprint set up.", NO_FINGERPRINT)
                offered = pub
                u.updateFingerprint(code, pub)
                Outcome.Ok(Unit)
            } catch (e: FerbException) {
                Outcome.Failed(e)
            } finally {
                publishOpen()
            }
        }
    }

    /** The Mac said Done: the key offered (when it is still this phone's) is the one the Mac holds now. */
    private suspend fun fingerprintTaken() = serial.withLock {
        val pub = offered ?: return@withLock
        val mac = vault.registeredKeys() ?: return@withLock
        if (vault.keys.strongPublic() == pub && mac.strong != pub) vault.saveRegisteredKeys(mac.copy(strong = pub))
        offered = null
        publishOpen()
    }

    /** This phone's link id (the account statement names it, kb PHN-828), or null before the link started. */
    suspend fun endpointId(): String? = withContext(io) {
        MainGuard.offMain("the link's endpoint id")
        uplink?.let { runCatching { it.endpointId() }.getOrNull() }?.takeIf { it.isNotEmpty() }
    }

    /**
     * PHN-125: the signed unlock the app sends on open. [signer] is a [PromptSigner] (one prompt: the fingerprint,
     * face or PIN, per the LOCK key) made with [signer]; the Mac's window is its own (five idle minutes).
     */
    suspend fun unlock(signer: Signer): Outcome<Unit> = call { it.unlock(signer) }.also { publishOpen() }

    /**
     * PHN-15: the app locked. The Mac is told (its unlock window for this phone ends now) and the conversation's watch
     * stops until the next [unlock] or [unlocked]; what waits still arrives as the Mac's status. Ok(false): offline (the
     * Mac's window ends by itself after its idle time).
     */
    suspend fun lock(): Outcome<Boolean> = call { it.lock() }

    /**
     * The owner's "Lock FERB now" (kb PHN-847): [lock], and the Mac is told it was by his hand, so with the app's lock
     * switch off it gives chat and tasks again only after the next signed unlock (PHN-848).
     */
    suspend fun lockByHand(): Outcome<Boolean> = call { it.lockByHand() }

    /** The app opened by the prompt alone (offline): the conversation is read again once the Mac admits it. */
    suspend fun unlocked(): Outcome<Unit> = call { it.unlocked() }

    /** "Stop everything on the Mac" (`stop.engage`): works while locked (PHN-125), never queued, no prompt. */
    suspend fun stopEverything(): Outcome<Unit> = call { it.stopEverything() }

    /**
     * Keys this phone holds that the Mac was never given: a fingerprint enrolled after a pairing made without one
     * makes the fingerprint-only key at the next start (or now: the missing keys the phone can hold are made here).
     * Null when there is nothing to register. Only keys the Mac lacks: a strong key Android destroyed is replaced by
     * [updateFingerprint] (or pairing again), so it never shows here.
     */
    suspend fun unregisteredKeys(): PublicKeys? = withContext(io) {
        serial.withLock {
            if (mobile == null || vault.registeredKeys() == null) return@withLock null
            val now = runCatching { vault.keys.ensure() }.getOrNull() ?: return@withLock null
            val mac = vault.registeredKeys() ?: return@withLock null
            val device = now.device?.takeIf { mac.device == null }
            val strong = now.strong?.takeIf { mac.strong == null }
            if (device == null && strong == null) null else PublicKeys(device, null, strong)
        }
    }

    /**
     * Registers [unregisteredKeys] with the Mac (`link.keys`), signed by the fingerprint-only key where the Mac
     * already holds one, else the lock key: [signer] makes the prompt for the kind asked. Ok(false): nothing to do.
     */
    suspend fun registerKeys(signer: (KeyKind) -> Signer): Outcome<Boolean> {
        val fresh = unregisteredKeys() ?: return Outcome.Ok(false)
        val mac = withContext(io) { vault.registeredKeys() } ?: return Outcome.Ok(false)
        val strongOk = withContext(io) { health().strong == KeyState.OK }
        val kind = if (mac.strong != null && strongOk) KeyKind.STRONG else KeyKind.LOCK
        val r = call { it.registerKeys(fresh.device, fresh.strong, signer(kind)) }
        if (r is Outcome.Ok) {
            withContext(io) { vault.saveRegisteredKeys(PublicKeys(mac.device ?: fresh.device, mac.lock, mac.strong ?: fresh.strong)) }
            publishOpen()
        }
        return when (r) {
            is Outcome.Ok -> Outcome.Ok(true)
            is Outcome.Failed -> r
        }
    }

    /**
     * The last status the Mac pushed or a read kept, or null. Like every call into the library it runs on [io]: on the
     * owner's phone the main thread asked for it itself and waited behind a sync, and the app froze (kb PHN-680).
     */
    suspend fun lastStatus(): com.ferb.mobile.PhoneStatus? = (call { it.lastStatus() } as? Outcome.Ok)?.value

    /** Runs [block] with the link on [linkLane]: in order, off the caller's thread, nothing thrown. */
    private fun tellLink(block: (Uplink) -> Unit) {
        scope.launch(linkLane) {
            val u = uplink ?: return@launch
            MainGuard.offMain("a call into the phone link")
            runCatching { block(u) }
        }
    }

    /** A prompt signer whose key loss updates [state]. */
    fun signer(context: Context, kind: KeyKind, text: PromptText): PromptSigner =
        PromptSigner(context, vault.keys, kind, text, onGone = { publishOpen() })

    /**
     * "Forget this Mac" (PHN-408): the fetched chat and its key, the Mac in the link's state, and the three signing
     * keys go; the store and the link start again empty, ready to pair.
     */
    suspend fun forgetThisMac(): Outcome<Boolean> = withContext(io) {
        serial.withLock {
            try {
                // PHN-646: first the Mac, when it can be reached and knows the call: it removes this phone (3 s at most)
                val told = tellMacToForget()
                // then everything here, whatever the Mac said: the link stops first (its late saves are dropped), the
                // store and its key, the link's identity and pairing, the signing keys; a new link with a new identity
                linkGen.incrementAndGet()
                uplink?.let { u ->
                    runCatching { u.shutdown() }
                    runCatching { u.close() }
                }
                uplink = null
                mobile?.let { m ->
                    runCatching { m.forgetThisMac() }
                    m.close()
                }
                mobile = null
                vault.forget()
                wiped = null
                bringUp()
                publishOpen()
                Outcome.Ok(told)
            } catch (e: FerbException) {
                _state.value = PhoneState.Failed(e)
                Outcome.Failed(e)
            }
        }
    }

    /** Asks the Mac to remove this phone (`link.forget`), bounded; true when it said it did. Never throws. */
    private suspend fun tellMacToForget(): Boolean {
        val m = mobile ?: return false
        val l = _link.value ?: return false
        if (!l.paired || l.phase != com.ferb.mobile.LinkPhase.ONLINE || l.macOlder) return false
        val asked = scope.async(io) { runCatching { m.forgetMe() }.isSuccess }
        return withTimeoutOrNull(FORGET_TELL_MS) { asked.await() } ?: false
    }

    /**
     * The app is unlocked and in front (true), or locked or left (false), kb PHN-648: the device key's presence keeps
     * the Mac's window open meanwhile. Non-blocking.
     */
    fun setOpen(on: Boolean) = tellLink { it.setOpen(on) }

    /** The unlock the lock key signed in the opening prompt, handed to the Mac once the link is up (kb PHN-649). */
    suspend fun unlockWithStatement(payload: String, sig: String, ageMs: Long): Outcome<Unit> =
        call { it.unlockWithStatement(payload, sig, ageMs.toULong()) }.also { publishOpen() }

    /**
     * This phone is paired and its lock key is there: the opening prompt can sign an unlock statement at once, before
     * the store and the link are up (kb PHN-649). Reads two small files and the Keystore; not on the main thread.
     */
    suspend fun statementReady(): Boolean = withContext(io) {
        runCatching { vault.hasLinkState() && vault.registeredKeys()?.lock != null && vault.keys.health().lock == KeyState.OK }
            .getOrDefault(false)
    }

    /** The app came to the front or went to the back (Stay connected keeps it true). Non-blocking. */
    fun setActive(on: Boolean) = tellLink { it.setActive(on) }

    /** The phone's default network changed (`wifi`, `cellular`, …). Non-blocking. */
    fun networkChanged(kind: String?) = tellLink { it.networkChanged(kind) }

    fun retry() = tellLink { it.retry() }

    /** Ends the store and the link and frees both (bounded: up to about 5 s for the link). Not on the main thread. */
    override fun close() {
        if (closed.getAndSet(true)) return
        val m = mobile
        val u = uplink
        mobile = null
        uplink = null
        m?.shutdown()
        u?.shutdown()
        m?.close()
        u?.close()
        _state.value = PhoneState.Closed
        scope.cancel()
    }

    // ---- inside -------------------------------------------------------------------------------------------------

    /**
     * The keys' health read again and published. Never on the calling thread: reading it is a Keystore round per key
     * (getKey and initSign), binder calls that can wait behind other Keystore work; after an unlock this ran on the
     * main thread (kb PHN-657).
     */
    private fun publishOpen() {
        scope.launch(io) {
            if (mobile != null && !closed.get()) _state.value = PhoneState.Open(health(), wiped)
        }
    }

    /**
     * The keys' health as the Mac sees them: the fingerprint-only key counts as GONE also when it isn't the one the Mac
     * holds (made for an update the owner didn't approve), so everything that needs it waits for Update fingerprint.
     */
    private fun health(): KeyHealth {
        val h = vault.keys.health()
        val mac = runCatching { vault.registeredKeys() }.getOrNull()?.strong ?: return h
        // a key the Mac doesn't hold, or none at all, can't answer the Mac's fingerprint checks
        val strong = if (h.strong != KeyState.GONE && vault.keys.strongPublic() != mac) KeyState.GONE else h.strong
        return h.copy(strong = strong, strongOnMac = true)
    }

    /** The link first (its kept state decides whether anything else is kept), then the store. */
    private fun bringUp() {
        var state = try {
            vault.linkState()
        } catch (e: VaultLost) {
            vault.wipe()
            wiped = e.why
            null
        }
        // a first start: keys left by an earlier install of this app on this phone are not reused
        if (state == null) vault.keys.deleteAll()
        var keys = vault.keys.ensure()
        val u = try {
            startLink(state, keys)
        } catch (e: FerbException.BadArgument) {
            vault.wipe()
            wiped = Lost.STATE_REFUSED
            state = null
            keys = vault.keys.ensure()
            startLink(null, keys)
        }
        state?.fill(0)
        uplink = u
        pubs = keys
        // a phone paired before the record of what the Mac holds existed: what it holds now is what it was given
        if (state != null && vault.registeredKeys() == null) runCatching { vault.saveRegisteredKeys(keys) }
        val m = openStore()
        mobile = m
        u.attach(m)
        _link.value = u.status()
    }

    private fun startLink(state: ByteArray?, keys: PublicKeys): Uplink =
        uplinks.start(UplinkStart(state, identity.name, identity.app, keys, linkHeard(linkGen.incrementAndGet()), deviceSigner))

    private fun restartLink(keys: PublicKeys) {
        uplink?.let {
            it.shutdown()
            it.close()
        }
        uplink = null
        val state = vault.linkState()
        val u = startLink(state, keys)
        state?.fill(0)
        uplink = u
        pubs = keys
        mobile?.let(u::attach)
        _link.value = u.status()
    }

    /** The store is a copy of the Mac's (ENG-10): one it can't open is dropped and made again, once. */
    private fun openStore(): Mobile {
        val config = MobileConfig(clientId = vault.clientId(), utcOffsetS = utcOffsetS())
        fun open(): Mobile {
            val key = vault.storeKey()
            try {
                return Mobile.open(vault.storeFile.path, key, config, clock, ids, heard)
            } finally {
                key.fill(0)
            }
        }
        return try {
            open()
        } catch (e: FerbException) {
            when (e) {
                is FerbException.WrongKey, is FerbException.BadKey, is FerbException.StoreNewer, is FerbException.Store -> {
                    Log.w(Keystore.TAG, "the store could not be opened (${e.code}); it is made again")
                    vault.dropStore()
                    open()
                }
                else -> throw e
            }
        }
    }

    companion object {
        /** How long "Forget this Mac" waits for the Mac to remove this phone before it wipes everything anyway. */
        const val FORGET_TELL_MS = 3_000L

        /** The app's own code: Update fingerprint needs a fingerprint enrolled on this phone first. */
        const val NO_FINGERPRINT = "no_fingerprint"
        /** The phone's object: its vault, the real link, its names. Then [start]. */
        fun create(context: Context): FerbPhone =
            FerbPhone(Vault.forPhone(context.applicationContext), LinkUplinks(), PhoneIdentity.of(context),
                nativeInit = { NativeInit.once(context.applicationContext) })
    }
}
