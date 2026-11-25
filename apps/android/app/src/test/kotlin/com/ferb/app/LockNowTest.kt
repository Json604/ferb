package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.AppLock
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Why
import com.ferb.app.ui.Host
import com.ferb.app.ui.Sheets
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.PhoneState
import com.ferb.design.Phone
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.MacInfo
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * "Lock FERB now" in This phone (kb PHN-847, PHN-848): the owner's tap locks the app at once by the five minutes' own
 * path (the lock page, the Mac told, the fetched files gone), and the app then opens only with the prompt, even with
 * "Fingerprint to open" off, until that prompt passes. The five minutes' lock is unchanged.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class LockNowTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private var t = 0L
    private val scheduler = TestCoroutineScheduler()
    private lateinit var session: Session
    private val signers = ArrayList<FakeSigner>()
    private var cancelPrompt = false
    private var localAsked = 0
    private val root: File = Files.createTempDirectory("ferb-lock-now").toFile()
    private val local = LocalFiles(root) { null }
    private val prefs get() = Prefs(ApplicationProvider.getApplicationContext())
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher(scheduler))
        session = Session { t }
        phone.state.value = PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm(): FerbViewModel =
        FerbViewModel(phone, session, prefs, { true }, null, SavedStateHandle(), local, wait = {},
            io = Dispatchers.Main, steady = { t }, account = testAccount()).also {
            it.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, cancelPrompt).also { s -> signers.add(s) }
                override suspend fun local(): Boolean { localAsked++; return !cancelPrompt }
            }
        }

    private fun shown() = signers.sumOf { it.asked } + localAsked

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    /** Work that hops to the IO threads (files on disk) comes back by itself: wait for it, bounded. */
    private fun until(what: String, cond: () -> Boolean) {
        val end = System.currentTimeMillis() + 5_000
        while (!cond() && System.currentTimeMillis() < end) { idle(); Thread.sleep(10) }
        idle()
        assertTrue("waited for $what", cond())
    }

    /** The app leaves for [ms] and comes back: paused, the process away, then in front, resumed and focused. */
    private fun away(vm: FerbViewModel, ms: Long) {
        vm.windowReady(false)
        session.foreground(false)
        t += ms
        session.foreground(true)
        vm.windowReady(true)
        idle()
    }

    /** The owner's lock switch off, as the Mac took it (PHN-846). */
    private fun lockOff() {
        prefs.lockOnOpen = false
        phone.lockOff = true
        phone.mac = MacInfo("Studio Mac", null, null, null, true)
    }

    // decision: PHN-847, PHN-848, PHN-15
    @Test
    fun the_button_locks_at_once_tells_the_mac_by_hand_and_the_fetched_files_go() {
        val scope = MainScope()
        tellMacOnLock(scope, session, phone, local)
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        vm.unlockApp()
        idle()
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        File(local.fetched, "k/a.txt").apply { parentFile!!.mkdirs(); writeText("x") }
        vm.show(Sheet.Profile)
        phone.calls.clear()
        val prompts = shown()
        vm.lockNow()
        // the same moment: the sheet gone, the lock page, nothing waiting on the Mac
        assertNull(vm.sheet.value)
        assertTrue(session.locked.value)
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
        until("the wipe") { !local.fetched.exists() }
        assertEquals("the Mac told once, by hand", listOf("lock:hand"), phone.calls.filter { it.startsWith("lock") })
        assertEquals("no prompt by itself: the lock page waits for his tap (PHN-616)", prompts, shown())
        // a second tap on a locked app does nothing more
        vm.lockNow()
        idle()
        assertEquals(1, phone.calls.count { it.startsWith("lock") })
        // his unlock opens it; the five minutes' lock after it is the plain one again
        vm.unlockApp()
        idle()
        assertFalse(session.locked.value)
        away(vm, AppLock.LOCK_AFTER_MS + 1)
        assertEquals(listOf("lock:hand", "lock"), phone.calls.filter { it.startsWith("lock") })
        scope.cancel()
    }

    // decision: PHN-847, PHN-846
    @Test
    fun with_the_switch_off_a_lock_by_hand_opens_only_with_the_prompt_until_it_passes() {
        lockOff()
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        idle()
        assertEquals("the switch off: open unasked", Gate.MAIN, vm.ui.value.gate)
        assertEquals(0, shown())
        vm.lockNow()
        idle()
        assertTrue(session.locked.value)
        assertEquals("the lock page, though the switch is off", Gate.LOCKED, vm.ui.value.gate)
        assertTrue(prefs.lockedByHand)
        assertFalse("the switch itself stays as he set it", vm.settings.value.lockOn)
        assertEquals(0, shown())
        // away and back: the prompt comes by itself; closed, it stays locked
        cancelPrompt = true
        away(vm, 1_000)
        assertEquals(1, shown())
        assertTrue(session.locked.value)
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
        // a new process (the phone closed the app): still locked, still the lock page
        session = Session { t }
        val vm2 = vm()
        assertEquals(Gate.LOCKED, vm2.ui.value.gate)
        session.foreground(true)
        vm2.windowReady(true)
        idle()
        assertTrue(session.locked.value)
        assertEquals(Gate.LOCKED, vm2.ui.value.gate)
        assertTrue(prefs.lockedByHand)
        // the prompt passes: open, and the one-shot is spent
        cancelPrompt = false
        vm2.unlockApp()
        idle()
        assertFalse(session.locked.value)
        assertEquals(Gate.MAIN, vm2.ui.value.gate)
        assertFalse(prefs.lockedByHand)
        // so the five minutes' lock opens unasked again, as the switch says
        val before = shown()
        away(vm2, AppLock.LOCK_AFTER_MS + 1)
        assertFalse(session.locked.value)
        assertEquals(Gate.MAIN, vm2.ui.value.gate)
        assertEquals("no prompt", before, shown())
    }

    // decision: PHN-847
    @Test
    fun a_cancelled_or_refused_prompt_keeps_the_one_shot() {
        lockOff()
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        idle()
        vm.lockNow()
        cancelPrompt = true
        vm.unlockApp()
        idle()
        assertTrue(session.locked.value)
        assertTrue(prefs.lockedByHand)
        vm.recheck()
        idle()
        assertTrue("recheck doesn't open it unasked", session.locked.value)
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
    }

    // decision: PHN-847
    @Test
    fun the_profile_page_s_lock_now_row_closes_into_the_lock_page() {
        val vm = vm()
        session.foreground(true)
        session.unlocked()
        vm.show(Sheet.Profile)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false,
                    sheet = { Sheets(vm, vm.ui.value, vm.sheet.value, PhoneInsets.Lab, host) }) {}
            }
        }
        compose.onNodeWithText("Opens again with your fingerprint").assertExists()
        compose.onNodeWithText("Lock FERB now").assertExists().performClick()
        compose.waitForIdle()
        assertNull(vm.sheet.value)
        assertTrue(session.locked.value)
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
    }
}
