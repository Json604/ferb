package com.ferb.app

import android.Manifest
import android.app.Application
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.location.Location
import android.location.LocationManager
import android.os.BatteryManager
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.Coarse
import com.ferb.app.model.DeviceFacts
import com.ferb.app.model.DeviceReporter
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.PlaceKeeper
import com.ferb.app.model.Why
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.mobile.Change
import com.ferb.mobile.DevicePlace
import com.ferb.mobile.DeviceReport
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.LocationFix
import java.nio.file.Files
import java.time.Duration
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * T-1214 on the phone: the facts from Android's own events (the reporter's dedupe and steps, no timer), the city only
 * after the owner's switch and Android's permission (and the geocoder only after a kilometre), and the Mac's request
 * for one exact fix: the sheet in front, a notification in the background, the answer either way.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class DeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val phone = FakePhone()
    private lateinit var session: Session
    private val prefs = Prefs(context)

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { 0L }
        prefs.shareCity = false
    }

    @After
    fun tearDown() = Dispatchers.resetMain()

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    /** Work that hops to the IO threads comes back by itself: wait for it, bounded. */
    private fun until(cond: () -> Boolean) {
        val end = System.currentTimeMillis() + 5_000
        while (!cond() && System.currentTimeMillis() < end) { idle(); Thread.sleep(10) }
        idle()
        assertTrue(cond())
    }

    private val open = PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))

    // decision: SENSE-420
    // decision: PHN-530
    // decision: PHN-531
    @Test
    fun each_change_is_sent_once_the_battery_by_five_point_steps_and_nothing_on_a_timer() = runTest {
        val state = MutableStateFlow<PhoneState>(open)
        val link = MutableStateFlow<LinkStatus?>(FakePhone.status())
        val sent = ArrayList<DeviceReport>()
        val r = DeviceReporter(DeviceFacts(model = "Invented Model", timeZone = "Asia/Kolkata", utcOffsetMinutes = 330))
        r.start(backgroundScope, state, link) { sent += it; Outcome.Ok(true) }
        runCurrent()
        assertEquals("the start sends what Android said", 1, sent.size)
        r.battery(80, false); runCurrent()
        assertEquals(2, sent.size)
        r.battery(81, false); r.battery(84, false); runCurrent()
        assertEquals("inside the same five-point step: nothing", 2, sent.size)
        assertEquals(80.toUByte(), sent.last().batteryPercent)
        r.battery(85, false); runCurrent()
        assertEquals(85.toUByte(), sent.last().batteryPercent)
        r.battery(85, true); runCurrent()
        assertEquals("charging changed", true, sent.last().charging)
        r.zone("Asia/Kolkata", 330); runCurrent()
        assertEquals("the same zone again: nothing", 4, sent.size)
        // a day with no event from Android: nothing is sent (no timer, no polling)
        advanceTimeBy(Duration.ofDays(1).toMillis()); runCurrent()
        assertEquals(4, sent.size)
        // a new connection hands the report to the library again (it sends once per connection)
        link.value = FakePhone.status(LinkPhase.WAITING); runCurrent()
        link.value = FakePhone.status(); runCurrent()
        assertTrue(sent.size > 4)
        // the store closed: nothing goes
        val before = sent.size
        state.value = PhoneState.Closed
        r.network("mobile"); runCurrent()
        assertEquals(before, sent.size)
        assertFalse("never a place while the city is off", sent.any { it.place != null })
    }

    // decision: SENSE-420, PHN-420
    // decision: PHN-638
    @Test
    fun the_city_is_looked_up_once_per_kilometre_and_the_sky_takes_the_rounded_fix() = runBlocking {
        val asked = ArrayList<Pair<Double, Double>>()
        val keeper = PlaceKeeper { lat, lon -> asked += lat to lon; DevicePlace("Pune", "Maharashtra", "India", "IN") }
        assertEquals("Pune", keeper.fix(18.5204, 73.8567)?.city)
        keeper.fix(18.5230, 73.8590) // about 380 m
        assertEquals("a short move asks nothing", 1, asked.size)
        assertEquals(Coarse(18.52, 73.86), keeper.coarse.value)
        keeper.fix(18.5400, 73.8567) // about 2.2 km
        assertEquals(2, asked.size)
        keeper.forget()
        assertNull(keeper.coarse.value)
        assertNull(keeper.place)
    }

    // decision: SENSE-420, PHN-420
    // decision: PHN-530, PHN-638
    @Test
    fun androids_broadcasts_reach_the_report_and_the_city_waits_for_the_switch_and_the_permission() {
        val scope = MainScope()
        val geocoded = ArrayList<Pair<Double, Double>>()
        val reporter = DeviceReporter(DeviceSensors.snapshot(context))
        val sensors = DeviceSensors(context, reporter, prefs, scope) { lat, lon -> geocoded += lat to lon; DevicePlace("Pune", "Maharashtra", "India", "IN") }
        sensors.start()
        fun battery(level: Int, status: Int = BatteryManager.BATTERY_STATUS_DISCHARGING) = context.sendBroadcast(
            Intent(Intent.ACTION_BATTERY_CHANGED).putExtra(BatteryManager.EXTRA_LEVEL, level).putExtra(BatteryManager.EXTRA_SCALE, 100).putExtra(BatteryManager.EXTRA_STATUS, status))
        battery(50); idle()
        assertEquals(50, reporter.facts.value.batteryPercent)
        battery(52); idle()
        assertEquals(50, reporter.facts.value.batteryPercent)
        battery(55, BatteryManager.BATTERY_STATUS_CHARGING); idle()
        assertEquals(55, reporter.facts.value.batteryPercent)
        assertEquals(true, reporter.facts.value.charging)
        context.sendBroadcast(Intent(Intent.ACTION_TIMEZONE_CHANGED).putExtra(Intent.EXTRA_TIMEZONE, "Europe/London")); idle()
        assertEquals("Europe/London", reporter.facts.value.timeZone)
        context.sendBroadcast(Intent(Intent.ACTION_SCREEN_OFF)); idle()
        assertEquals(false, reporter.facts.value.screenOn)
        context.sendBroadcast(Intent(Intent.ACTION_USER_PRESENT)); idle()
        assertEquals(true, reporter.facts.value.screenOn)
        assertEquals(false, reporter.facts.value.screenLocked)
        // nothing waits on the main thread's clock: every fact came from an event
        assertEquals(Duration.ZERO, shadowOf(Looper.getMainLooper()).nextScheduledTaskTime)
        // the city: off until the owner's switch; then Android's permission; then fixes
        assertEquals(DeviceFacts.LOCATION_OFF, reporter.facts.value.location)
        prefs.shareCity = true
        sensors.city()
        assertEquals(DeviceFacts.LOCATION_DENIED, reporter.facts.value.location)
        shadowOf(context as Application).grantPermissions(Manifest.permission.ACCESS_COARSE_LOCATION)
        val lm = context.getSystemService(LocationManager::class.java)
        shadowOf(lm).setLocationEnabled(true)
        sensors.city()
        assertEquals(DeviceFacts.LOCATION_ON, reporter.facts.value.location)
        shadowOf(lm).simulateLocation(Location(LocationManager.PASSIVE_PROVIDER).apply { latitude = 18.5204; longitude = 73.8567; accuracy = 900f; time = System.currentTimeMillis() })
        idle()
        assertEquals("Pune", reporter.facts.value.place?.city)
        assertEquals(Coarse(18.52, 73.86), sensors.coarse.value)
        assertEquals(1, geocoded.size)
        assertTrue("the report carries the city, never coordinates", reporter.facts.value.report().place?.city == "Pune")
        // off again: nothing kept
        prefs.shareCity = false
        sensors.city()
        assertEquals(DeviceFacts.LOCATION_OFF, reporter.facts.value.location)
        assertNull(reporter.facts.value.report().place)
        assertNull(sensors.coarse.value)
        scope.cancel()
    }

    private class FakeLocator(var allowed: Boolean = true, var fix: LocationFix? = LocationFix(18.5204, 73.8567, 8.0)) : Locator {
        var asked = 0
        override fun fine() = allowed
        override fun coarse() = allowed
        override suspend fun exact(timeoutMs: Long): LocationFix? { asked++; return fix }
    }

    private fun vm(locator: Locator, answered: () -> Unit = {}) =
        FerbViewModel(phone, session, prefs, { true }, null, SavedStateHandle(), LocalFiles(Files.createTempDirectory("ferb-device").toFile()) { null },
            locator = locator, say = context::getString, locationAnswered = answered).also {
            it.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind)
                override suspend fun local() = true
            }
        }

    // decision: GATE-613, SENSE-420
    // decision: PHN-537
    @Test
    fun the_macs_request_for_the_exact_place_opens_a_sheet_in_front_and_waits_otherwise() {
        session.unlocked()
        session.foreground(true)
        val locator = FakeLocator()
        var answered = 0
        val vm = vm(locator) { answered++ }
        phone.changes.tryEmit(Change.LocationWanted("loc-1")); idle()
        assertEquals(Sheet.Location("loc-1"), vm.sheet.value)
        vm.shareLocation("loc-1", granted = true); idle()
        assertEquals(Triple("loc-1", LocationFix(18.5204, 73.8567, 8.0), null), phone.located.single())
        assertNull(vm.sheet.value)
        assertEquals(R.string.exact_sent, vm.ui.value.note?.res)
        assertEquals("its notification goes", 1, answered)
        // Android's permission refused: the Mac is told why, nothing read
        locator.asked = 0
        vm.shareLocation("loc-2", granted = false); idle()
        assertEquals(context.getString(R.string.loc_why_denied), phone.located.last().third)
        assertEquals(0, locator.asked)
        // no fix in time
        locator.fix = null
        vm.shareLocation("loc-3", granted = true); idle()
        assertEquals(context.getString(R.string.loc_why_none), phone.located.last().third)
        // the owner says no
        vm.declineLocation("loc-4"); idle()
        assertEquals(context.getString(R.string.loc_why_no), phone.located.last().third)
        assertNull(phone.located.last().second)
        // in the background: no sheet until the owner is back
        session.foreground(false); idle()
        phone.changes.tryEmit(Change.LocationWanted("loc-5")); idle()
        assertNull(vm.sheet.value)
        session.foreground(true)
        until { vm.sheet.value == Sheet.Location("loc-5") }
        vm.show(null)
        // asked while the app was locked: read at the unlock
        session.lockNow(); idle()
        phone.wanted = "loc-6"
        session.unlocked(); idle()
        assertEquals(Sheet.Location("loc-6"), vm.sheet.value)
    }

    // decision: GATE-613, PHN-415
    // decision: PHN-537
    @Test
    fun in_the_background_the_request_is_a_notification_with_no_button_that_shares() {
        shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        val scope = MainScope()
        val notifier = Notifier(context, phone, prefs, session)
        notifier.start(scope)
        session.unlocked()
        session.foreground(true)
        val nm = context.getSystemService(NotificationManager::class.java)
        phone.changes.tryEmit(Change.LocationWanted("loc-1")); idle()
        assertTrue("in front: the sheet, not a notification", shadowOf(nm).allNotifications.isEmpty())
        session.foreground(false)
        phone.changes.tryEmit(Change.LocationWanted("loc-2")); idle()
        val n = shadowOf(nm).allNotifications.single()
        assertEquals(context.getString(R.string.n_exact_title), n.extras.getCharSequence(android.app.Notification.EXTRA_TITLE).toString())
        assertTrue("no button shares anything", n.actions == null || n.actions.isEmpty())
        assertEquals("loc-2", shadowOf(n.contentIntent).savedIntent.getStringExtra(MainActivity.EXTRA_LOCATION))
        notifier.locationAnswered()
        assertTrue(shadowOf(nm).allNotifications.isEmpty())
        scope.cancel()
    }
}
