package com.ferb.core

import com.ferb.mobile.LinkStatus
import com.ferb.mobile.Mobile
import com.ferb.mobile.TransferProgress
import com.ferb.mobile.Transport
import com.ferb.mobile.TransportException
import com.ferb.mobile.WatchSink
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
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

    /** Folders by path: `link.files`' reply entries (name, dir, size, locked). [keyPlaces] want a fresh proof. */
    val folders = ConcurrentHashMap<String, List<FakeEntry>>()
    val keyPlaces = ConcurrentHashMap.newKeySet<String>()

    /** Files by path, as bytes; a fetch sends them in [chunk]-byte pieces, waiting at [gate] after the first piece. */
    val files = ConcurrentHashMap<String, ByteArray>()
    @Volatile var chunk = 4
    @Volatile var gate: CountDownLatch? = null

    /** What arrived per upload id, and the chat.status / settings / tasks / phone.list replies. */
    val uploads = ConcurrentHashMap<String, ByteArray>()
    val gets = CopyOnWriteArrayList<String>()
    val cancelled = ConcurrentHashMap.newKeySet<String>()
    @Volatile var status = """{"approvals":[],"running":[],"results":[],"stopped":false}"""
    val settings = ConcurrentHashMap<String, String>()
    @Volatile var tasks = """{"tasks":[],"more":false}"""
    @Volatile var macName = "MacBook Pro"
    @Volatile var frames: List<String> = emptyList()
    private val frameAt = AtomicInteger(0)
    val settingCalls = CopyOnWriteArrayList<String>()

    /** T-1214, T-1211: what `phone.device_report` brought (its params as sent), `link.tree`'s folders (path → files by
     * their path inside), a finished task's files (`taskId#index` → bytes), and the recent and search replies. */
    val reports = CopyOnWriteArrayList<String>()
    val trees = ConcurrentHashMap<String, Map<String, ByteArray>>()
    @Volatile var treeSkipped = 0
    val taskFiles = ConcurrentHashMap<String, ByteArray>()
    val groups = CopyOnWriteArrayList<String>()
    @Volatile var recent = """{"files":[],"skipped":0}"""
    @Volatile var search = """{"path":"~","query":"","files":[],"looked":0,"skipped":0,"cut":false,"incomplete":false}"""
    val searches = CopyOnWriteArrayList<String>()
    val puts = CopyOnWriteArrayList<String>()

    override fun call(method: String, paramsJson: String, proofJson: String?): String {
        calls.add(method)
        proofJson?.let(proofs::add)
        return when (method) {
            "chat.entries.page" -> """{"conversation":"c1","epoch":"E1","head":1,"entries":[$ENTRY],"more":false,"before_pos":null}"""
            "chat.entries.changes" -> """{"conversation":"c1","epoch":"E1","head":1,"entries":[],"more":false,"next_seq":1}"""
            "link.challenge" -> """{"challenge":"C-${calls.size}"}"""
            "link.unlock", "link.unlock_statement", "task.stop", "chat.approve", "chat.entries.mark", "link.lock", "stop.engage", "link.keys" -> """{"ok":true}"""
            "chat.status" -> status
            "link.files" -> {
                val path = field(paramsJson, "path")
                if (path in keyPlaces && proofJson == null) throw TransportException.Failed("fresh_unlock_required", "This place asks for your fingerprint each time.", null, null)
                val es = folders[path] ?: throw TransportException.Failed("not_found", "No such folder.", null, null)
                val parent = path.substringBeforeLast('/', "").ifEmpty { null }
                es.joinToString(",", """{"path":"$path","parent":${parent?.let { "\"$it\"" } ?: "null"},"high_value":${path in keyPlaces},"entries":[""", """],"total":${es.size},"cut":false}""") {
                    """{"name":"${it.name}","dir":${it.dir},"link":false,"size":${it.size},"modified":1767348000,"locked":${it.locked}}"""
                }
            }
            "settings.get" -> settings.entries.joinToString(",", """{"values":{""", "}}") { (k, v) -> "\"$k\":$v" }
            "settings.set" -> {
                settingCalls.add(paramsJson)
                val key = field(paramsJson, "key")
                val value = paramsJson.substringAfter("\"value\":").removeSuffix("}")
                settings[key] = value
                """{"key":"$key","value":$value}"""
            }
            "tasks.list" -> tasks
            "phone.device_report" -> { reports.add(paramsJson); """{"ok":true}""" }
            "phone.recent" -> recent
            "phone.search" -> { searches.add(paramsJson); search }
            "link.put_group" -> { groups.add(field(paramsJson, "name")); """{"group":"g-${groups.size}","saved":"~/Downloads/FERB from phone/${field(paramsJson, "name")}"}""" }
            "link.tree" -> {
                val path = field(paramsJson, "path")
                val fs = trees[path] ?: throw TransportException.Failed("not_found", "No such folder.", null, null)
                fs.entries.joinToString(",", """{"path":"$path","name":"${path.substringAfterLast('/')}","files":[""", """],"bytes":${fs.values.sumOf { it.size }},"skipped":$treeSkipped,"cut":false}""") { (rel, b) ->
                    """{"rel":"$rel","size":${b.size}}"""
                }
            }
            "phone.list" -> """{"phones":[{"id":"p1","name":"Invented phone","paired_at":"2026-11-10T09:00:00+05:30","this":true}],"mac":{"name":"$macName"}}"""
            else -> throw TransportException.Failed("method_not_allowed", "$method is not allowed", null, null)
        }
    }

    /** A string member of a flat JSON object (enough for the params the core writes). */
    private fun field(json: String, key: String): String = json.substringAfter("\"$key\":\"").substringBefore('"')

    override fun get(id: String, paramsJson: String, saveTo: String?, append: Boolean, progress: TransferProgress): String {
        gets.add(paramsJson)
        cancelled.remove(id)
        if (paramsJson.contains("\"what\":\"preview\"")) {
            val i = frameAt.getAndIncrement()
            return frames.getOrElse(i) { frames.lastOrNull() ?: """{"meta":{"state":"done"},"size":0,"received":0}""" }
        }
        // a finished task's own file (`what: file`): the Mac finds it from the task's record, never from a path
        val path = if (paramsJson.contains("\"what\":\"file\"")) {
            val task = field(paramsJson, "task_id")
            val index = paramsJson.substringAfter("\"index\":").takeWhile { it.isDigit() }
            "task:$task#$index".also { k -> taskFiles["$task#$index"]?.let { files[k] = it } }
        } else field(paramsJson, "path")
        if (path in keyPlaces && !paramsJson.contains("\"proof\"")) throw TransportException.Failed("fresh_unlock_required", "This file asks for your fingerprint each time.", null, null)
        val bytes = files[path] ?: trees.entries.firstNotNullOfOrNull { (dir, fs) -> fs[path.removePrefix("$dir/")]?.takeIf { path.startsWith("$dir/") } }
            ?: throw TransportException.Failed("not_found", "No such file.", null, null)
        val offset = paramsJson.substringAfter("\"offset\":").takeWhile { it.isDigit() }.toIntOrNull() ?: 0
        val out = File(saveTo!!)
        if (!append) out.writeBytes(ByteArray(0))
        var at = offset
        var first = true
        while (at < bytes.size) {
            if (id in cancelled) throw TransportException.Failed("cancelled", "Stopped on the phone.", null, null)
            val n = minOf(chunk, bytes.size - at)
            out.appendBytes(bytes.copyOfRange(at, at + n))
            at += n
            progress.progress(at.toULong(), bytes.size.toULong())
            if (first) {
                first = false
                gate?.await(5, TimeUnit.SECONDS)
            }
        }
        return """{"meta":{"name":"${path.substringAfterLast('/')}","offset":$offset,"total":${bytes.size}},"received":${bytes.size - offset},"saved_to":"$saveTo"}"""
    }

    override fun put(id: String, path: String, optsJson: String, progress: TransferProgress): String {
        puts.add(optsJson)
        cancelled.remove(id)
        val resume = optsJson.substringAfter("\"resume\":", "null").let { if (it.startsWith("\"")) it.drop(1).substringBefore('"') else null }
        val uid = resume ?: "up-${uploads.size + 1}"
        val have = uploads[uid] ?: ByteArray(0)
        progress.accepted(uid, have.size.toULong())
        val bytes = File(path).readBytes()
        var at = have.size
        var first = true
        while (at < bytes.size) {
            if (id in cancelled) throw TransportException.Failed("cancelled", "Stopped on the phone.", null, null)
            val n = minOf(chunk, bytes.size - at)
            uploads[uid] = (uploads[uid] ?: ByteArray(0)) + bytes.copyOfRange(at, at + n)
            at += n
            progress.progress(at.toULong(), bytes.size.toULong())
            if (first) {
                first = false
                gate?.await(5, TimeUnit.SECONDS)
            }
        }
        return """{"upload_id":"$uid","saved":"~/Downloads/FERB/${field(optsJson, "name")}","size":${bytes.size},"sha256":"x"}"""
    }

    override fun cancel(id: String) {
        cancelled.add(id)
        gate?.countDown()
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

/** One entry of a fake folder. */
data class FakeEntry(val name: String, val dir: Boolean = false, val size: Long = 0, val locked: Boolean = false)

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
