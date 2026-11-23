package com.ferb.app

import android.Manifest
import android.app.Application
import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import android.graphics.Bitmap
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.CameraWatch
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.MacAsleep
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import com.ferb.core.Outcome
import com.ferb.mobile.CameraFrame
import com.ferb.mobile.Change
import com.ferb.mobile.FerbException
import com.ferb.mobile.Insomnia
import com.ferb.mobile.InsomniaLastEnd
import com.ferb.mobile.InsomniaNotice
import com.ferb.mobile.InsomniaWarning
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.OlderState
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.TranscriptView
import java.nio.file.Files
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * Insomnia mode on the phone (APP-980..APP-984): the Mac tab's switch opens the warning and Keep it working is signed
 * with the fingerprint; End, Lock my Mac and It's fine ask nothing; the notice and the warnings as notifications, each
 * posted once and removed when the Mac's status no longer has them; how it ended said once; the camera view pulls only
 * while it shows, always out of screenshots, and says the library's words on the shared relay.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class InsomniaTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val phone = FakePhone()
    private lateinit var session: Session
    private val prefs = Prefs(context)
    private val signers = ArrayList<Pair<FakeSigner, Why>>()
    private var cancelPrompt = false
    private val root = Files.createTempDirectory("ferb-insomnia").toFile()
    private val nm = context.getSystemService(NotificationManager::class.java)
    private val scopes = ArrayList<CoroutineScope>()

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { 0L }
        session.unlocked()
        prefs.forgetPairing()
        phone.view = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)
        phone.status = status(phone.insomnia)
    }

    @After
    fun tearDown() {
        scopes.forEach { it.cancel() }
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun status(i: Insomnia?) = PhoneStatus(false, true, false, emptyList(), emptyList(), emptyList(), null, i)

    private fun vm() = FerbViewModel(phone, session, prefs, { true }, null, SavedStateHandle(), LocalFiles(root) { null }).also {
        it.prompts = object : Prompts {
            override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, cancelPrompt).also { s -> signers += s to why }
            override suspend fun local() = true
        }
    }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    private fun on(notice: InsomniaNotice? = null, warnings: List<InsomniaWarning> = emptyList()) =
        phone.insomnia.copy(on = true, variant = "shield", line = "Keeping your Mac working · 2 tasks running · battery 64%",
            notice = notice, warnings = warnings)

    private val touched = InsomniaNotice("n-1", "touched", "2026-11-20T15:10:00+05:30", "Someone is using your Mac's keyboard or trackpad.", null, 1u)
    private val battery = InsomniaWarning("w-1", "battery_low", "Your Mac's battery is at 18%.", null)

    // ---- the switch, the warning and the signed start --------------------------------------------------------------

    // decision: APP-980, APP-984, PHN-112, PHN-133, PHN-774
    @Test
    fun the_switch_opens_the_warning_and_keep_it_working_is_signed_with_the_fingerprint() {
        val vm = vm()
        assertNotNull("the Mac says it can: the row shows", vm.ui.value.status?.insomnia)
        vm.askInsomnia()
        assertEquals(Sheet.Insomnia, vm.sheet.value)
        assertFalse("nothing is turned on before Keep it working", phone.calls.any { it.startsWith("insomnia-start") })
        // the owner cancels the fingerprint: nothing is sent, the warning stays
        cancelPrompt = true
        vm.startInsomnia("shield", screenYes = true)
        assertTrue(phone.signed.none { it.startsWith("sig-") })
        assertEquals(Sheet.Insomnia, vm.sheet.value)
        cancelPrompt = false
        vm.startInsomnia("shield", screenYes = true)
        assertTrue("insomnia-start:shield:true" in phone.calls)
        assertEquals("the fingerprint-only key, for this change", KeyKind.STRONG to Why.INSOMNIA, signers.last().let { it.first.kind to it.second })
        assertEquals(listOf("sig-STRONG"), phone.signed)
        assertNull("the sheet goes once the Mac took it", vm.sheet.value)
        assertTrue(vm.ui.value.status!!.insomnia!!.on)
        // the locked way never carries the one Yes
        phone.insomnia = phone.insomnia.copy(on = false)
        vm.startInsomnia("locked", screenYes = false)
        assertTrue("insomnia-start:locked:false" in phone.calls)
    }

    // decision: APP-980, APP-983
    @Test
    fun end_lock_my_mac_and_its_fine_ask_nothing() {
        phone.insomnia = on(touched)
        phone.status = status(phone.insomnia)
        val vm = vm()
        vm.insomniaFine("n-1")
        assertTrue("insomnia-fine:n-1" in phone.calls)
        assertNull(vm.ui.value.status!!.insomnia!!.notice)
        vm.lockMac()
        assertTrue("lock-mac" in phone.calls)
        assertEquals(Note(R.string.err_mac_said, "Your Mac is locked."), vm.ui.value.note)
        vm.endInsomnia()
        assertTrue("insomnia-end" in phone.calls)
        assertFalse(vm.ui.value.status!!.insomnia!!.on)
        assertTrue("no prompt for any of them", signers.isEmpty())
    }

    /** How it ended is the Mac's alert on every device (kb APP-1108, APP-1109): the app adds no note of its own. */
    // decision: APP-980, APP-1109
    @Test
    fun how_it_ended_is_not_said_again_by_the_app() {
        session.foreground(true)
        val vm = vm()
        val ended = phone.insomnia.copy(on = false, lastEnd = InsomniaLastEnd("e-1", "back", "Welcome back. FERB finished 2 tasks.", null))
        phone.status = status(ended)
        phone.changes.tryEmit(Change.Status)
        idle()
        assertNull("the Mac's alert says it; a note would say it twice", vm.ui.value.note)
    }

    // ---- the notifications -------------------------------------------------------------------------------------------

    private fun started(): Notifier = Notifier(context, phone, prefs, session, wait = {}).also { n -> MainScope().also { scopes += it; n.start(it) } }

    private fun macSays(i: Insomnia?) {
        phone.status = status(i)
        phone.changes.tryEmit(Change.Status)
        idle()
    }

    private val mine: List<Notification>
        get() = nm.activeNotifications.filter { it.tag == Notifier.TAG_INSOMNIA }.map { it.notification }

    private fun Notification.text() = extras.getCharSequence(Notification.EXTRA_TEXT).toString()
    private fun Notification.title() = extras.getCharSequence(Notification.EXTRA_TITLE).toString()
    private val Notification.quiet: Boolean get() = flags and Notification.FLAG_ONLY_ALERT_ONCE != 0 && group == Notifier.SILENT_GROUP

    // decision: APP-983, APP-981, PHN-776
    @Test
    fun the_notice_is_posted_once_with_its_three_actions_and_goes_when_answered() {
        shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        session.foreground(false)
        started()
        macSays(on(touched))
        val n = mine.single()
        assertEquals(context.getString(R.string.n_insomnia_touched), n.title())
        assertEquals(touched.text, n.text())
        assertEquals(Notifier.CH_INSOMNIA, n.channelId)
        assertFalse("a new notice alerts", n.quiet)
        assertEquals(listOf("Show me", "Lock my Mac", "It's fine"), n.actions.map { it.title.toString() })
        // Show me opens the app on the camera view; the other two are answered from the notification
        assertTrue(shadowOf(n.actions[0].actionIntent).isActivityIntent)
        assertTrue(shadowOf(n.actions[0].actionIntent).savedIntent.getBooleanExtra(MainActivity.EXTRA_CAMERA, false))
        val lock = shadowOf(n.actions[1].actionIntent)
        assertTrue(lock.isBroadcastIntent)
        assertEquals(InsomniaReceiver.LOCK, lock.savedIntent.action)
        assertEquals("n-1", lock.savedIntent.getStringExtra(InsomniaReceiver.NOTICE))
        assertEquals(InsomniaReceiver.FINE, shadowOf(n.actions[2].actionIntent).savedIntent.action)
        assertNull("no picture in a notification", n.getLargeIcon())
        // the same status again: posted once
        nm.cancelAll()
        macSays(on(touched))
        assertTrue("not posted again", mine.isEmpty())
        macSays(on(touched.copy(count = 2u)))
        assertTrue("a later touch updates it quietly", mine.single().quiet)
        // answered (here or on the Mac): gone
        macSays(on(null))
        assertTrue(mine.isEmpty())
        // an app FERB didn't open
        macSays(on(InsomniaNotice("n-2", "activation", null, "Terminal opened on your Mac.", "Terminal", 1u)))
        assertEquals(context.getString(R.string.n_insomnia_activation), mine.single().title())
    }

    /** Someone at his Mac is news wherever he is: the notice is posted with the app on screen too. */
    // decision: APP-983
    @Test
    fun the_notice_is_posted_while_the_app_is_on_screen_too() {
        shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        session.foreground(true)
        started()
        macSays(on(touched))
        assertEquals(1, mine.size)
    }

    // decision: APP-981
    @Test
    fun each_warning_is_posted_once_without_actions_and_goes_when_gone_or_the_mode_ends() {
        shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        session.foreground(false)
        started()
        val hot = InsomniaWarning("w-2", "hot", "Your Mac is hot.", null)
        macSays(on(warnings = listOf(battery, hot)))
        assertEquals(2, mine.size)
        for (w in mine) {
            assertEquals(context.getString(R.string.n_insomnia_warning), w.title())
            assertTrue("no actions: a tap opens the app", w.actions.isNullOrEmpty())
        }
        assertEquals(setOf(battery.text, hot.text), mine.map { it.text() }.toSet())
        nm.cancelAll()
        macSays(on(warnings = listOf(battery, hot)))
        assertTrue("posted once", mine.isEmpty())
        macSays(on(touched, listOf(battery)))
        macSays(on(warnings = listOf(battery)))
        // the mode ends: what it posted goes, and how it ended is said once, quietly
        val ended = phone.insomnia.copy(on = false, lastEnd = InsomniaLastEnd("e-9", "back", "Welcome back.", null))
        macSays(ended)
        assertTrue(mine.isEmpty())
        val end = nm.activeNotifications.single { it.tag == Notifier.TAG_INSOMNIA_END }.notification
        assertEquals("Welcome back.", end.text())
        assertTrue(end.quiet)
        nm.cancelAll()
        macSays(ended)
        assertTrue("said once", nm.activeNotifications.isEmpty())
        // turned off by the owner himself: nothing to tell him (kb APP-1108)
        macSays(phone.insomnia.copy(on = false, lastEnd = InsomniaLastEnd("e-10", "turned_off", "Done. Your Mac can sleep as usual again.", null)))
        assertTrue("his own turning off is not told", nm.activeNotifications.none { it.tag == Notifier.TAG_INSOMNIA_END })
        // an older Mac (no insomnia) leaves nothing behind either
        macSays(on(touched))
        macSays(null)
        assertTrue(mine.isEmpty())
    }

    // ---- the camera view ---------------------------------------------------------------------------------------------

    // decision: APP-983, PHN-454, PHN-775, PHN-810
    @Test
    fun the_camera_pulls_only_while_its_view_shows_and_follows_the_screenshot_switch() {
        session.foreground(true)
        val vm = vm()
        vm.showCamera()
        assertEquals(Sheet.Camera, vm.sheet.value)
        assertFalse("the camera follows the owner's switch (PHN-810)", vm.mustSecure.value)
        assertEquals(1, phone.calls.count { it == "camera" })
        // it leaves: the pulling stops
        vm.show(null)
        assertEquals(1, phone.cameraStopped)
        assertFalse(vm.camera.showing)
        assertFalse(vm.mustSecure.value)
        // the app goes to the background under it
        vm.showCamera()
        session.foreground(false)
        idle()
        assertEquals(2, phone.cameraStopped)
        assertNull(vm.sheet.value)
        // the app locks under it
        session.foreground(true)
        vm.showCamera()
        session.lockNow()
        idle()
        assertEquals(3, phone.cameraStopped)
        assertNull(vm.sheet.value)
    }

    /** A frame is decoded in memory and its bytes cleared; on the shared relay the library's words are shown. */
    // decision: APP-983, PHN-444
    @Test
    fun frames_stay_in_memory_and_the_shared_relay_says_the_library_s_words() {
        val pic = Bitmap.createBitmap(2, 2, Bitmap.Config.ARGB_8888)
        val watch = CameraWatch(MainScope().also { scopes += it }, phone) { pic }
        watch.start()
        idle()
        assertEquals("starting", watch.view.value!!.state)
        val bytes = byteArrayOf(1, 2, 3)
        phone.camera!!.frame(CameraFrame(bytes, 4L, "live", null))
        assertEquals(pic, watch.view.value!!.frame)
        assertTrue("the frame's bytes are cleared once decoded", bytes.all { it == 0.toByte() })
        phone.camera!!.frame(CameraFrame(null, 4L, "off", "Your Mac's camera is off."))
        assertNull("off: the last frame goes", watch.view.value!!.frame)
        val words = "Your Mac's camera is available when connected directly or through your relay."
        phone.camera!!.ended("needs_better_path", words)
        assertEquals(Note(R.string.err_mac_said, words), watch.view.value!!.ended)
        watch.stop()
        assertNull(watch.view.value)
        // refused before the stream starts: its note
        phone.cameraRefusal = Outcome.Failed(FerbException.NeedsBetterPath(words, "needs_better_path"))
        watch.start()
        idle()
        assertEquals(Note(R.string.err_needs_better_path), watch.view.value!!.ended)
    }

    // ---- the lid line, the Mac asleep, the waiting messages (round 2) ------------------------------------------------

    /** The warning's closed-lid sentence is the Mac's own (true to its helper's state); an older Mac's: the app's words. */
    // decision: APP-980, APP-981, PHN-777
    @Test
    fun the_warning_takes_the_lid_line_from_the_mac_and_falls_back_for_an_older_one() {
        val open = context.getString(R.string.insomnia_body_open)
        val fallback = context.getString(R.string.insomnia_body_closed)
        val ready = phone.insomnia.copy(lidLine = "You can close the lid too: FERB keeps working, plugged in or on battery.", closedLid = "ready")
        assertEquals("$open ${ready.lidLine}", MacAsleep.body(open, fallback, ready))
        assertEquals("an older Mac: the app's words", "$open $fallback", MacAsleep.body(open, fallback, phone.insomnia))
        assertEquals("no status yet: the app's words", "$open $fallback", MacAsleep.body(open, fallback, null))
        assertTrue(fallback.contains("on battery"))
    }

    /** Why FERB's lid helper isn't on: the Mac's words, shown under the lid line and on the Mac card; none when fine. */
    // decision: APP-981, PHN-790
    @Test
    fun the_helper_s_problem_is_the_mac_s_words_and_nothing_when_it_is_fine() {
        val waiting = phone.insomnia.copy(helper = com.ferb.mobile.InsomniaHelper("requires_approval", null,
            "FERB's helper is waiting for your OK in System Settings → General → Login Items."))
        assertEquals(waiting.helper!!.problem, MacAsleep.helperProblem(waiting))
        val fine = phone.insomnia.copy(helper = com.ferb.mobile.InsomniaHelper("enabled", null, null))
        assertNull(MacAsleep.helperProblem(fine))
        assertNull("an older Mac", MacAsleep.helperProblem(phone.insomnia))
    }

    /** The lid-on-battery warning follows the helper: new words under the same id update it quietly, never twice. */
    // decision: APP-981, PHN-789
    @Test
    fun a_warning_with_new_words_under_the_same_id_is_updated_quietly() {
        shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        session.foreground(false)
        started()
        val lid = InsomniaWarning("w-lid", "lid_closed_battery", "Your Mac's lid is closed on battery: it will sleep.", null)
        macSays(on(warnings = listOf(lid)))
        assertFalse(mine.single().quiet)
        val ready = lid.copy(text = "Your Mac's lid is closed on battery: FERB keeps it working.")
        macSays(on(warnings = listOf(ready)))
        assertEquals(ready.text, mine.single().text())
        assertTrue("updated quietly", mine.single().quiet)
        nm.cancelAll()
        macSays(on(warnings = listOf(ready)))
        assertTrue("the same words: not posted again", mine.isEmpty())
    }

    private val asleepNote: Notification? get() = shadowOf(nm).getNotification(Notifier.keyOf(Notifier.KEY_ASLEEP))

    /** The Mac can't be reached while insomnia mode was on: one quiet notification, gone when the link is back. */
    // decision: APP-981, PHN-779, PHN-788
    @Test
    fun the_mac_asleep_is_said_once_quietly_and_goes_when_the_link_is_back() {
        shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        session.foreground(false)
        phone.status = status(on())
        started()
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        idle()
        val n = asleepNote!!
        assertEquals(context.getString(R.string.mac_asleep), n.text())
        assertTrue("quiet", n.quiet)
        nm.cancelAll()
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        idle()
        assertNull("said once", asleepNote)
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        idle()
        phone.link.value = FakePhone.status(LinkPhase.ONLINE)
        idle()
        assertNull("gone when the link is back", asleepNote)
        // lost again: said again, once
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        idle()
        assertNotNull(asleepNote)
        // a link stopped for good (removed) is not a sleeping Mac
        phone.link.value = FakePhone.status(LinkPhase.STOPPED)
        idle()
        assertNull(asleepNote)
    }

    // decision: APP-981, PHN-778
    @Test
    fun the_mac_asleep_is_said_only_when_insomnia_mode_was_on() {
        shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        session.foreground(false)
        phone.status = status(phone.insomnia)
        started()
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        idle()
        assertNull(asleepNote)
        phone.link.value = FakePhone.status(LinkPhase.ONLINE)
        phone.status = status(null)
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        idle()
        assertNull("an older Mac", asleepNote)
    }

    /**
     * A message waiting for the Mac: "Your Mac is asleep; this goes when it wakes." once the link is lost while insomnia
     * mode was on; else today's "Waiting for your Mac" (and "Waiting for you to unlock" for the phone's unlock).
     */
    // decision: APP-981, PHN-401, PHN-778, PHN-779
    @Test
    fun a_waiting_message_says_the_mac_is_asleep_only_while_it_may_be() {
        val lost = FakePhone.status(LinkPhase.CONNECTING)
        val asleep = MacAsleep.shown(status(on()), lost, lost = true)
        assertTrue(asleep)
        assertEquals(R.string.outbox_mac_asleep, MacAsleep.waitingLine("link", asleep))
        assertEquals(R.string.outbox_waiting_unlock, MacAsleep.waitingLine("unlock", asleep))
        assertFalse("just connecting: not yet", MacAsleep.shown(status(on()), lost, lost = false))
        assertFalse("insomnia mode off", MacAsleep.shown(status(phone.insomnia), lost, lost = true))
        assertFalse("an older Mac", MacAsleep.shown(status(null), lost, lost = true))
        assertFalse("removed: not asleep", MacAsleep.shown(status(on()), FakePhone.status(LinkPhase.STOPPED), lost = true))
        assertEquals(R.string.outbox_waiting, MacAsleep.waitingLine("link", false))
        assertEquals("Your Mac is asleep; this goes when it wakes.", context.getString(R.string.outbox_mac_asleep))
    }

    /** The Mac asleep in a brief Power Nap wake, insomnia mode off: its own words under the tabs while it is so, and a
     *  message waiting for it says it goes when the Mac wakes; nothing from a Mac that doesn't say or a removed link. */
    // decision: SENSE-430
    @Test
    fun the_mac_asleep_in_a_power_nap_wake_says_so_in_its_own_words() {
        val words = "Your Mac is asleep with its lid closed. It wakes for a moment now and then, so replies can take a " +
            "few minutes, and anything that needs its screen waits until you open it."
        val asleep = status(phone.insomnia.copy(on = false, asleep = true, asleepLine = words))
        val online = FakePhone.status(LinkPhase.ONLINE)
        assertEquals(words, MacAsleep.line(asleep, online))
        assertEquals(words, MacAsleep.line(asleep, FakePhone.status(LinkPhase.CONNECTING)))
        assertTrue(MacAsleep.waiting(asleep, FakePhone.status(LinkPhase.CONNECTING), lost = false))
        assertNull("removed: not asleep", MacAsleep.line(asleep, FakePhone.status(LinkPhase.STOPPED)))
        assertNull("awake", MacAsleep.line(status(phone.insomnia), online))
        assertNull("an older Mac", MacAsleep.line(status(null), online))
        assertFalse(MacAsleep.waiting(status(phone.insomnia), online, lost = false))
    }
}
