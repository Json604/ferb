package com.ferb.app

import com.ferb.app.model.Json
import com.ferb.mobile.Clock
import com.ferb.mobile.Ids
import com.ferb.mobile.Listener
import com.ferb.mobile.Mobile
import com.ferb.mobile.MobileConfig
import com.ferb.mobile.Transport
import com.ferb.mobile.TransportException
import com.ferb.mobile.TranscriptView
import com.ferb.mobile.WatchSink
import java.io.File
import java.nio.file.Files
import java.util.concurrent.atomic.AtomicInteger

/** One conversation of the engine's chat fixtures (engine/fixtures/chat-entries), as the Mac stores it. */
data class FixtureConversation(val file: String, val scenario: String, val conversation: String, val entries: List<Map<String, Any?>>)

object Fixtures {
    @Suppress("UNCHECKED_CAST")
    fun conversations(): List<FixtureConversation> = listOf("scenarios.json", "engine_chat.json", "engine_more.json").flatMap { file ->
        val text = Fixtures::class.java.classLoader!!.getResource(file)!!.readText()
        val root = Json.parse(text) as Map<String, Any?>
        (root["scenarios"] as List<Any?>).flatMap { s ->
            s as Map<String, Any?>
            val entries = (s["entries"] as List<Any?>).map { it as Map<String, Any?> }
            entries.groupBy { it["conversation_id"] as String }.map { (conv, es) -> FixtureConversation(file, s["name"] as String, conv, es) }
        }
    }

    /** JSON text from the reader's values (numbers that are whole are written as integers). */
    fun write(v: Any?): String = when (v) {
        null -> "null"
        is String -> buildString {
            append('"')
            for (c in v) when {
                c == '"' -> append("\\\""); c == '\\' -> append("\\\\"); c == '\n' -> append("\\n"); c == '\r' -> append("\\r")
                c == '\t' -> append("\\t"); c < ' ' -> append("\\u%04x".format(c.code)); else -> append(c)
            }
            append('"')
        }
        is Boolean -> v.toString()
        is Double -> if (v % 1.0 == 0.0 && Math.abs(v) < 9.0e15) v.toLong().toString() else v.toString()
        is Number -> v.toString()
        is Map<*, *> -> v.entries.joinToString(",", "{", "}") { (k, x) -> write(k as String) + ":" + write(x) }
        is List<*> -> v.joinToString(",", "[", "]") { write(it) }
        else -> error("not JSON: $v")
    }
}

/**
 * A Mac that serves one stored conversation by ENG-14's rules (pages newest first, changes after a cursor, the watch
 * caught up), as ferb-mobile's tests/common/mac.rs does, and answers questions and Stop. [refuse] makes a method
 * answer with a link code (the phone gate's refusals).
 */
class FixtureMac(private val c: FixtureConversation) : Transport {
    val calls = java.util.concurrent.CopyOnWriteArrayList<String>()
    val proofs = java.util.concurrent.CopyOnWriteArrayList<String?>()
    val refuse = java.util.concurrent.ConcurrentHashMap<String, String>()
    /** A method's answer as the Mac's daemon would give it, from its params (the coding sessions' calls, kb HAND-651..666). */
    val answers = java.util.concurrent.ConcurrentHashMap<String, (Map<String, Any?>) -> String>()
    /** Each call's params, in order (method to its JSON). */
    val params = java.util.concurrent.CopyOnWriteArrayList<Pair<String, String>>()
    private val byPos = c.entries.sortedBy { (it["pos"] as Double) }
    private val head = c.entries.maxOf { it["seq"] as Double }.toLong()

    override fun call(method: String, paramsJson: String, proofJson: String?): String {
        calls.add(method)
        proofs.add(proofJson)
        refuse[method]?.let { code -> throw TransportException.Failed(code, "refused by the test", null, null) }
        @Suppress("UNCHECKED_CAST")
        val p = Json.parse(paramsJson) as Map<String, Any?>
        params.add(method to paramsJson)
        answers[method]?.let { return it(p) }
        return when (method) {
            "chat.entries.page" -> {
                val before = (p["before_pos"] as? Double)
                val limit = (p["limit"] as? Double)?.toInt() ?: 50
                val all = byPos.filter { before == null || (it["pos"] as Double) < before }
                val from = maxOf(0, all.size - limit)
                val more = from > 0
                Fixtures.write(mapOf("conversation" to c.conversation, "epoch" to "E1", "head" to head.toDouble(), "entries" to all.subList(from, all.size),
                    "more" to more, "before_pos" to if (more) all[from]["pos"] else null))
            }
            "chat.entries.changes" -> {
                val after = (p["after_seq"] as? Double) ?: 0.0
                val es = c.entries.filter { (it["seq"] as Double) > after }.sortedBy { it["seq"] as Double }
                Fixtures.write(mapOf("conversation" to c.conversation, "epoch" to "E1", "head" to head.toDouble(), "entries" to es, "more" to false,
                    "next_seq" to (es.lastOrNull()?.get("seq") ?: after)))
            }
            "chat.entries.mark" -> """{"marks":{}}"""
            "link.challenge", "phone.challenge" -> """{"challenge":"C-${calls.size}"}"""
            "link.unlock", "task.stop", "chat.approve", "chat.answer", "chat.correct" -> """{"ok":true}"""
            else -> throw TransportException.Failed("method_not_allowed", "$method is not allowed", null, null)
        }
    }

    override fun send(paramsJson: String): String {
        val n = calls.size
        return """{"type":"ack","request_id":"r-$n","entry_id":"pm-$n","seq":${head + n},"epoch":"E1","submission_id":"sub-$n"}"""
    }

    override fun watch(paramsJson: String, sink: WatchSink): ULong {
        sink.frame("""{"type":"caught_up","conversation":"${c.conversation}","epoch":"E1","head":$head}""")
        return 1u
    }

    override fun closeWatch(watchId: ULong) {}

    override fun get(id: String, paramsJson: String, saveTo: String?, append: Boolean, progress: com.ferb.mobile.TransferProgress): String =
        throw TransportException.Failed("not_found", "The fixture Mac holds no files.", null, null)

    override fun put(id: String, path: String, optsJson: String, progress: com.ferb.mobile.TransferProgress): String =
        throw TransportException.Failed("not_found", "The fixture Mac takes no files.", null, null)

    override fun cancel(id: String) {}

    companion object {
        /** A phone core (the real library on the host) synced from [mac]: its transcript of the conversation. */
        fun transcript(mac: FixtureMac, utcOffsetS: Int = 0, use: (Mobile) -> Unit = {}): TranscriptView {
            val dir = Files.createTempDirectory("ferb-app-test").toFile()
            val n = AtomicInteger()
            val m = Mobile.open(File(dir, "phone.db").path, ByteArray(32) { 7 }, MobileConfig("client-test", utcOffsetS, null, null, null),
                object : Clock { override fun nowIso() = "2026-01-02T10:00:00+00:00" },
                object : Ids { override fun newId() = "id-${n.incrementAndGet()}" },
                object : Listener { override fun changed(change: com.ferb.mobile.Change) {} })
            try {
                m.connectTransport(mac)
                m.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
                m.sync()
                use(m)
                val conv = m.currentConversation() ?: error("no conversation after sync")
                return m.transcript(conv)
            } finally {
                m.shutdown()
                m.close()
                dir.deleteRecursively()
            }
        }
    }
}
