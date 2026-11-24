package com.ferb.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.ferb.app.model.LibraryPhone
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Why
import com.ferb.core.FerbPhone
import com.ferb.core.KeyKind
import com.ferb.core.MemoryProtector
import com.ferb.core.PhoneIdentity
import com.ferb.core.PhoneState
import com.ferb.core.SoftwareKeys
import com.ferb.core.Uplink
import com.ferb.core.UplinkFactory
import com.ferb.core.UplinkStart
import com.ferb.core.Vault
import com.ferb.mobile.LinkListener
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.Mobile
import com.ferb.mobile.Signer
import com.ferb.mobile.TransferProgress
import com.ferb.mobile.Transport
import com.ferb.mobile.TransportException
import com.ferb.mobile.WatchSink
import java.io.File
import java.time.LocalDate
import java.util.Random
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The owner's freeze, as a test on a running emulator or phone (kb PHN-680..PHN-683): the real phone library, the real
 * view model, notifier and lock on the real main thread, against a stand-in Mac on a link like his on 2026-11-18:
 * it moves between a direct path and the shared relay, drops for a few seconds and comes back, a new connection
 * answers `locked` until the phone has proved itself on it (the first unlock statement is refused), the Mac is slow
 * to answer (the read mark above all), and it pushes its status and live frames all the while. The owner opens, the
 * prompt passes, he reads, sends, locks, and opens again, for [STORM_S] seconds. A watchdog asks the main thread
 * every 20 ms; any stall over [STALL_MS] fails the test with where the main thread stood.
 */
@RunWith(AndroidJUnit4::class)
class MainThreadStormTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val main = Handler(Looper.getMainLooper())

    private fun <T> onMain(block: () -> T): T {
        var out: Result<T>? = null
        instrumentation.runOnMainSync { out = runCatching(block) }
        return out!!.getOrThrow()
    }

    // decision: PHN-680, PHN-681, PHN-682, PHN-683
    @Test
    fun the_main_thread_never_stalls_while_the_link_storms() {
        val seconds = InstrumentationRegistry.getArguments().getString("storm_s")?.toLongOrNull() ?: STORM_S
        val dir = File(context.cacheDir, "storm-${System.nanoTime()}")
        val keys = SoftwareKeys()
        val vault = Vault(dir, MemoryProtector(), keys)
        // a paired phone: the link's kept state and the keys its Mac holds (the opening prompt signs a statement)
        vault.saveLinkState("invented-link-state".toByteArray())
        vault.saveRegisteredKeys(keys.ensure())
        val mac = StormMac()
        val uplinks = StormUplinks(mac)
        val phone = FerbPhone(vault, uplinks, PhoneIdentity("Storm phone", "com.ferb.phone test"))
        val api = LibraryPhone(phone)
        val scope = MainScope()
        val prefs = Prefs(context)
        val session = Session { SystemClock.elapsedRealtime() }
        val watch = MainWatchdog(main, STALL_MS).also { it.start() }
        val storm = Storm(mac, uplinks)
        lateinit var vm: FerbViewModel
        try {
            onMain {
                Notifier(context, api, prefs, session).start(scope)
                tellMacOnLock(scope, session, api, LocalFiles.forPhone(context))
                vm = FerbViewModel(api, session, prefs, { true }, null, SavedStateHandle(), LocalFiles.forPhone(context), account = testAccount())
                vm.prompts = object : Prompts {
                    // the system prompt: the owner's finger takes a moment, then the lock key signs
                    override fun signer(kind: KeyKind, why: Why) = object : Signer {
                        override fun sign(payload: String): String? { Thread.sleep(300); return keys.sign(KeyKind.LOCK, payload) }
                    }
                    override suspend fun local(): Boolean = true
                }
            }
            assertTrue("the phone opens", runBlocking { phone.start() } is PhoneState.Open)
            storm.start()
            val until = SystemClock.elapsedRealtime() + seconds * 1000
            val random = Random(11)
            var opens = 0
            // the first open asks by itself (the app arrived in front, locked); the later ones are his tap
            onMain { session.foreground(true); vm.windowReady(true) }
            while (SystemClock.elapsedRealtime() < until) {
                val opened = waitFor(8_000) { !session.locked.value }
                if (!opened) fail("the app did not open within 8 s of the prompt (open ${opens + 1}): busy=${vm.ui.value.busy}${watch.report()}")
                opens++
                // open: he reads, sends, looks at Tasks, comes back, for a while
                val stay = SystemClock.elapsedRealtime() + 5_000 + random.nextInt(9_000)
                while (SystemClock.elapsedRealtime() < minOf(stay, until)) {
                    onMain {
                        when (random.nextInt(5)) {
                            0 -> vm.refresh()
                            1 -> { vm.typed("storm ${random.nextInt(1000)}"); vm.send() }
                            2 -> vm.loadTasks()
                            3 -> { session.foreground(false); session.foreground(true) }
                            else -> vm.recheck()
                        }
                    }
                    Thread.sleep(150L + random.nextInt(600))
                }
                // his own lock; then his tap on the lock page
                onMain { session.lockNow() }
                Thread.sleep(400L + random.nextInt(800))
                if (SystemClock.elapsedRealtime() < until) onMain { vm.unlockApp() }
            }
            val stalls = watch.stalls()
            val counts = mac.counts()
            android.util.Log.i("ferb-storm", "opens=$opens stalls=${stalls.size} longest=${watch.longest()} ms starved=${watch.starved()} calls=$counts drops=${storm.drops.get()}")
            assertTrue("the storm ran: $counts", (counts["chat.entries.mark"] ?: 0) >= 2 && (counts["link.unlock_statement"] ?: 0) >= 2 && storm.drops.get() >= 2)
            assertTrue("the chat arrived: ${vm.ui.value.rows.size} rows", vm.ui.value.rows.size >= 100)
            if (stalls.isNotEmpty()) fail("the main thread stalled ${stalls.size} time(s) over $STALL_MS ms in $seconds s of the storm ($opens opens):\n" + stalls.joinToString("\n"))
        } finally {
            storm.stop()
            watch.stop()
            scope.cancel()
            Thread { runCatching { phone.close() } }.apply { start(); join(15_000) }
            dir.deleteRecursively()
        }
    }

    private fun waitFor(ms: Long, what: () -> Boolean): Boolean {
        val until = SystemClock.elapsedRealtime() + ms
        while (SystemClock.elapsedRealtime() < until) {
            if (what()) return true
            Thread.sleep(25)
        }
        return what()
    }

    companion object {
        /** How long the storm runs unless the run says otherwise (`-e storm_s N`). */
        const val STORM_S = 150L

        /** The longest the main thread may go without answering. */
        const val STALL_MS = 250L
    }
}

