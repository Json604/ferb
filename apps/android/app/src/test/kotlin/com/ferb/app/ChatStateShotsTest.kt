package com.ferb.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.unit.dp
import com.ferb.design.components.FerbBubble
import com.ferb.design.components.StatusLine
import com.ferb.design.components.StatusTone
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PlanPart
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.OutboxItem
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File
import java.time.ZoneId

/**
 * The chat's new states as the owner sees them (kb PHN-626..PHN-628), goldens in app/src/test/screenshots: the waiting
 * look while the first page comes, FERB thinking, his message sending, a day's start between days, and the home screen's
 * way back to earlier days.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class ChatStateShotsTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val vm by lazy {
        FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-chat-shots").toFile()) { null }, account = testAccount())
    }
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val base = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))
    private val today = ChatModel.day(System.currentTimeMillis(), ZoneId.systemDefault())

    @Composable
    private fun Frame(ui: UiState, home: Boolean = false, hour: Double = 22.5) {
        Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
            PhoneScreen(labMoment(hour), insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                Main(vm, ui, PhoneTab.CHAT, home = home, PhoneInsets.Lab, labMoment(hour), host)
            }
        }
    }

    private fun shoot(name: String) {
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag("phone").captureRoboImage("src/test/screenshots/$name", roborazziOptions = ScreenShotsTest.options)
    }

    // decision: PHN-628
    @Test
    fun the_first_page_on_its_way_is_a_waiting_look_never_a_blank_screen() {
        compose.setContent { Frame(base) }
        shoot("chat-waiting.png")
    }

    // decision: PHN-628
    @Test
    fun his_message_sending_and_ferb_thinking() {
        val rows = listOf(
            ChatRow.Owner("a", "What's left on my calendar today?", emptyList(), null),
            ChatRow.Ferb("b", "Two things: the design review at 4, and a call with the bank at 6:30."),
            ChatRow.Owner("c", "Move the bank call to tomorrow", emptyList(), null),
            ChatRow.Typing("c:typing"),
            ChatRow.Outbox("o:1", OutboxItem("1", "And remind me at 9", "2026-11-17T10:00:00+00:00", "sending", null, null, false, emptyList())),
        )
        compose.mainClock.autoAdvance = false // the typing dots move for as long as FERB thinks: never idle by themselves
        compose.setContent { Frame(base.copy(conversation = "c1", rows = rows)) }
        shoot("chat-sending-thinking.png")
    }

    // decision: PHN-696
    @Test
    fun today_shows_only_today_with_the_way_to_earlier_days() {
        val rows = listOf(
            ChatRow.Owner("y1", "Compare the three standing desks", emptyList(), null),
            ChatRow.Task("y2", "t-1", "On it. Three parts.", 3, 3, listOf(
                PlanPart(MarkKind.DONE, "Open your three desks", "3 pages read"),
                PlanPart(MarkKind.DONE, "Compare price and delivery", "Done"),
                PlanPart(MarkKind.DONE, "Put the cheapest in your cart", "Done"),
            )),
            ChatRow.Owner("t1", "Did the desk ship?", emptyList(), null),
            ChatRow.Ferb("t2:answer", "Yes, it arrives Thursday."),
        )
        val days = mapOf("y1" to today - 1, "y2" to today - 1, "t1" to today, "t2" to today)
        compose.setContent { Frame(base.copy(conversation = "c1", rows = rows, days = days, chatDay = today)) }
        shoot("chat-today-only.png")
    }

    // decision: PHN-696, PHN-698
    @Test
    fun the_earlier_days_are_their_own_place_with_the_way_back_to_today() {
        val rows = listOf(
            ChatRow.Owner("w1", "Book the dentist for next week", emptyList(), null),
            ChatRow.Ferb("w2:answer", "Booked: Tuesday at 10."),
            ChatRow.Owner("y1", "Compare the three standing desks", emptyList(), null),
            ChatRow.Ferb("y2:answer", "The Flexispot E7 is cheapest at $389, delivered Thursday."),
            ChatRow.Owner("t1", "Did the desk ship?", emptyList(), null),
        )
        val days = mapOf("w1" to today - 1, "w2" to today - 1, "y1" to today - 1, "y2" to today - 1, "t1" to today)
        vm.openChat()
        compose.setContent { Frame(base.copy(conversation = "c1", rows = rows, days = days, chatDay = today)) }
        shoot("chat-earlier.png")
    }

    // decision: PHN-696
    @Test
    fun typing_on_an_empty_today_stays_on_its_home_page() {
        val rows = listOf(ChatRow.Owner("y1", "Compare the three standing desks", emptyList(), null))
        vm.typed("Book a table for two at 8")
        compose.setContent { Frame(base.copy(conversation = "c1", rows = rows, days = mapOf("y1" to today - 1), chatDay = today), home = true) }
        shoot("home-typing.png")
    }

    // decision: PHN-626
    @Test
    fun a_new_day_with_nothing_said_offers_the_earlier_messages() {
        val rows = listOf(ChatRow.Owner("y1", "Compare the three standing desks", emptyList(), null))
        compose.setContent { Frame(base.copy(conversation = "c1", rows = rows, days = mapOf("y1" to today - 1)), home = true) }
        shoot("home-earlier.png")
    }

    // ---- the status line (kb PHN-660, PHN-661) and the way back to the latest messages (PHN-662, PHN-663) ----

    /** Every state of the status line over the sky at [hour], with a message of FERB's under them for comparison. */
    @Composable
    private fun StatusStates(hour: Double, fontScale: Float = 1f) {
        val d = androidx.compose.ui.platform.LocalDensity.current
        androidx.compose.runtime.CompositionLocalProvider(androidx.compose.ui.platform.LocalDensity provides androidx.compose.ui.unit.Density(d.density, fontScale)) {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
                PhoneScreen(labMoment(hour), insets = PhoneInsets.Lab, glide = false) {
                    Column(Modifier.padding(top = 90.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                        StatusLine("Reaching your Mac…", StatusTone.WORKING)
                        StatusLine("Still reaching your Mac…", StatusTone.WORKING)
                        StatusLine("FERB may use your Mac's screen until you're back", StatusTone.INFO)
                        Box(Modifier.padding(horizontal = 16.dp)) { FerbBubble("Your desk arrives Thursday.") }
                    }
                }
            }
        }
    }

    // decision: PHN-660, PHN-661
    @Test
    fun the_status_line_at_dawn() {
        compose.setContent { StatusStates(6.1) }
        shoot("status-line-dawn.png")
    }

    // decision: PHN-660, PHN-661
    @Test
    fun the_status_line_at_noon() {
        compose.setContent { StatusStates(12.5) }
        shoot("status-line-noon.png")
    }

    // decision: PHN-660, PHN-661
    @Test
    fun the_status_line_at_night() {
        compose.setContent { StatusStates(22.5) }
        shoot("status-line-night.png")
    }

    // decision: PHN-660
    @Test
    fun the_status_line_in_large_text() {
        compose.setContent { StatusStates(12.5, fontScale = 1.5f) }
        shoot("status-line-large-text.png")
    }

    // decision: PHN-660, PHN-661
    @Test
    fun the_status_line_in_the_chat() {
        val rows = listOf(
            ChatRow.Owner("a", "Did the desk ship?", emptyList(), null),
            ChatRow.Ferb("b", "Yes, it arrives Thursday."),
        )
        compose.setContent { Frame(base.copy(conversation = "c1", rows = rows, macLine = MacLine.SLOW), hour = 12.5) }
        shoot("chat-status-slow.png")
    }

    /** Thirty lines of yesterday and thirty of today, scrolled to the [key] row. */
    private fun browsing(key: String, name: String, earlier: Boolean) {
        val rows = (1..30).map { ChatRow.Ferb("y$it", "Yesterday, line $it.") } + (1..30).map { ChatRow.Ferb("t$it", "Today, line $it.") }
        val days = rows.associate { it.key to if (it.key.startsWith("y")) today - 1 else today }
        if (earlier) vm.openChat()
        compose.setContent { Frame(base.copy(conversation = "c1", rows = rows, days = days, chatDay = today)) }
        compose.waitForIdle()
        val places = ChatModel.split(ChatModel.withDays(rows, days, today), today)
        val shown = if (earlier) places.earlier else listOf<ChatRow>(ChatRow.EarlierLink("earlier-link")) + places.today
        compose.onNodeWithTag("chat-list").performScrollToIndex(shown.indexOfFirst { it.key == key })
        compose.waitForIdle()
        shoot(name)
    }

    // decision: PHN-662, PHN-663
    @Test
    fun in_an_earlier_day_the_way_back_says_today() = browsing("y10", "chat-earlier-day.png", earlier = true)

    // decision: PHN-662, PHN-663
    @Test
    fun in_today_scrolled_up_the_way_back_is_the_arrow() = browsing("t5", "chat-scrolled-up.png", earlier = false)

    // ---- files in the message box and a message with files on its way (kb PHN-690, PHN-691); today empty (PHN-695) ----

    private fun chip(name: String, size: Long) = com.ferb.app.model.Chip("c-$name", name, size, "/nowhere/$name")

    private val earlier = listOf(
        ChatRow.Owner("a", "Where are the scans from Monday?", emptyList(), null),
        ChatRow.Ferb("b", "In Documents › Scans. Do you want me to rename them by date?"),
    )

    // decision: PHN-690
    @Test
    fun one_file_waits_in_the_message_box() {
        vm.outgoing.setForTest(listOf(chip("IMG-20261118-WA0011.jpg", 2_400_000)))
        compose.setContent { Frame(base.copy(conversation = "c1", rows = earlier), hour = 12.5) }
        shoot("composer-one-file.png")
    }

    // decision: PHN-690
    @Test
    fun several_files_wait_in_the_message_box_with_words() {
        vm.outgoing.setForTest(listOf(chip("Lease agreement.pdf", 1_100_000), chip("IMG_2041.jpg", 3_200_000), chip("Budget 2027.xlsx", 84_000)))
        vm.typed("Rename these by date and put them in Scans")
        compose.setContent { Frame(base.copy(conversation = "c1", rows = earlier)) }
        shoot("composer-files.png")
    }

    // decision: PHN-691
    @Test
    fun messages_with_files_on_their_way() {
        val going = com.ferb.app.model.Sending("m1", "Here are the photos from the site", listOf(chip("IMG_2041.jpg", 3_200_000), chip("IMG_2042.jpg", 2_900_000)),
            emptyList(), com.ferb.app.model.SendPhase.UPLOADING, done = 2_700_000, total = 6_100_000)
        val waiting = com.ferb.app.model.Sending("m2", "And the lease", listOf(chip("Lease agreement.pdf", 1_100_000)),
            emptyList(), com.ferb.app.model.SendPhase.WAITING_PATH, total = 1_100_000)
        // on the shared relay, where files wait (PHN-410): the second stays waiting
        phone.link.value = FakePhone.status(path = com.ferb.mobile.LinkPath.PUBLIC_RELAY)
        vm.outgoing.setForTest(emptyList(), listOf(going, waiting))
        compose.setContent { Frame(base.copy(conversation = "c1", rows = earlier, link = phone.link.value), hour = 12.5) }
        shoot("chat-sending-files.png")
    }

    // decision: PHN-691
    @Test
    fun a_message_with_files_not_sent() {
        val stopped = com.ferb.app.model.Sending("m1", "Here are the photos from the site", listOf(chip("IMG_2041.jpg", 3_200_000)),
            emptyList(), com.ferb.app.model.SendPhase.STOPPED, done = 1_000_000, total = 3_200_000)
        val closed = com.ferb.app.model.Sending("m2", "", listOf(chip("Lease agreement.pdf", 1_100_000)),
            emptyList(), com.ferb.app.model.SendPhase.CLOSED, total = 1_100_000)
        vm.outgoing.setForTest(emptyList(), listOf(stopped, closed))
        compose.setContent { Frame(base.copy(conversation = "c1", rows = earlier)) }
        shoot("chat-not-sent-files.png")
    }


    // ---- the connection control and its sheet (kb PHN-751, PHN-752) ----

    /** The top bar with the control in each state, over the sky at [hour]. */
    @Composable
    private fun Controls(hour: Double) {
        Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
            PhoneScreen(labMoment(hour), insets = PhoneInsets.Lab, glide = false) {
                Column(Modifier.padding(top = 20.dp), verticalArrangement = Arrangement.spacedBy(22.dp)) {
                    for (m in com.ferb.design.components.LinkMark.entries) {
                        androidx.compose.runtime.CompositionLocalProvider(com.ferb.design.components.LocalConnection provides com.ferb.design.components.Connection(m, m.name, {})) {
                            com.ferb.design.components.TopBar(PhoneTab.CHAT, waiting = null, onTab = {}, onProfile = {})
                        }
                    }
                }
            }
        }
    }

    // decision: PHN-801
    @Test
    fun the_connection_control_at_dawn() {
        compose.setContent { Controls(6.1) }
        shoot("connection-dawn.png")
    }

    // decision: PHN-801
    @Test
    fun the_connection_control_at_noon() {
        compose.setContent { Controls(12.5) }
        shoot("connection-noon.png")
    }

    // decision: PHN-801
    @Test
    fun the_connection_control_at_night() {
        compose.setContent { Controls(22.5) }
        shoot("connection-night.png")
    }

    private fun sheet(ui: UiState, name: String) {
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
                PhoneScreen(labMoment(22.5), insets = PhoneInsets.Lab, glide = false) {
                    com.ferb.app.ui.ConnectionSheetFor(vm, ui, PhoneInsets.Lab) {}
                }
            }
        }
        shoot(name)
    }

    // decision: PHN-752
    @Test
    fun the_connection_sheet_connecting() = sheet(base.copy(link = FakePhone.status(com.ferb.mobile.LinkPhase.CONNECTING)), "connection-sheet-connecting.png")

    // decision: PHN-752
    @Test
    fun the_connection_sheet_on_the_shared_relay() =
        sheet(base.copy(link = FakePhone.status(path = com.ferb.mobile.LinkPath.PUBLIC_RELAY)), "connection-sheet-shared.png")

    // decision: PHN-752, PHN-758
    @Test
    fun the_connection_sheet_cant_reach() = sheet(base.copy(link = FakePhone.status(com.ferb.mobile.LinkPhase.WAITING,
        problem = com.ferb.mobile.LinkProblem("mac_unreachable", "Your Mac didn't answer. It may be asleep or off the network.", null)), linkLost = true),
        "connection-sheet-unreachable.png")
}
