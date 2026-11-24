package com.ferb.app

import android.app.Application
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeDown
import androidx.compose.ui.unit.dp
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.core.PhoneState
import com.ferb.design.Phone
import com.ferb.design.components.ChatJump
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.chatJump
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.Change
import com.ferb.mobile.Item
import com.ferb.mobile.NoticeItem
import com.ferb.mobile.OlderState
import com.ferb.mobile.TranscriptView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import java.time.LocalDate
import java.time.ZoneId

/**
 * The chat as the owner met it on his phone (kb PHN-625..PHN-628): the home screen only for an empty chat (a first-time
 * user or a new day with nothing said), the chat as soon as something was said today; what arrives after the chat was
 * first drawn shows without anything else changing (through the app's real tab and page hosts); the end followed while
 * he is at it, a reply that grows included; a pill when he scrolled up; a day's start between days.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class ChatListTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val scheduler = TestCoroutineScheduler()
    private val zone = ZoneId.systemDefault()
    /** The phone's own today: the chat names days by the phone's clock ("Today", "Yesterday"). */
    private val today = LocalDate.now(ZoneId.systemDefault())
    private var nowMs = today.atTime(10, 0).atZone(zone).toInstant().toEpochMilli()
    private lateinit var session: Session
    private val root: File = java.nio.file.Files.createTempDirectory("ferb-chat").toFile()
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
        session = Session { 0L }
        phone.state.value = PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm() = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
        LocalFiles(root) { null }, now = { nowMs }, account = testAccount())

    /** A line of FERB's made on [day] at [hour], as the library hands it (its time counts from 2001-01-01). */
    private fun said(id: String, text: String, day: LocalDate, hour: Int = 9): Item {
        val at = day.atTime(hour, 0).atZone(zone).toEpochSecond() - 978_307_200.0
        return Item.Notice(NoticeItem(id, at, false, id, null, text, emptyList(), null))
    }

    private fun view(vararg items: Item) = TranscriptView("c1", items.toList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)

    private fun changed() {
        phone.changes.tryEmit(Change.Transcript("c1"))
    }

    // decision: PHN-625, PHN-626
    @Test
    fun opening_with_something_said_today_shows_the_chat_not_the_home_screen() {
        phone.view = view(said("e1", "Your backup finished.", today))
        val vm = vm()
        session.unlocked()
        assertFalse("the chat, not the home screen: he said or FERB posted something today", vm.home.value)
    }

    // decision: PHN-625, PHN-626
    @Test
    fun a_new_day_with_nothing_said_shows_home_until_the_first_word_of_the_day() {
        phone.view = view(said("e1", "Yesterday's answer.", today.minusDays(1)))
        val vm = vm()
        session.unlocked()
        assertTrue("a new day with nothing said yet: the home screen", vm.home.value)
        phone.view = view(said("e1", "Yesterday's answer.", today.minusDays(1)), said("e2", "Good morning, your 9:00 call moved.", today))
        changed()
        assertFalse("FERB posted today: the chat", vm.home.value)
    }

    // decision: PHN-626
    @Test
    fun tapping_chat_on_the_chat_stays_on_the_chat() {
        phone.view = view(said("e1", "Done.", today))
        val vm = vm()
        session.unlocked()
        vm.choose(PhoneTab.CHAT)
        vm.choose(PhoneTab.CHAT)
        assertFalse(vm.home.value)
    }

    // decision: PHN-696
    @Test
    fun the_chat_has_a_days_start_between_days() {
        val v = view(said("e1", "Older.", today.minusDays(3)), said("e2", "Yesterday.", today.minusDays(1)), said("e3", "Now.", today))
        val rows = ChatModel.withDays(ChatModel.rows(v), ChatModel.days(v, zone), today.toEpochDay())
        val days = rows.filterIsInstance<ChatRow.Day>().map { it.day }
        assertEquals(listOf(today.minusDays(3).toEpochDay(), today.minusDays(1).toEpochDay(), today.toEpochDay()), days)
        assertTrue(rows.first() is ChatRow.Day)
    }

    /** What arrives after the chat was first drawn shows at once: nothing else has to change (no typing). */
    // decision: PHN-625, PHN-622
    @Test
    fun entries_that_arrive_after_the_chat_was_drawn_show_without_anything_else_changing() {
        var ui by mutableStateOf(UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK)))
        val vm = vm()
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) { Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host) }
            }
        }
        compose.waitForIdle()
        val v = view(said("e1", "Your desk order is in the cart.", today))
        ui = ui.copy(rows = ChatModel.rows(v), days = ChatModel.days(v, zone), conversation = "c1")
        compose.waitForIdle()
        compose.onNodeWithText("Your desk order is in the cart.").assertIsDisplayed()
    }

    /** A reply that grows as it streams keeps its end in view while he is at the end; scrolled up, a pill instead. */
    // decision: PHN-628, PHN-629
    @Test
    fun the_end_is_followed_while_he_is_at_it_and_a_pill_says_when_he_is_not() {
        val many = (1..30).map { said("e$it", "Message number $it of the day.", today, hour = 8) }
        var ui by mutableStateOf(UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = ChatModel.rows(view(*many.toTypedArray()))))
        val vm = vm()
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) { Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host) }
            }
        }
        compose.waitForIdle()
        compose.onNodeWithText("Message number 30 of the day.").assertIsDisplayed()
        // FERB's reply arrives and grows by many lines in one row: its last line is in view
        val long = (1..40).joinToString("\n") { "Line $it of the reply." }
        ui = ui.copy(rows = ui.rows + ChatRow.Ferb("r:answer", "Line 1 of the reply."))
        compose.waitForIdle()
        ui = ui.copy(rows = ui.rows.dropLast(1) + ChatRow.Ferb("r:answer", long))
        compose.waitForIdle()
        val reply = compose.onNodeWithText(long).fetchSemanticsNode().boundsInRoot
        val box = compose.onNodeWithText("Add to this", substring = true, useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
        assertTrue("the reply's last line is above the message box: ${reply.bottom} <= ${box.top}", reply.bottom <= box.top)
        // he scrolls up; something new arrives: the list stays where he put it and the pill says it
        compose.onNodeWithText("Line 1 of the reply.", substring = true).performTouchInput { swipeDown(startY = top + 10, endY = bottom, durationMillis = 300) }
        compose.waitForIdle()
        Snapshot.sendApplyNotifications()
        ui = ui.copy(rows = ui.rows + ChatRow.Ferb("r2:answer", "One more thing."))
        compose.waitForIdle()
        compose.onNodeWithText("New message").assertIsDisplayed()
    }

    // ---- the way back to the latest messages and the day he is in (kb PHN-662, PHN-663) ----

    /** Thirty lines of yesterday and thirty of today, drawn by the app's real Main. */
    // ---- two places, as FERB.app separates days (kb PHN-696..PHN-698) ----

    private class Shown(val vm: FerbViewModel, val rows: () -> List<ChatRow>, val add: (ChatRow) -> Unit)

    /**
     * [yesterday] lines of yesterday and [todays] of today, drawn by the app's real Main, its page (today or the earlier
     * days) the view model's own; [earlier]: "Earlier messages" was opened.
     */
    private fun days(yesterday: Int, todays: Int, earlier: Boolean = false): Shown {
        val items = (1..yesterday).map { said("y$it", "Yesterday line $it.", today.minusDays(1), hour = 8) } +
            (1..todays).map { said("t$it", "Today line $it.", today, hour = 8) }
        val v = view(*items.toTypedArray())
        var ui by mutableStateOf(UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = ChatModel.rows(v), days = ChatModel.days(v, zone), chatDay = today.toEpochDay()))
        val vm = vm()
        if (earlier) vm.openChat()
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) { Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host) }
            }
        }
        compose.waitForIdle()
        val shown = {
            val d = today.toEpochDay()
            val p = ChatModel.split(ChatModel.withDays(ui.rows, ui.days, d), d)
            if (vm.earlier.value) p.earlier else if (p.earlier.isNotEmpty() && p.today.isNotEmpty()) listOf<ChatRow>(ChatRow.EarlierLink("earlier-link")) + p.today else p.today
        }
        return Shown(vm, shown, { ui = ui.copy(rows = ui.rows + it) })
    }

    private fun scrollTo(rows: List<ChatRow>, key: String) {
        compose.onNodeWithTag("chat-list").performScrollToIndex(rows.indexOfFirst { it.key.startsWith(key) })
        compose.waitForIdle()
    }

    // decision: PHN-696, PHN-698
    @Test
    fun today_shows_only_todays_rows_with_the_way_to_earlier_days_at_its_top() {
        val chat = days(30, 5)
        compose.onNodeWithText("Today line 5.").assertIsDisplayed()
        compose.onNodeWithText("Yesterday line", substring = true).assertDoesNotExist()
        compose.onNodeWithTag("earlier-link").assertIsDisplayed().performClick()
        compose.waitForIdle()
        assertTrue("the earlier days", chat.vm.earlier.value)
        compose.onNodeWithText("Yesterday line 30.").assertIsDisplayed()
        compose.onNodeWithText("Today line", substring = true).assertDoesNotExist()
        compose.onNodeWithTag("chat-jump").assert(hasText("Today")).performClick()
        compose.waitForIdle()
        assertFalse("back to today", chat.vm.earlier.value)
        compose.onNodeWithText("Today line 5.").assertIsDisplayed()
        compose.onNodeWithText("Yesterday line", substring = true).assertDoesNotExist()
    }

    // decision: PHN-662, PHN-664, PHN-696
    @Test
    fun in_today_scrolled_up_the_way_back_is_the_arrow_and_a_new_message_says_so() {
        val chat = days(2, 30)
        scrollTo(chat.rows(), "t5")
        compose.onNodeWithTag("chat-jump").assert(hasContentDescription("Go to the latest messages"))
        compose.onNodeWithTag("chat-jump").assert(hasText("Today").not())
        compose.onNodeWithTag("sticky-day").assertDoesNotExist()
        chat.add(ChatRow.Ferb("r:new", "Your report is ready."))
        compose.waitForIdle()
        compose.onNodeWithTag("chat-jump").assert(hasText("New message"))
    }

    // decision: PHN-662, PHN-664
    @Test
    fun the_chat_tab_tapped_on_todays_chat_goes_to_the_end() {
        val chat = days(2, 30)
        scrollTo(chat.rows(), "t3")
        compose.onNodeWithText("Today line 30.").assertDoesNotExist()
        compose.runOnIdle { chat.vm.choose(PhoneTab.CHAT) }
        compose.waitForIdle()
        compose.onNodeWithText("Today line 30.").assertIsDisplayed()
        compose.onNodeWithTag("chat-jump").assertDoesNotExist()
    }

    // decision: PHN-662, PHN-663, PHN-698
    @Test
    fun what_the_way_back_shows_and_the_day_of_a_row() {
        val d = today.toEpochDay()
        assertEquals(ChatJump.NONE, chatJump(atEnd = true, unseen = true, shownDay = d - 1, endDay = d))
        assertEquals(ChatJump.NEW_MESSAGE, chatJump(atEnd = false, unseen = true, shownDay = d - 1, endDay = d))
        assertEquals(ChatJump.DAY, chatJump(atEnd = false, unseen = false, shownDay = d - 1, endDay = d))
        assertEquals(ChatJump.LATEST, chatJump(atEnd = false, unseen = false, shownDay = d, endDay = d))
        assertEquals(ChatJump.TODAY, chatJump(atEnd = true, unseen = false, shownDay = d - 1, endDay = d - 1, earlier = true))
        val rows = listOf(ChatRow.Older("older", OlderState.Complete), ChatRow.Day("day:1", d - 1), ChatRow.Ferb("a", "A"), ChatRow.Day("day:2", d), ChatRow.Ferb("b", "B"))
        assertEquals(listOf(d - 1, d - 1, d - 1, d, d), rows.indices.map { ChatModel.dayAt(rows, it) })
    }

    // decision: PHN-696
    @Test
    fun the_split_puts_each_row_in_its_place() {
        val d = today.toEpochDay()
        val rows = listOf(ChatRow.Older("older", OlderState.Complete), ChatRow.Day("day:1", d - 1), ChatRow.Ferb("a", "A"),
            ChatRow.Asking("q", "ap-1", null), ChatRow.Day("day:2", d), ChatRow.Ferb("b", "B"), ChatRow.Outbox("o:1", com.ferb.mobile.OutboxItem("1", "Later", "2026-01-02T10:00:00+00:00", "waiting", "link", null, false, emptyList())))
        val p = ChatModel.split(rows, d)
        assertEquals(listOf("older", "day:1", "a"), p.earlier.map { it.key })
        assertEquals("today's rows, then the open question of an earlier day", listOf("b", "o:1", "q"), p.today.map { it.key })
    }

    // decision: PHN-694, PHN-698
    @Test
    fun anywhere_in_the_earlier_days_the_way_back_says_today() {
        val chat = days(90, 0, earlier = true)
        compose.onNodeWithTag("chat-jump").assert(hasText("Today"))
        // his finger moves the list up a little: he is away from the end, in yesterday
        compose.onNodeWithTag("chat-list").performTouchInput { swipeDown(startY = centerY, endY = centerY + 300, durationMillis = 300) }
        compose.waitForIdle()
        compose.onNodeWithTag("sticky-day").assert(hasText("Yesterday"))
        for (key in listOf("y1", "y45", "y90")) {
            scrollTo(chat.rows(), key)
            compose.onNodeWithTag("chat-jump").assert(hasText("Today"))
        }
    }

    // decision: PHN-698
    @Test
    fun the_chat_tab_in_the_earlier_days_goes_back_to_today() {
        val chat = days(30, 3, earlier = true)
        compose.runOnIdle { chat.vm.choose(PhoneTab.CHAT) }
        compose.waitForIdle()
        assertFalse(chat.vm.earlier.value)
        compose.onNodeWithText("Today line 3.").assertIsDisplayed()
    }

    // decision: PHN-698
    @Test
    fun the_way_to_today_is_there_after_another_tab_and_with_the_keyboard_open() {
        val v = view(*(1..30).map { said("y$it", "Yesterday line $it.", today.minusDays(1), hour = 8) }.toTypedArray())
        val ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = ChatModel.rows(v), days = ChatModel.days(v, zone), chatDay = today.toEpochDay())
        val vm = vm()
        vm.openChat()
        var tab by mutableStateOf(PhoneTab.CHAT)
        var bottom by mutableStateOf(Phone.gestureArea)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    Main(vm, ui, tab, home = false, PhoneInsets(Phone.statusBar, bottom), labMoment(12.5), host)
                }
            }
        }
        compose.waitForIdle()
        compose.onNodeWithTag("chat-jump").assert(hasText("Today"))
        tab = PhoneTab.TASKS
        compose.waitForIdle()
        tab = PhoneTab.CHAT
        compose.waitForIdle()
        compose.onNodeWithTag("chat-jump").assert(hasText("Today")).assertIsDisplayed()
        bottom = 330.dp
        compose.waitForIdle()
        compose.onNodeWithTag("chat-jump").assert(hasText("Today")).assertIsDisplayed()
    }

    // ---- the page follows what was said, never the typing (kb PHN-696, PHN-697) ----

    /** The app's real Main on the view model's own state: its rows, its page, its tab. */
    private fun live(vm: FerbViewModel) {
        compose.setContent {
            val u by vm.ui.collectAsState()
            val h by vm.home.collectAsState()
            val t by vm.tab.collectAsState()
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) { Main(vm, u, t, h, PhoneInsets.Lab, labMoment(12.5), host) }
            }
        }
        compose.waitForIdle()
    }

    // decision: PHN-696
    @Test
    fun typing_on_an_empty_today_never_shows_an_earlier_row_and_his_send_is_alone_in_today() {
        phone.view = view(*(1..12).map { said("y$it", "Yesterday line $it.", today.minusDays(1)) }.toTypedArray())
        val vm = vm()
        session.unlocked()
        live(vm)
        assertTrue("today's home page", vm.home.value)
        compose.mainClock.autoAdvance = false
        val words = "Book the table"
        for (i in 1..words.length) {
            vm.typed(words.take(i))
            compose.mainClock.advanceTimeByFrame()
            compose.onNodeWithText("Yesterday line", substring = true).assertDoesNotExist()
            assertTrue("still today's home page at \"${words.take(i)}\"", vm.home.value)
        }
        compose.mainClock.autoAdvance = true
        vm.send()
        compose.waitForIdle()
        assertFalse("something said today: today's chat", vm.home.value)
        compose.onNodeWithText("Book the table").assertIsDisplayed()
        compose.onNodeWithText("Yesterday line", substring = true).assertDoesNotExist()
    }

    // decision: PHN-696
    @Test
    fun ferb_posting_today_opens_todays_chat_with_only_today_in_it() {
        val yesterday = (1..12).map { said("y$it", "Yesterday line $it.", today.minusDays(1)) }
        phone.view = view(*yesterday.toTypedArray())
        val vm = vm()
        session.unlocked()
        live(vm)
        assertTrue(vm.home.value)
        phone.view = view(*(yesterday + said("t1", "Your 9:00 call moved to 10.", today)).toTypedArray())
        changed()
        compose.waitForIdle()
        assertFalse(vm.home.value)
        compose.onNodeWithText("Your 9:00 call moved to 10.").assertIsDisplayed()
        compose.onNodeWithText("Yesterday line", substring = true).assertDoesNotExist()
    }

    // decision: PHN-697
    @Test
    fun a_day_change_while_the_app_is_open_waits_for_a_natural_moment() {
        nowMs = today.atTime(23, 58).atZone(zone).toInstant().toEpochMilli()
        phone.view = view(said("t1", "Late answer.", today, hour = 23))
        val vm = vm()
        session.unlocked()
        live(vm)
        assertFalse(vm.home.value)
        vm.typed("And one more")
        // midnight passes while he types, and something is read again: the page stays where it is
        nowMs = today.plusDays(1).atTime(0, 2).atZone(zone).toInstant().toEpochMilli()
        changed()
        compose.waitForIdle()
        assertFalse("not moved under him", vm.home.value)
        compose.onNodeWithText("Late answer.").assertIsDisplayed()
        assertEquals("And one more", vm.draft.value)
        // the Chat tab chosen: the day moves; nothing said on the new day yet: its home page
        compose.runOnIdle { vm.choose(PhoneTab.CHAT) }
        compose.waitForIdle()
        assertTrue("the new day's home page", vm.home.value)
        assertEquals("the words he typed stay in the box", "And one more", vm.draft.value)
    }
}