/**
 * Asks the main thread to answer every 20 ms and keeps each time it didn't for [stallMs], with where it stood. A
 * moment in which this thread itself was not run (the whole process starved by the machine) is counted apart, never
 * as the main thread's stall.
 */
class MainWatchdog(private val main: Handler, private val stallMs: Long) {
    private val answered = AtomicLong(SystemClock.uptimeMillis())
    private val found = CopyOnWriteArrayList<String>()
    private val longest = AtomicLong(0)
    private val starved = AtomicInteger(0)
    @Volatile private var running = true
    private val beat = object : Runnable {
        override fun run() {
            answered.set(SystemClock.uptimeMillis())
            if (running) main.postDelayed(this, 20)
        }
    }

    fun start() {
        main.post(beat)
        Thread({
            var open = false
            var worst = 0L
            var stack = ""
            var last = SystemClock.uptimeMillis()
            var starvedNow = false
            while (running) {
                runCatching { Thread.sleep(20) }
                val now = SystemClock.uptimeMillis()
                // this thread asked for 20 ms and got far more: nothing of the process ran, so nothing is known
                if (now - last > 150) { starvedNow = true; starved.incrementAndGet() }
                last = now
                val stuck = now - answered.get()
                if (stuck >= stallMs && !starvedNow) {
                    if (!open) {
                        open = true
                        val frames = Looper.getMainLooper().thread.stackTrace
                        val ours = frames.filter { it.className.startsWith("com.ferb") }.take(8)
                        stack = (frames.take(4) + ours).distinct().joinToString(" < ") { "${it.className.substringAfterLast('.')}.${it.methodName}:${it.lineNumber}" }
                    }
                    worst = maxOf(worst, stuck)
                } else if (stuck < stallMs) {
                    if (open) {
                        found.add("$worst ms at $stack")
                        longest.accumulateAndGet(worst) { a, b -> maxOf(a, b) }
                    }
                    open = false; worst = 0; starvedNow = false
                }
            }
            if (open) { found.add("$worst ms (still stalled at the end) at $stack"); longest.accumulateAndGet(worst) { a, b -> maxOf(a, b) } }
        }, "storm-main-watch").apply { isDaemon = true }.start()
    }

