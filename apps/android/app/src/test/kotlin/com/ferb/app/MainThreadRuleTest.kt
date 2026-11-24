package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LibraryPhone
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Why
import com.ferb.core.FakeMac
import com.ferb.core.FerbPhone
import com.ferb.core.KeyKind
import com.ferb.core.MainGuard
import com.ferb.core.MemoryProtector
import com.ferb.core.PhoneIdentity
import com.ferb.core.PhoneState
import com.ferb.core.SoftwareKeys
import com.ferb.core.Uplink
import com.ferb.core.UplinkFactory
import com.ferb.core.Vault
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.Mobile
import com.ferb.mobile.Signer
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * The rule the owner's freeze broke (kb PHN-680, PHN-682): the main thread never enters the phone library. The real
 * phone object over the real library (the host's build) and a fake Mac, with the app's own view model, notifier and
 * lock on the main thread: opening by the prompt, the statement to the Mac, the chat, a status the Mac changes (the
 * moment the phone froze: both the notifier and the view model then asked the library for the last status on the main
 * thread), a message, the link's own words, the lock. [MainGuard] stands at the library's door and names every call
 * that reaches it from the main thread; there must be none.
 */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class MainThreadRuleTest {
    private val onMain = CopyOnWriteArrayList<String>()
    private val root = Files.createTempDirectory("ferb-main-rule").toFile()
    private val scope = MainScope()

    @Before
    fun setUp() {
        MainGuard.onMain = { what ->
            val at = Throwable().stackTrace.filter { it.className.startsWith("com.ferb") && !it.className.contains("MainGuard") }.take(4)
                .joinToString(" < ") { "${it.className.substringAfterLast('.')}.${it.methodName}:${it.lineNumber}" }
            onMain += "$what at $at"
        }
    }

    @After
    fun tearDown() {
        MainGuard.onMain = null
        scope.cancel()
        root.deleteRecursively()
    }

    /** Lets the main thread's work run until [done]; the library's own work is on its threads meanwhile. */
    private fun until(what: String, ms: Long = 20_000, done: () -> Boolean) {
        val end = System.nanoTime() + ms * 1_000_000
        while (true) {
            shadowOf(Looper.getMainLooper()).idle()
            if (done()) break
            if (System.nanoTime() > end) fail("never happened: $what (calls into the library from the main thread so far: $onMain)")
            Thread.sleep(2)
        }
    }

    /** A link that is online on a direct path, with what the app told it, in order. */
    private class OnlineLink(private val mac: FakeMac) : UplinkFactory {
        val said = CopyOnWriteArrayList<String>()
        override fun start(start: com.ferb.core.UplinkStart): Uplink = object : Uplink {
            override fun attach(mobile: Mobile) {
                mobile.connectTransport(mac)
                mobile.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
            }
            override fun pair(code: String, account: String) {}
            override fun endpointId(): String = ""
            override fun status() = LinkStatus(LinkPhase.ONLINE, LinkPath.DIRECT, "a direct connection", null, true, true, false, null, false, null, null, null, false, false)
            override fun setActive(on: Boolean) { said += "active=$on" }
            override fun setOpen(on: Boolean) { said += "open=$on" }
            override fun networkChanged(kind: String?) { said += "network=$kind" }
            override fun retry() { said += "retry" }
            override fun forget() {}
            override fun shutdown() {}
            override fun close() {}
        }
    }

    // decision: PHN-680, PHN-682
    @Test
    fun opening_unlocking_a_changed_status_a_message_and_the_lock_never_enter_the_library_on_the_main_thread() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val keys = SoftwareKeys()
        val vault = Vault(java.io.File(root, "vault"), MemoryProtector(), keys)
        vault.saveLinkState("invented-link-state".toByteArray())
        vault.saveRegisteredKeys(keys.ensure())
        val mac = FakeMac()
        val link = OnlineLink(mac)
        val phone = FerbPhone(vault, link, PhoneIdentity("Invented phone", "test"))
        val api = LibraryPhone(phone)
        val session = Session { android.os.SystemClock.elapsedRealtime() }
        val prefs = Prefs(context)
        try {
            Notifier(context, api, prefs, session).start(scope)
            tellMacOnLock(scope, session, api, LocalFiles(java.io.File(root, "files")) { null })
            val vm = FerbViewModel(api, session, prefs, { true }, null, SavedStateHandle(), LocalFiles(java.io.File(root, "files")) { null }, account = testAccount())
            vm.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = object : Signer {
                    override fun sign(payload: String): String? = keys.sign(KeyKind.LOCK, payload)
                }
                override suspend fun local() = true
            }
            val started = java.util.concurrent.atomic.AtomicReference<Throwable?>(null)
            scope.launch { runCatching { phone.start() }.onFailure(started::set) }
            until("the phone opens") { started.get()?.let { throw AssertionError("the phone's start threw", it) }; phone.state.value is PhoneState.Open }

            // the app arrives in front, locked: the prompt by itself, the app opens, the statement goes to the Mac
            session.foreground(true)
            vm.windowReady(true)
            until("the app opens on the prompt") { !session.locked.value }
            until("the statement reaches the Mac") { "link.unlock_statement" in mac.calls }
            until("the chat is read") { vm.ui.value.rows.isNotEmpty() }

            // the Mac's status changes: the notifier and the view model both read the last status
            mac.status = """{"approvals":[],"running":[{"task_id":"t-1","name":"Invented task","goal":"g","started_at":null}],"results":[],"stopped":false}"""
            scope.launch { api.status() }
            until("the changed status reaches the screens") { vm.ui.value.status?.running?.isNotEmpty() == true }

            // a message, the link's own words in the order said, and the owner's lock (the Mac is told)
            vm.typed("hello")
            vm.send()
            until("the message goes") { mac.sends.isNotEmpty() }
            phone.setActive(false)
            phone.setActive(true)
            phone.networkChanged("wifi")
            phone.retry()
            until("the link heard the app") { link.said.containsAll(listOf("active=false", "active=true", "network=wifi", "retry")) }
            val said = link.said.filter { it.startsWith("active") || it.startsWith("network") || it == "retry" }
            assertEquals("the link hears the app in the order said", listOf("active=false", "active=true", "network=wifi", "retry"), said.takeLast(4))
            session.lockNow()
            until("the Mac is told of the lock") { "link.lock" in mac.calls }

            assertTrue("the main thread entered the phone library:\n" + onMain.joinToString("\n"), onMain.isEmpty())
        } finally {
            Thread { runCatching { phone.close() } }.apply { start(); join(15_000) }
        }
    }
}
