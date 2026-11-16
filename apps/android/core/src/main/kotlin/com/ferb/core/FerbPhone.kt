package com.ferb.core

import android.content.Context
import android.os.Build
import android.provider.Settings
import android.util.Log
import com.ferb.mobile.Change
import com.ferb.mobile.Clock
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
import kotlinx.coroutines.launch
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

    private val linkHeard = object : LinkListener {
        override fun statusChanged(status: LinkStatus) {
            scope.launch { _link.value = status }
        }

        override fun persist(state: ByteArray) {
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
                _state.value = PhoneState.Open(vault.keys.health(), wiped)
            } catch (e: FerbException) {
                _state.value = PhoneState.Failed(e)
            }
            _state.value
        }
    }

    /** Runs [block] with the core on [io]; the library's error comes back typed. */
    suspend fun <T> call(block: (Mobile) -> T): Outcome<T> = withContext(io) {
        val m = mobile ?: return@withContext Outcome.Failed(FerbException.Closed("FERB is not open.", null))
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
     * now hold, such as a lock key after a screen lock was set), and the link restarts to carry them.
     */
    suspend fun pair(code: String): Outcome<Unit> = withContext(io) {
        serial.withLock {
            try {
                val fresh = vault.keys.renew()
                if (fresh != pubs) restartLink(fresh)
                val u = uplink ?: throw FerbException.Closed("FERB is not open.", null)
                u.pair(code)
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

    /** The app opened by the prompt alone (offline): the conversation is read again once the Mac admits it. */
    suspend fun unlocked(): Outcome<Unit> = call { it.unlocked() }

    /** "Stop everything on the Mac" (`stop.engage`): works while locked (PHN-125), never queued, no prompt. */
    suspend fun stopEverything(): Outcome<Unit> = call { it.stopEverything() }

    /**
     * Keys this phone holds that the Mac was never given: a fingerprint enrolled after a pairing made without one
     * makes the fingerprint-only key at the next start (or now: the missing keys the phone can hold are made here).
     * Null when there is nothing to register. Only keys the Mac lacks: a strong key Android destroyed is replaced only
     * by pairing again (PHN-133), so it never shows here.
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
        val kind = if (mac.strong != null && vault.keys.health().strong == KeyState.OK) KeyKind.STRONG else KeyKind.LOCK
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

    /** The last status the Mac pushed or a read kept (memory only; safe on the main thread), or null. */
    fun lastStatusNow(): com.ferb.mobile.PhoneStatus? = try {
        mobile?.lastStatus()
    } catch (e: FerbException) {
        null
    } catch (e: IllegalStateException) {
        null
    }

    /** A prompt signer whose key loss updates [state]. */
    fun signer(context: Context, kind: KeyKind, text: PromptText): PromptSigner =
        PromptSigner(context, vault.keys, kind, text, onGone = { publishOpen() })

    /**
     * "Forget this Mac" (PHN-408): the fetched chat and its key, the Mac in the link's state, and the three signing
     * keys go; the store and the link start again empty, ready to pair.
     */
    suspend fun forgetThisMac(): Outcome<Unit> = withContext(io) {
        serial.withLock {
            try {
                mobile?.let { m ->
                    runCatching { m.forgetThisMac() }
                    m.close()
                }
                mobile = null
                uplink?.let { u ->
                    runCatching { u.forget() } // the Mac-less state reaches persist before the link closes
                    u.shutdown()
                    u.close()
                }
                uplink = null
                vault.forget()
                wiped = null
                bringUp()
                publishOpen()
                Outcome.Ok(Unit)
            } catch (e: FerbException) {
                _state.value = PhoneState.Failed(e)
                Outcome.Failed(e)
            }
        }
    }

    /** The app came to the front or went to the back (Stay connected keeps it true). Non-blocking. */
    fun setActive(on: Boolean) {
        runCatching { uplink?.setActive(on) }
    }

    /** The phone's default network changed (`wifi`, `cellular`, …). Non-blocking. */
    fun networkChanged(kind: String?) {
        runCatching { uplink?.networkChanged(kind) }
    }

    fun retry() {
        runCatching { uplink?.retry() }
    }

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

    private fun publishOpen() {
        if (mobile != null && !closed.get()) _state.value = PhoneState.Open(vault.keys.health(), wiped)
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
        uplinks.start(UplinkStart(state, identity.name, identity.app, keys, linkHeard, deviceSigner))

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
        /** The phone's object: its vault, the real link, its names. Then [start]. */
        fun create(context: Context): FerbPhone =
            FerbPhone(Vault.forPhone(context.applicationContext), LinkUplinks(), PhoneIdentity.of(context),
                nativeInit = { NativeInit.once(context.applicationContext) })
    }
}
