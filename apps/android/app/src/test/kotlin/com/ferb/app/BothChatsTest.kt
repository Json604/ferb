package com.ferb.app

import android.app.Application
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeDown
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.design.Phone
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.Change
import com.ferb.mobile.FerbException
import com.ferb.mobile.Item
import com.ferb.mobile.NoticeItem
import com.ferb.mobile.OlderState
import com.ferb.mobile.OwnerWords
import com.ferb.mobile.TranscriptView
import com.ferb.mobile.TurnItem
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.RoborazziTaskType
import com.github.takahirom.roborazzi.captureRoboImage
import kotlinx.coroutines.CompletableDeferred
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
import org.robolectric.annotation.GraphicsMode
import java.io.File
import java.time.LocalDate
import java.time.ZoneId

/**
 * The temporary chat is the chat with another data source (the owner, APP-1126: "i told you to match normal chat and
 * temp chat UI, why are they even diff???"). Each case runs on both chats through the app's real Main: a row that
 * arrives fades in, scrolled up a new row brings the pill and the way back to the latest, and his message shows at once
 * as his outbox row at the end, the list going to it. Before, the temporary chat was a copy of the chat's screen with
 * none of the three.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class BothChatsTest {
    @get:Rule val compose = createComposeRule()

    /** Where the chat's rows come from: the kept chat, or the temporary chat. Everything else is one path. */
    enum class Source { SAVED, TEMPORARY }

    private val phone = FakePhone()
    private val scheduler = TestCoroutineScheduler()
    private val zone = ZoneId.systemDefault()
    private val today = LocalDate.now(zone)
    private lateinit var session: Session
    private val root: File = java.nio.file.Files.createTempDirectory("ferb-both-chats").toFile()
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val base = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))

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
        LocalFiles(root) { null }, say = { "" }, account = testAccount())

    private fun said(id: String, text: String): Item {
        val at = today.atTime(9, 0).atZone(zone).toEpochSecond() - 978_307_200.0
        return Item.Notice(NoticeItem(id, at, false, id, null, text, emptyList(), null))
    }

    /** His own message, as the library folds the Mac's entry. */
    private fun his(id: String, text: String): Item = Item.Turn(TurnItem(
        entry = id, at = today.atTime(9, 0).atZone(zone).toEpochSecond() - 978_307_200.0, legacy = false, clientId = null,
        requestId = "temporary-$id", taskId = null, from = "owner", taskName = null, status = "done", statusInferred = false,
        notice = null, mergedInto = null, question = null, answeredBelow = false, answer = null, card = null, checklist = null,
        firstReply = null, parts = emptyList(), goal = null, words = OwnerWords(text, emptyList(), emptyList(), emptyList(), null)))

    private fun view(conversation: String, items: List<Item>) =
        TranscriptView(conversation, items, emptyList(), emptyList(), emptyList(), OlderState.Complete)

    /** [items] as [source]'s rows on the screen's state. */
    private fun UiState.with(source: Source, items: List<Item>): UiState = when (source) {
        Source.SAVED -> view("c1", items).let { v -> copy(conversation = "c1", rows = ChatModel.rows(v), days = ChatModel.days(v, zone), chatDay = today.toEpochDay()) }
        Source.TEMPORARY -> copy(conversation = "c1", chatDay = today.toEpochDay(), temporary = TemporaryUi("T1", ChatModel.rows(view("temporary:T1", items))))
    }

    /** The real Main on the chat tab, drawing whatever [ui] says now. */
    private fun show(vm: FerbViewModel, ui: () -> UiState) {
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    Main(vm, ui(), PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host)
                }
            }
        }
        compose.waitForIdle()
    }

    private fun Bitmap.same(other: Bitmap): Boolean {
        if (width != other.width || height != other.height) return false
        val a = IntArray(width * height).also { getPixels(it, 0, width, 0, 0, width, height) }
        val b = IntArray(width * height).also { other.getPixels(it, 0, width, 0, 0, width, height) }
        return a.contentEquals(b)
    }

    /** The row's pixels now (written by Roborazzi to a file of the build, as the app's other pixel checks do). */
    private fun picture(text: String): Bitmap {
        val f = File(root, "pictures/${java.util.UUID.randomUUID()}.png")
        compose.onNodeWithText(text).captureRoboImage(f.path, roborazziOptions = RoborazziOptions(taskType = RoborazziTaskType.Record))
        return BitmapFactory.decodeFile(f.path)
    }

    // ---- a row that arrives fades in where it lands; one there already is drawn at once (kb PHN-624, PHN-670) ----

    private fun aRowThatArrivesFadesIn(source: Source) {
        var ui by mutableStateOf(base.with(source, listOf(said("e1", "Already here."))))
        show(vm()) { ui }
        compose.mainClock.autoAdvance = false
        ui = ui.with(source, listOf(said("e1", "Already here."), said("e2", "Just arrived.")))
        Snapshot.sendApplyNotifications()
        compose.mainClock.advanceTimeByFrame()
        compose.mainClock.advanceTimeByFrame()
        val arriving = picture("Just arrived.")
        val there = picture("Already here.")
        compose.mainClock.advanceTimeBy(3_000)
        assertFalse("$source: the new row was still fading in", arriving.same(picture("Just arrived.")))
        assertTrue("$source: the row already there never faded", there.same(picture("Already here.")))
    }

    // decision: APP-1126, PHN-837, PHN-624
    @Test fun a_row_that_arrives_fades_in_in_the_chat() = aRowThatArrivesFadesIn(Source.SAVED)

    // decision: APP-1126, PHN-837, PHN-624
    @Test fun a_row_that_arrives_fades_in_in_the_temporary_chat() = aRowThatArrivesFadesIn(Source.TEMPORARY)

    // ---- scrolled up, what arrives brings the pill and the way back to the latest (kb PHN-628, PHN-662) ----

    private fun scrolledUpANewRowBringsThePill(source: Source) {
        val many = (1..30).map { said("e$it", "Message number $it.") }
        var ui by mutableStateOf(base.with(source, many))
        show(vm()) { ui }
        compose.onNodeWithText("Message number 30.").assertIsDisplayed()
        compose.onNodeWithTag("chat-jump").assertDoesNotExist()
        compose.onNodeWithTag("chat-list").performTouchInput { swipeDown(startY = top + 10, endY = bottom, durationMillis = 300) }
        compose.waitForIdle()
        Snapshot.sendApplyNotifications()
        ui = ui.with(source, many + said("e31", "One more thing."))
        compose.waitForIdle()
        compose.onNodeWithText("New message").assertIsDisplayed()
        compose.onNodeWithTag("chat-jump").assertIsDisplayed()
    }

    // decision: APP-1126, PHN-837, PHN-628, PHN-662
    @Test fun scrolled_up_a_new_row_brings_the_pill_in_the_chat() = scrolledUpANewRowBringsThePill(Source.SAVED)

    // decision: APP-1126, PHN-837, PHN-628, PHN-662
    @Test fun scrolled_up_a_new_row_brings_the_pill_in_the_temporary_chat() = scrolledUpANewRowBringsThePill(Source.TEMPORARY)

    // ---- his message: at once, his outbox row at the end, the list going to it (kb PHN-401, PHN-628) ----

    private fun hisMessageIsHisOutboxRowAtTheEnd(source: Source): FerbViewModel {
        val many = (1..30).map { said("e$it", "Message number $it.") }
        when (source) {
            Source.SAVED -> phone.view = view("c1", many)
            Source.TEMPORARY -> { phone.temporaryOn = true; phone.temporaryItems = many; phone.temporaryGate = CompletableDeferred() }
        }
        val vm = vm()
        session.unlocked()
        compose.setContent {
            val ui by vm.ui.collectAsState()
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    Main(vm, ui.copy(gate = Gate.MAIN), PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host)
                }
            }
        }
        compose.waitForIdle()
        compose.onNodeWithTag("chat-list").performScrollToIndex(0)
        compose.waitForIdle()
        vm.typed("what is 2+2")
        vm.send()
        compose.waitForIdle()
        val rows = if (source == Source.SAVED) vm.ui.value.rows else vm.ui.value.temporary!!.rows
        val last = rows.last()
        assertTrue("$source: his message is his outbox row at the end: $last", last is ChatRow.Outbox && last.item.text == "what is 2+2")
        compose.onNodeWithText("what is 2+2").assertIsDisplayed()
        return vm
    }

    // decision: APP-1126, PHN-837, PHN-401, PHN-628
    @Test fun his_message_is_his_outbox_row_at_the_end_in_the_chat() { hisMessageIsHisOutboxRowAtTheEnd(Source.SAVED) }

    // decision: APP-1126, PHN-837, PHN-628
    @Test fun his_message_is_his_outbox_row_at_the_end_in_the_temporary_chat() {
        val vm = hisMessageIsHisOutboxRowAtTheEnd(Source.TEMPORARY)
        compose.onNodeWithText("Sending…").assertIsDisplayed()
        assertTrue("never the phone's outbox", phone.outbox.isEmpty())
        // the Mac's chat holds it before the send's answer comes (its rev moved): one row of it, the Mac's
        val many = (1..30).map { said("e$it", "Message number $it.") }
        phone.temporaryItems = many + his("m1", "what is 2+2")
        phone.changes.tryEmit(Change.Temporary)
        compose.waitForIdle()
        val his = vm.ui.value.temporary!!.rows.filter { (it is ChatRow.Owner && it.text == "what is 2+2") || it is ChatRow.Outbox }
        assertEquals(listOf("m1"), his.map { it.key })
        phone.temporaryGate!!.complete(Unit)
        compose.waitForIdle()
        compose.onNodeWithText("Sending…").assertDoesNotExist()
        compose.onNodeWithText("what is 2+2").assertIsDisplayed()
    }

    // decision: APP-1126, PHN-837
    @Test
    fun a_temporary_send_that_fails_takes_its_row_away_and_keeps_the_words() {
        phone.temporaryOn = true
        phone.temporaryGate = CompletableDeferred()
        phone.temporaryRefusal = Outcome.Failed(FerbException.Offline("FERB's Mac can't be reached now.", "mac_unreachable"))
        val vm = vm()
        session.unlocked()
        vm.typed("again")
        vm.send()
        assertTrue(vm.ui.value.temporary!!.rows.last() is ChatRow.Outbox)
        assertEquals("", vm.draft.value)
        phone.temporaryGate!!.complete(Unit)
        assertTrue("its row went", vm.ui.value.temporary!!.rows.none { it is ChatRow.Outbox })
        assertEquals("again", vm.draft.value)
    }
}