    fun stop() { running = false; Thread.sleep(60) }
    fun stalls(): List<String> = found.toList()
    fun longest(): Long = longest.get()
    fun starved(): Int = starved.get()
    fun report(): String = if (found.isEmpty()) "" else "\nmain thread stalls so far:\n" + found.joinToString("\n")
}

/**
 * The stand-in Mac: one conversation of [StormMac.STORED] messages from yesterday and none today, served by ENG-14's
 * rules; a new connection is `locked` for private calls and the unlock statement until the phone has proved itself on
 * it ([proved], set a moment after each connection, as `phone.present` arrives after the first calls); slow answers.
 */
class StormMac : Transport {
    @Volatile var online = false
    @Volatile var proved = false
    @Volatile var window = false
    private val random = Random(7)
    private val day = LocalDate.now().minusDays(1).toString()
    private val entries = CopyOnWriteArrayList<Pair<Long, String>>()
    private val head = AtomicLong(0)
    private val counts = ConcurrentHashMap<String, AtomicInteger>()
    val sinks = ConcurrentHashMap<ULong, WatchSink>()
    private val nextWatch = AtomicLong(0)

    init {
        repeat(STORED) { add("Stored message ${it + 1} of yesterday, with a few more words than one.", day) }
    }

    fun counts(): Map<String, Int> = counts.mapValues { it.value.get() }.toSortedMap()

    private fun entry(seq: Long, text: String, day: String) =
        """{"id":"e-$seq","conversation_id":"c1","pos":${(1L shl 32) + seq},"seq":$seq,"rev":1,"kind":"message","state":"done","open":false,"request_id":"r-$seq","client_id":null,"submission_id":null,"created_at":"${day}T10:00:00+00:00","updated_at":"${day}T10:00:00+00:00","day":"$day","body":{"text":"$text","attachments":[],"hidden":[]},"deleted":false}"""

    /** One more committed message; the JSON a watch frame carries. */
    fun add(text: String, day: String = LocalDate.now().toString()): String {
        val seq = head.incrementAndGet()
        return entry(seq, text, day).also { entries.add(seq to it) }
    }

    private fun number(json: String, key: String): Long? =
        Regex("\"$key\":(-?\\d+)").find(json)?.groupValues?.get(1)?.toLongOrNull()

    private fun refused(code: String, words: String): Nothing = throw TransportException.Failed(code, words, null, null)

    private fun slow(method: String) {
        val ms = when (method) {
            // the call the sync waits on last: slow above all (the owner's link, mid-move between paths)
            "chat.entries.mark" -> 1_500L + random.nextInt(2_500)
            "link.unlock_statement", "link.unlock", "link.challenge" -> 150L + random.nextInt(900)
            else -> 20L + random.nextInt(250)
        }
        val until = SystemClock.elapsedRealtime() + ms
        while (SystemClock.elapsedRealtime() < until) {
            if (!online) refused("mac_unreachable", "FERB's Mac did not answer.")
            Thread.sleep(20)
        }
    }

