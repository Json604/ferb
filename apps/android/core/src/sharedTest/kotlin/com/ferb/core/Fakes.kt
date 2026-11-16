package com.ferb.core

import com.ferb.mobile.LinkStatus
import com.ferb.mobile.Mobile
import com.ferb.mobile.Transport
import com.ferb.mobile.TransportException
import com.ferb.mobile.WatchSink
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

// Shared by the JVM tests (the host library) and the instrumented ones (the phone's): invented data only.

const val ENTRY =
    """{"id":"m-1","conversation_id":"c1","pos":4294967296,"seq":1,"rev":1,"kind":"message","state":"done","open":false,"request_id":"r-1","client_id":null,"submission_id":null,"created_at":"2026-01-02T10:00:00+00:00","updated_at":"2026-01-02T10:00:00+00:00","day":"2026-01-02","body":{"text":"Invented question","attachments":[],"hidden":[]},"deleted":false}"""

/** A fake Mac (the S8 check's, journal 112): one conversation with one message; acknowledges a send. */
class FakeMac : Transport {
    val calls = CopyOnWriteArrayList<String>()
    val proofs = CopyOnWriteArrayList<String>()
    val sends = CopyOnWriteArrayList<String>()
    private val sinks = ConcurrentHashMap<ULong, WatchSink>()
    private val nextWatch = AtomicLong(0)

    override fun call(method: String, paramsJson: String, proofJson: String?): String {
        calls.add(method)
        proofJson?.let(proofs::add)
        return when (method) {
            "chat.entries.page" -> """{"conversation":"c1","epoch":"E1","head":1,"entries":[$ENTRY],"more":false,"before_pos":null}"""
            "chat.entries.changes" -> """{"conversation":"c1","epoch":"E1","head":1,"entries":[],"more":false,"next_seq":1}"""
            "link.challenge" -> """{"challenge":"C-${calls.size}"}"""
            "link.unlock", "task.stop", "chat.approve", "chat.entries.mark" -> """{"ok":true}"""
            else -> throw TransportException.Failed("method_not_allowed", "$method is not allowed", null, null)
        }
    }

    override fun send(paramsJson: String): String {
        sends.add(paramsJson)
        return """{"type":"ack","request_id":"r-${sends.size + 1}","entry_id":"pm-${sends.size + 1}","seq":${sends.size + 1},"epoch":"E1","submission_id":"sub-${sends.size}"}"""
    }

    override fun watch(paramsJson: String, sink: WatchSink): ULong {
        val id = nextWatch.incrementAndGet().toULong()
        sinks[id] = sink
        sink.frame("""{"type":"caught_up","conversation":"c1","epoch":"E1","head":1}""")
        return id
    }

    override fun closeWatch(watchId: ULong) {
        sinks.remove(watchId)
    }
}

/** The fake Mac as the core's way to the Mac, online on a direct path. */
class FakeUplink(val mac: FakeMac = FakeMac(), private val online: Boolean = true) : Uplink {
    val paired = CopyOnWriteArrayList<String>()
    val active = CopyOnWriteArrayList<Boolean>()
    var shutDown = 0
    var closed = 0
    override fun attach(mobile: Mobile) {
        mobile.connectTransport(mac)
        if (online) mobile.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
    }
    override fun pair(code: String) {
        paired.add(code)
    }
    override fun status(): LinkStatus? = null
    override fun setActive(on: Boolean) {
        active.add(on)
    }
    override fun networkChanged(kind: String?) {}
    override fun retry() {}
    override fun forget() {}
    override fun shutdown() {
        shutDown++
    }
    override fun close() {
        closed++
    }
}

/** Starts [FakeUplink]s, recording what each start was given; persists [persistOnStart] like a first link start. */
class FakeUplinks(private val persistOnStart: ByteArray? = "invented-link-state".toByteArray()) : UplinkFactory {
    val starts = CopyOnWriteArrayList<UplinkStart>()
    val made = CopyOnWriteArrayList<FakeUplink>()
    var refuseState = false

    /** Whether the next uplinks come online at once (online starts the core's own sync). */
    @Volatile var online = true
    override fun start(start: UplinkStart): Uplink {
        if (refuseState && start.state != null) throw com.ferb.mobile.FerbException.BadArgument("not the link's state", null)
        // a copy, as the real link copies it (the caller zeroes its own)
        starts.add(UplinkStart(start.state?.copyOf(), start.name, start.app, start.keys, start.listener, start.deviceSigner))
        if (start.state == null) persistOnStart?.let { start.listener.persist(it.copyOf()) }
        return FakeUplink(online = online).also(made::add)
    }
}

/** The Keystore's stand-in where there is none (the JVM): a key in this process only. */
class MemoryProtector : AesGcmProtector() {
    @Volatile private var key: SecretKey? = null
    override fun key(create: Boolean): SecretKey? {
        key?.let { return it }
        if (!create) return null
        return KeyGenerator.getInstance("AES").apply { init(256) }.generateKey().also { key = it }
    }
    override fun destroy() {
        key = null
    }
}

/** Software P-256 keys standing in for the Keystore's three (no prompt). */
class SoftwareKeys(var lockScreen: Boolean = true, var fingerprint: Boolean = true) : PhoneKeys {
    val pairs = ConcurrentHashMap<KeyKind, KeyPair>()
    val gone = ConcurrentHashMap.newKeySet<KeyKind>()
    val deletes = AtomicInteger(0)

    private fun make() = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()

    private fun pub(kind: KeyKind) = pairs[kind]?.takeIf { kind !in gone }?.public?.let(Proofs::publicKey)

    override fun ensure(): PublicKeys {
        if (!pairs.containsKey(KeyKind.DEVICE)) pairs[KeyKind.DEVICE] = make()
        if (!pairs.containsKey(KeyKind.LOCK) && lockScreen) pairs[KeyKind.LOCK] = make()
        if (!pairs.containsKey(KeyKind.STRONG) && fingerprint) pairs[KeyKind.STRONG] = make()
        return PublicKeys(pub(KeyKind.DEVICE), pub(KeyKind.LOCK), pub(KeyKind.STRONG))
    }

    override fun renew(): PublicKeys {
        gone.forEach { pairs.remove(it) }
        gone.clear()
        return ensure()
    }

    private fun state(kind: KeyKind) = when {
        kind in gone -> KeyState.GONE
        pairs.containsKey(kind) -> KeyState.OK
        else -> KeyState.NONE
    }

    override fun health() = KeyHealth(state(KeyKind.DEVICE), state(KeyKind.LOCK), state(KeyKind.STRONG))

    override fun deviceSign(payload: String): String? = sign(KeyKind.DEVICE, payload)

    fun sign(kind: KeyKind, payload: String): String? {
        val pair = pairs[kind]?.takeIf { kind !in gone } ?: return null
        val der = Signature.getInstance(Proofs.ALGORITHM).apply {
            initSign(pair.private)
            update(payload.toByteArray(Charsets.UTF_8))
        }.sign()
        return Proofs.signature(der)
    }

    override fun signatureFor(kind: KeyKind): Signature {
        val pair = pairs[kind]?.takeIf { kind !in gone } ?: throw KeyGone("no $kind key")
        return Signature.getInstance(Proofs.ALGORITHM).apply { initSign(pair.private) }
    }

    override fun deleteAll() {
        deletes.incrementAndGet()
        pairs.clear()
        gone.clear()
    }
}
