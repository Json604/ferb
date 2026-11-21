package com.ferb.app

import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.view.WindowManager
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import android.content.Intent
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Question
import com.ferb.mobile.QuestionNeeds
import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * On a running emulator or phone: the merged manifest, the screen-lock gate, the lock and pairing screens, and a
 * notification posted from an invented question. Nothing pairs (the Rust test world is not on Android).
 */
@RunWith(AndroidJUnit4::class)
class OnPhoneTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val device = UiDevice.getInstance(instrumentation)

    private fun shell(cmd: String): String =
        instrumentation.uiAutomation.executeShellCommand(cmd).let { fd ->
            android.os.ParcelFileDescriptor.AutoCloseInputStream(fd).bufferedReader().readText()
        }

    @After
    fun noPin() {
        shell("locksettings clear --old 1111")
    }

    // decision: PHN-448, PHN-501, PHN-447, PHN-421, PHN-500, SENSE-420, GATE-613, PHN-604
    @Test
    fun the_manifest_asks_only_what_the_app_uses_and_keeps_out_of_backups() {
        assertEquals("com.ferb.phone", context.packageName)
        val pm = context.packageManager
        val info = pm.getPackageInfo(context.packageName, PackageManager.GET_PERMISSIONS or PackageManager.GET_SERVICES or PackageManager.GET_RECEIVERS)
        assertEquals(
            setOf(
                "android.permission.INTERNET", "android.permission.ACCESS_NETWORK_STATE", "android.permission.USE_BIOMETRIC",
                "android.permission.POST_NOTIFICATIONS", "android.permission.FOREGROUND_SERVICE",
                "android.permission.FOREGROUND_SERVICE_REMOTE_MESSAGING", "android.permission.CAMERA",
                "android.permission.ACCESS_COARSE_LOCATION", "android.permission.ACCESS_FINE_LOCATION",
                "android.permission.RECORD_AUDIO",
            ),
            info.requestedPermissions!!.toSet(),
        )
        assertEquals("no backup", 0, info.applicationInfo!!.flags and ApplicationInfo.FLAG_ALLOW_BACKUP)
        val service = info.services!!.single { it.name.endsWith("StayConnectedService") }
        assertEquals(ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING, service.foregroundServiceType)
        assertFalse(service.exported)
        assertTrue("FERB's receivers are its own", info.receivers!!.filter { it.name.startsWith("com.ferb") }.none { it.exported })
        // androidx's profile installer is the only exported one, and only to the system's DUMP holders
        assertTrue(info.receivers!!.filter { it.exported }.all { it.permission == "android.permission.DUMP" })
    }

    // decision: PHN-8, PHN-420
    @Test
    fun without_a_screen_lock_the_app_says_how_and_shows_nothing_else() {
        shell("locksettings clear --old 1111")
        ActivityScenario.launch(MainActivity::class.java).use { s ->
            assertNotNull(device.wait(Until.findObject(By.text("Set a screen lock first")), 15_000))
            s.onActivity { a -> assertTrue("FLAG_SECURE", a.window.attributes.flags and WindowManager.LayoutParams.FLAG_SECURE != 0) }
        }
    }

    // decision: PHN-8, PHN-13, PHN-447, PHN-794, PHN-615, PHN-616, PHN-636
    @Test
    fun with_a_screen_lock_it_opens_on_the_prompt_and_shows_pairing() {
        shell("locksettings set-pin 1111")
        val app = context.applicationContext as FerbApplication
        // a fresh arrival: the process away from the screen (no activity of an earlier test left), and locked
        val end = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < end && app.session.front.value) Thread.sleep(100)
        instrumentation.runOnMainSync { app.session.lockNow() }
        app.prefs.firstRunAsked = emptySet() // as on a fresh install: the first run's asks are still to come
        ActivityScenario.launch(MainActivity::class.java).use {
            // the system's own prompt by itself on open (the PIN: the emulator has no fingerprint), nothing tapped
            assertNotNull("the prompt by itself on open", device.wait(Until.findObject(By.text("Unlock FERB")), 15_000))
            assertTrue("nothing private before the unlock", device.findObject(By.text("Use FERB from your phone")) == null)
            device.pressBack() // cancelled
            assertNotNull("the lock page stays with its button", device.wait(Until.findObject(By.text("Unlock with fingerprint")), 5_000))
            assertFalse("no prompt again after a cancel", device.wait(Until.hasObject(By.text("Unlock FERB")), 3_000))
            device.setOrientationLandscape()
            Thread.sleep(1_500)
            device.setOrientationNatural()
            assertFalse("no prompt after a turn of the screen", device.wait(Until.hasObject(By.text("Unlock FERB")), 3_000))
            device.wait(Until.findObject(By.text("Unlock with fingerprint")), 5_000).click()
            assertNotNull("the button asks", device.wait(Until.findObject(By.text("Unlock FERB")), 5_000))
            Thread.sleep(1_000)
            shell("input text 1111")
            shell("input keyevent 66")
            // a fresh install: the first run's asks after the unlock, never before it (kb PHN-636); Not now still pairs
            assertNotNull("the first run's asks after the unlock", device.wait(Until.findObject(By.text("Before you start")), 15_000))
            device.findObject(By.text("Not now")).click()
            assertNotNull("pairing after the unlock", device.wait(Until.findObject(By.text("Use FERB from your phone")), 15_000))
            // the owner's own lock from inside: the page waits for his tap
            instrumentation.runOnMainSync { app.session.lockNow() }
            assertNotNull("his own lock", device.wait(Until.findObject(By.text("Unlock with fingerprint")), 5_000))
            assertFalse("no prompt by itself after his own lock", device.wait(Until.hasObject(By.text("Unlock FERB")), 3_000))
            device.findObject(By.text("Unlock with fingerprint")).click()
            assertNotNull(device.wait(Until.findObject(By.text("Unlock FERB")), 5_000))
            Thread.sleep(1_000)
            shell("input text 1111")
            shell("input keyevent 66")
            assertNotNull(device.wait(Until.findObject(By.text("Use FERB from your phone")), 15_000))
            device.findObject(By.text("Type code")).click()
            assertNotNull("the type-code sheet", device.wait(Until.findObject(By.text("Type the code")), 5_000))
            device.pressBack()
        }
    }

    // decision: PHN-415, PHN-417, PHN-422, PHN-504
    // decision: PHN-519
    @Test
    fun a_question_is_posted_with_its_buttons_and_no_words_on_the_lock_screen_for_checks() {
        shell("pm grant ${context.packageName} android.permission.POST_NOTIFICATIONS")
        val app = context as FerbApplication
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.cancelAll()
        val routineNeeds = QuestionNeeds(true, false, false, false, null, null, false)
        val fingerprintNeeds = QuestionNeeds(true, true, false, true, "ch-1", null, false)
        val routine = Question("ap-test-1", null, "Which size?", "Which size of desk should FERB pick?", null, "owner_question", "owner.ask", listOf("Small", "Large"), emptyList(), false, routineNeeds)
        val check = Question("ap-test-2", null, "Pay $48.20?", "Pay the electricity bill now.", null, "owner_question", "ferb.confirm", listOf("Yes", "No"), listOf("Pay", "Don't"), false, routineNeeds)
        val private = Question("ap-test-3", null, null, "The 6-digit code Northfield Energy sent you.", null, "owner_login_code", null, emptyList(), emptyList(), true, QuestionNeeds(true, true, true, false, "ch-3", null, false))
        val fingerprint = Question("ap-test-4", null, "Delete 12 files?", "Empty the Trash on your Mac.", null, "owner_question", "ferb.confirm", listOf("Yes", "No"), emptyList(), false, fingerprintNeeds)
        val held = Question("ap-test-5", null, null, "", null, "owner_question", null, emptyList(), emptyList(), false, routineNeeds, held = true)
        app.notifier.question(routine, lockedNow = false)
        app.notifier.question(check, lockedNow = true)
        app.notifier.question(private, lockedNow = false)
        app.notifier.question(fingerprint, lockedNow = false)
        app.notifier.question(held, lockedNow = false)
        Thread.sleep(500)
        val posted = nm.activeNotifications.associateBy { it.id }
        val r = posted.getValue(Notifier.keyOf("q:ap-test-1")).notification
        assertEquals(listOf("Small", "Large"), r.actions.map { it.title.toString() }.sortedBy { if (it == "Small") 0 else 1 })
        assertEquals(Notification.VISIBILITY_PRIVATE, r.visibility)
        assertFalse("the public version has no words", r.publicVersion.extras.getCharSequence(Notification.EXTRA_TEXT).toString().contains("desk"))
        // PHN-417: a Yes that needs the fingerprint is answered in the app: worded, no buttons
        val f = posted.getValue(Notifier.keyOf("q:ap-test-4")).notification
        assertTrue("a fingerprint question has no buttons", f.actions == null || f.actions.isEmpty())
        assertTrue(f.extras.getCharSequence(Notification.EXTRA_TEXT).toString().contains("Needs your fingerprint"))
        assertTrue(f.extras.getCharSequence(Notification.EXTRA_TEXT).toString().contains("Trash"))
        // PHN-126: held words: wordless, no buttons
        val h = posted.getValue(Notifier.keyOf("q:ap-test-5")).notification
        assertTrue(h.actions == null || h.actions.isEmpty())
        assertEquals("FERB has a question", h.extras.getCharSequence(Notification.EXTRA_TITLE).toString())
        val c = posted.getValue(Notifier.keyOf("q:ap-test-2")).notification
        assertEquals(Notifier.CH_CHECKS, c.channelId)
        // posted while locked: no words, no buttons; the owner's unlock words it again
        assertFalse(c.extras.getCharSequence(Notification.EXTRA_TEXT).toString().contains("electricity"))
        assertTrue(c.actions == null || c.actions.isEmpty())
        app.notifier.question(check, lockedNow = false, quietly = true)
        Thread.sleep(300)
        val worded = nm.activeNotifications.single { it.id == Notifier.keyOf("q:ap-test-2") }.notification
        assertTrue(worded.extras.getCharSequence(Notification.EXTRA_TEXT).toString().contains("electricity"))
        assertEquals(listOf("Pay", "Don't"), worded.actions.map { it.title.toString() })
        val p = posted.getValue(Notifier.keyOf("q:ap-test-3")).notification
        assertTrue("a private answer has no buttons", p.actions == null || p.actions.isEmpty())
        assertFalse("and its question is not worded", p.extras.getCharSequence(Notification.EXTRA_TEXT).toString().contains("Northfield"))
        nm.cancelAll()
    }

    /**
     * On Android's own notification service (kb PHN-760..764): the planner's question is posted on the high-importance
     * questions channel with its words, no buttons and a tap that opens the chat; one he already saw in the app is
     * posted in the silent group (no sound, no heads-up); a new one is not.
     */
    // decision: PHN-760, PHN-762, PHN-764
    @Test
    fun the_planner_s_question_pops_and_one_he_saw_goes_to_the_shade_quietly() {
        shell("pm grant ${context.packageName} android.permission.POST_NOTIFICATIONS")
        val app = context as FerbApplication
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.cancelAll()
        val asks = QuestionNeeds(true, false, false, false, null, null, false)
        val clarify = Question("clarify:r-test", null, null, "Which email account should I use?", null, Question.CLARIFY, null, emptyList(), emptyList(), false, asks)
        val seen = Question("ap-test-seen", null, "Send it?", "Send the draft to Ajay.", null, "owner_question", "owner.ask", listOf("Yes", "No"), emptyList(), false, asks)
        app.notifier.question(clarify, lockedNow = false)
        app.notifier.question(seen, lockedNow = false, quietly = true)
        Thread.sleep(500)
        val posted = nm.activeNotifications.associateBy { it.id }
        val q = posted.getValue(Notifier.keyOf("q:clarify:r-test")).notification
        assertEquals(Notifier.CH_QUESTIONS, q.channelId)
        assertEquals(NotificationManager.IMPORTANCE_HIGH, nm.getNotificationChannel(Notifier.CH_QUESTIONS).importance)
        assertTrue(q.extras.getCharSequence(Notification.EXTRA_TEXT).toString().startsWith("Which email account should I use?"))
        assertTrue("answered by a message: no buttons", q.actions == null || q.actions.isEmpty())
        assertTrue("a new one alerts", q.group == null && q.flags and Notification.FLAG_ONLY_ALERT_ONCE == 0)
        val s = posted.getValue(Notifier.keyOf("q:ap-test-seen")).notification
        assertEquals(Notifier.SILENT_GROUP, s.group)
        assertEquals(Notification.GROUP_ALERT_SUMMARY, s.groupAlertBehavior)
        nm.cancelAll()
    }

    // decision: PHN-454, PHN-420
    // decision: PHN-518
    @Test
    fun watch_live_keeps_the_window_out_of_screenshots_even_with_the_switch_off() {
        shell("locksettings clear --old 1111")
        val app = context as FerbApplication
        ActivityScenario.launch(MainActivity::class.java).use { s ->
            fun secure(): Boolean { var on = false; s.onActivity { a -> on = a.window.attributes.flags and WindowManager.LayoutParams.FLAG_SECURE != 0 }; return on }
            fun waitFor(want: Boolean): Boolean { val end = System.currentTimeMillis() + 5_000; while (secure() != want && System.currentTimeMillis() < end) Thread.sleep(50); return secure() == want }
            try {
                s.onActivity { a -> app.session.unlocked(); a.setBlockScreenshots(false) }
                assertTrue("the switch off and the app unlocked: screenshots allowed", waitFor(false))
                s.onActivity { a -> a.vm.watchLive("t-test") }
                assertTrue("Watch live: FLAG_SECURE whatever the switch says", waitFor(true))
                s.onActivity { a -> a.vm.closeLive() }
                assertTrue(waitFor(false))
            } finally {
                s.onActivity { a -> a.setBlockScreenshots(true); a.vm.closeLive(); app.session.lockNow() }
            }
        }
    }

    // decision: PHN-113, PHN-115, PHN-11, PHN-15
    // decision: PHN-515
    // decision: PHN-517
    @Test
    fun a_picked_file_is_copied_here_and_a_fetched_file_is_handed_out_only_through_ferbs_provider() {
        val local = LocalFiles.forPhone(context)
        // the picker's answer, stood in by a Uri to a file the test made (no picker is opened)
        val src = File(context.cacheDir, "test-picked/receipt.pdf").apply { parentFile!!.mkdirs(); writeBytes(ByteArray(64) { it.toByte() }) }
        val staged = local.stage(android.net.Uri.fromFile(src).toString())!!
        assertEquals("receipt.pdf", staged.name)
        assertTrue(staged.path.startsWith(local.outgoing.path))
        assertArrayEquals(src.readBytes(), File(staged.path).readBytes())
        // a fetched file: Open and Share build the system's chooser on FERB's provider, with read access; nothing starts
        val fetched = local.fetchTarget("f-test", "Lease agreement.pdf").apply { writeBytes(ByteArray(32) { 7 }) }
        val open = Handout.open(context, fetched, fetched.name)
        assertEquals(Intent.ACTION_CHOOSER, open.action)
        @Suppress("DEPRECATION") val view = open.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)!!
        assertEquals(Intent.ACTION_VIEW, view.action)
        assertEquals("application/pdf", view.type)
        assertEquals("content", view.data!!.scheme)
        assertEquals("${context.packageName}.files", view.data!!.authority)
        assertTrue(view.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0)
        assertArrayEquals(fetched.readBytes(), context.contentResolver.openInputStream(view.data!!)!!.use { it.readBytes() })
        @Suppress("DEPRECATION") val send = Handout.share(context, fetched, fetched.name).getParcelableExtra<Intent>(Intent.EXTRA_INTENT)!!
        assertEquals(Intent.ACTION_SEND, send.action)
        // the copies waiting to be sent are not reachable through the provider
        assertTrue(runCatching { Handout.uri(context, File(staged.path)) }.isFailure)
        // PHN-15: the lock removes the fetched files; the provider then has nothing
        local.wipeFetched()
        assertFalse(fetched.exists())
        assertTrue(runCatching { context.contentResolver.openInputStream(view.data!!)!!.close() }.isFailure)
        local.remove(staged.path)
        src.parentFile!!.deleteRecursively()
    }

    // decision: PHN-421, PHN-501
    @Test
    fun stay_connected_runs_as_a_foreground_service_with_its_quiet_line() {
        shell("pm grant ${context.packageName} android.permission.POST_NOTIFICATIONS")
        val nm = context.getSystemService(NotificationManager::class.java)
        ActivityScenario.launch(MainActivity::class.java).use { s ->
            s.onActivity { a -> StayConnectedService.set(a, true) }
            val deadline = System.currentTimeMillis() + 10_000
            while (!StayConnectedService.running && System.currentTimeMillis() < deadline) Thread.sleep(100)
            assertTrue("the service runs", StayConnectedService.running)
            Thread.sleep(500)
            val line = nm.activeNotifications.single { it.id == StayConnectedService.ID }.notification
            assertEquals(Notifier.CH_STAY, line.channelId)
            assertEquals("FERB stays connected", line.extras.getCharSequence(Notification.EXTRA_TITLE).toString())
            assertTrue((context as FerbApplication).signals.stayConnected)
            s.onActivity { a -> StayConnectedService.set(a, false) }
            val gone = System.currentTimeMillis() + 10_000
            while (StayConnectedService.running && System.currentTimeMillis() < gone) Thread.sleep(100)
            assertFalse(StayConnectedService.running)
        }
    }
}