    override fun call(method: String, paramsJson: String, proofJson: String?): String {
        counts.getOrPut(method) { AtomicInteger() }.incrementAndGet()
        if (!online) refused("mac_unreachable", "FERB's Mac can't be reached.")
        slow(method)
        val private = method in setOf("chat.entries.page", "chat.entries.changes", "chat.entries.get", "tasks.list", "phone.list", "phone.recent", "settings.get")
        if (private && !(window && proved)) refused("locked", "Unlock FERB on your phone first.")
        return when (method) {
            "link.unlock_statement" -> {
                // a connection that hasn't proved the phone's device key yet: locked, try again in a moment (PHN-649)
                if (!proved) refused("locked", "Unlock FERB on your phone first.")
                window = true
                """{"unlocked":true}"""
            }
            "link.unlock" -> { window = true; """{"unlocked":true}""" }
            "link.lock" -> { window = false; """{"unlocked":false}""" }
            "link.challenge" -> """{"challenge":"C-${random.nextInt(1_000_000)}"}"""
            "chat.entries.page" -> {
                val before = number(paramsJson, "before_pos")
                val limit = (number(paramsJson, "limit") ?: 50).toInt()
                val all = entries.filter { before == null || (1L shl 32) + it.first < before }
                val from = maxOf(0, all.size - limit)
                val more = from > 0
                """{"conversation":"c1","epoch":"E1","head":${head.get()},"entries":[${all.subList(from, all.size).joinToString(",") { it.second }}],"more":$more,"before_pos":${if (more) ((1L shl 32) + all[from].first).toString() else "null"}}"""
            }
            "chat.entries.changes" -> {
                val after = number(paramsJson, "after_seq") ?: 0
                val limit = (number(paramsJson, "limit") ?: 50).toInt()
                val all = entries.filter { it.first > after }
                val page = all.take(limit)
                """{"conversation":"c1","epoch":"E1","head":${head.get()},"entries":[${page.joinToString(",") { it.second }}],"more":${all.size > page.size},"next_seq":${page.lastOrNull()?.first ?: after}}"""
            }
            "chat.entries.mark" -> """{"marks":{}}"""
            "chat.status" -> status(0)
            "tasks.list" -> """{"tasks":[],"more":false}"""
            "phone.list" -> """{"phones":[{"id":"p1","name":"Storm phone","paired_at":"2026-11-10T09:00:00+05:30","this":true}],"mac":{"name":"Storm Mac"}}"""
            "phone.device_report", "task.stop", "stop.engage", "link.keys" -> """{"ok":true}"""
            "screen_use.status" -> """{"active":false,"can_offer":true}"""
            else -> refused("method_not_allowed", "$method is not allowed")
        }
    }

    /** The Mac's `chat.status`, different for each [n] (what runs changes), as its pushes are. */
    fun status(n: Int): String =
        """{"approvals":[],"running":[{"task_id":"t-1","name":"Storm task $n","goal":"A task that runs","started_at":null}],"results":[],"stopped":false}"""

    override fun send(paramsJson: String): String {
        counts.getOrPut("chat.send") { AtomicInteger() }.incrementAndGet()
        if (!online) refused("mac_unreachable", "FERB's Mac can't be reached.")
        slow("chat.send")
        if (!(window && proved)) refused("locked", "Unlock FERB on your phone first.")
        val n = head.get()
        return """{"type":"ack","request_id":"r-s$n","entry_id":"pm-$n","seq":$n,"epoch":"E1","submission_id":"sub-$n"}"""
    }

    override fun watch(paramsJson: String, sink: WatchSink): ULong {
        counts.getOrPut("chat.entries.watch") { AtomicInteger() }.incrementAndGet()
        if (!online) refused("mac_unreachable", "FERB's Mac can't be reached.")
        if (!(window && proved)) refused("locked", "Unlock FERB on your phone first.")
        val id = nextWatch.incrementAndGet().toULong()
        sinks[id] = sink
        return id
    }

    override fun closeWatch(watchId: ULong) { sinks.remove(watchId) }
    override fun get(id: String, paramsJson: String, saveTo: String?, append: Boolean, progress: TransferProgress): String = refused("not_found", "The storm Mac holds no files.")
    override fun put(id: String, path: String, optsJson: String, progress: TransferProgress): String = refused("not_found", "The storm Mac takes no files.")
    override fun cancel(id: String) {}

    companion object {
        const val STORED = 300
    }
}

/** The stand-in link: what [Storm] says the link's state is, told to the core and to the app as the real link does. */
class StormUplinks(private val mac: StormMac) : UplinkFactory {
    @Volatile var mobile: Mobile? = null
    @Volatile var listener: LinkListener? = null
    @Volatile private var now = status(LinkPhase.CONNECTING, LinkPath.OFFLINE)

    private fun status(phase: LinkPhase, path: LinkPath) = LinkStatus(
        phase = phase, path = path,
        tierWords = when (path) { LinkPath.DIRECT -> "a direct connection"; LinkPath.PUBLIC_RELAY -> "the shared relay"; else -> null },
        pairingCode = null, paired = true, bulkAllowed = path == LinkPath.DIRECT, slow = false, ownRelayUrl = null,
        ownRelayAlways = false, relay = null, problem = null, retryInS = if (phase == LinkPhase.WAITING) 2uL else null,
        macOlder = false, macNewer = false,
    )

