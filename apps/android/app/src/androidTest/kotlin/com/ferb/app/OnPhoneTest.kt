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
import com.ferb.app.model.Question
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

    // decision: PHN-448, PHN-501, PHN-447, PHN-421, PHN-500
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

    // decision: PHN-8, PHN-13, PHN-447, PHN-505
    @Test
    fun with_a_screen_lock_it_opens_on_the_prompt_and_shows_pairing() {
        shell("locksettings set-pin 1111")
        ActivityScenario.launch(MainActivity::class.java).use {
            val unlock = device.wait(Until.findObject(By.text("Unlock with fingerprint")), 15_000)
            assertNotNull("the lock screen", unlock)
            assertTrue("nothing private before the unlock", device.findObject(By.text("Use FERB from your phone")) == null)
            unlock.click()
            // the system's own prompt: the PIN (the emulator has no fingerprint)
            device.wait(Until.hasObject(By.pkg("com.android.systemui")), 5_000)
            Thread.sleep(1_000)
            shell("input text 1111")
            shell("input keyevent 66")
            assertNotNull("pairing after the unlock", device.wait(Until.findObject(By.text("Use FERB from your phone")), 15_000))
            device.findObject(By.text("Type code")).click()
            assertNotNull("the type-code sheet", device.wait(Until.findObject(By.text("Type the code")), 5_000))
            device.pressBack()
        }
    }

    // decision: PHN-415, PHN-417, PHN-422, PHN-504
    @Test
    fun a_question_is_posted_with_its_buttons_and_no_words_on_the_lock_screen_for_checks() {
        shell("pm grant ${context.packageName} android.permission.POST_NOTIFICATIONS")
        val app = context as FerbApplication
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.cancelAll()
        val routine = Question("ap-test-1", null, "Which size?", "Which size of desk should FERB pick?", null, "owner_question", "owner.ask", listOf("Small", "Large"), emptyList(), false)
        val check = Question("ap-test-2", null, "Pay $48.20?", "Pay the electricity bill now.", null, "owner_question", "ferb.confirm", listOf("Yes", "No"), listOf("Pay", "Don't"), false)
        val private = Question("ap-test-3", null, null, "The 6-digit code Northfield Energy sent you.", null, "owner_login_code", null, emptyList(), emptyList(), true)
        app.notifier.question(routine, lockedNow = false)
        app.notifier.question(check, lockedNow = true)
        app.notifier.question(private, lockedNow = false)
        Thread.sleep(500)
        val posted = nm.activeNotifications.associateBy { it.id }
        val r = posted.getValue(Notifier.keyOf("q:ap-test-1")).notification
        assertEquals(listOf("Small", "Large"), r.actions.map { it.title.toString() }.sortedBy { if (it == "Small") 0 else 1 })
        assertEquals(Notification.VISIBILITY_PRIVATE, r.visibility)
        assertFalse("the public version has no words", r.publicVersion.extras.getCharSequence(Notification.EXTRA_TEXT).toString().contains("desk"))
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