    /** The link's state changed: the core hears its event, the app its status (each on the link's own thread). */
    fun say(phase: LinkPhase, path: LinkPath) {
        now = status(phase, path)
        val tier = when (path) { LinkPath.DIRECT -> "direct"; LinkPath.PUBLIC_RELAY -> "shared_relay"; else -> null }
        val event = if (phase == LinkPhase.ONLINE) """{"type":"state","phase":"online","tier":"$tier"}""" else """{"type":"state","phase":"${if (phase == LinkPhase.WAITING) "waiting" else "connecting"}"}"""
        runCatching { mobile?.linkEvent(event) }
        if (phase == LinkPhase.ONLINE) runCatching { mobile?.linkEvent("""{"type":"bulk","allowed":${path == LinkPath.DIRECT},"tier":"$tier"}""") }
        listener?.statusChanged(now)
    }

    override fun start(start: UplinkStart): Uplink {
        listener = start.listener
        return object : Uplink {
            override fun attach(mobile: Mobile) {
                mobile.connectTransport(mac)
                this@StormUplinks.mobile = mobile
            }
            override fun pair(code: String, account: String) {}
            override fun endpointId(): String = ""
            override fun status(): LinkStatus? = now
            override fun setActive(on: Boolean) {}
            override fun networkChanged(kind: String?) {}
            override fun retry() {}
            override fun forget() {}
            override fun shutdown() {}
            override fun close() {}
        }
    }
}

/**
 * The owner's link on 2026-11-18, quicker: online on a direct path, moved to the shared relay, dropped for one to
 * three seconds, back; each new connection proved 600 ms after it is up. Meanwhile the Mac pushes its status and live
 * frames several times a second and commits a message now and then.
 */
class Storm(private val mac: StormMac, private val link: StormUplinks) {
    @Volatile private var running = false
    val drops = AtomicInteger(0)
    private val random = Random(5)
    private val threads = ArrayList<Thread>()

    private fun nap(ms: Long) { val until = SystemClock.elapsedRealtime() + ms; while (running && SystemClock.elapsedRealtime() < until) Thread.sleep(20) }

    fun start() {
        running = true
        threads += Thread({
            while (running) {
                mac.online = true
                link.say(LinkPhase.ONLINE, if (random.nextBoolean()) LinkPath.DIRECT else LinkPath.PUBLIC_RELAY)
                nap(600)
                mac.proved = true
                nap(2_000L + random.nextInt(4_000))
                link.say(LinkPhase.ONLINE, LinkPath.PUBLIC_RELAY)
                nap(500L + random.nextInt(2_500))
                link.say(LinkPhase.ONLINE, LinkPath.DIRECT)
                nap(1_000L + random.nextInt(3_000))
                // the drop: calls under way fail, every watch ends, the phone redials
                mac.online = false
                mac.proved = false
                drops.incrementAndGet()
                mac.sinks.clear()
                link.say(LinkPhase.CONNECTING, LinkPath.OFFLINE)
                nap(500L + random.nextInt(1_000))
                link.say(LinkPhase.WAITING, LinkPath.OFFLINE)
                nap(500L + random.nextInt(2_000))
            }
        }, "storm-link").also { it.isDaemon = true; it.start() }
        threads += Thread({
            var n = 0
            while (running) {
                nap(120L + random.nextInt(300))
                if (!mac.online) continue
                n++
                // the bridge's pushed status (allowed while locked), a live frame, and now and then a new message
                runCatching { link.mobile?.linkEvent("""{"type":"status","status":${mac.status(n)}}""") }
                for (s in mac.sinks.values) runCatching { s.frame("""{"type":"live","request_id":"r-live","frame":{"type":"status","text":"Working on it ($n)"}}""") }
                if (n % 15 == 0 && mac.window && mac.proved) {
                    val e = mac.add("A message that arrived in the storm ($n).")
                    for (s in mac.sinks.values) runCatching { s.frame("""{"type":"change","entry":$e}""") }
                }
            }
        }, "storm-pushes").also { it.isDaemon = true; it.start() }
    }

    fun stop() {
        running = false
        threads.forEach { it.join(3_000) }
    }
}
