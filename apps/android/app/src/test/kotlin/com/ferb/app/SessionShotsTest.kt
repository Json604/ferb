package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.click
import androidx.compose.ui.test.swipeDown
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import com.ferb.app.model.LiveView
import com.ferb.app.model.NewSessionUi
import com.ferb.app.model.SessionUi
import com.ferb.app.model.SessionSending
import com.ferb.app.model.SessionSendPhase
import com.ferb.app.model.TerminalPick
import com.ferb.app.model.SessionsUi
import com.ferb.app.ui.HandOverSheet
import com.ferb.app.ui.LivePage
import com.ferb.app.ui.NewSessionPage
import com.ferb.app.ui.PickTerminalSheet
import com.ferb.app.ui.SessionScreenPage
import com.ferb.app.ui.SessionYesSheet
import com.ferb.app.ui.SessionsTabPage
import com.ferb.design.Phone
import com.ferb.design.components.Connection
import com.ferb.design.components.LinkMark
import com.ferb.design.components.LocalConnection
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.SessionCard
import com.ferb.mobile.SessionChoice
import com.ferb.mobile.SessionYes
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The coding sessions' screens as the app draws them from their state (kb HAND-651..666), at the lab's Android size with the
 * top bar's connection control: the tab (sessions, off, none), one session (its conversation, the agent's question, the
 * message box; nothing can be sent), the Mac's question before a send, Hand to FERB, New session and its command, and
 * Watch. `:app:recordRoborazziDebug` rewrites them (look at them first).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class SessionShotsTest {
    private companion object {
        const val PHONE = "phone"
        /** The day the stand-in lines were said on (FakePhone.item), so a golden never depends on the day it runs. */
        val TODAY = com.ferb.app.model.ChatModel.chatDay(java.time.Instant.parse("2026-11-22T10:00:00Z").toEpochMilli(), java.time.ZoneId.systemDefault())
    }

    @get:Rule val compose = createComposeRule()

    private val options = RoborazziOptions(compareOptions = RoborazziOptions.CompareOptions(changeThreshold = 0.001f))
    private val connection = Connection(LinkMark.DIRECT, "Connected directly", null)

    private val sessions = listOf(
        FakePhone.agentSession("claude:s1", "Fix the flaky login test", "working"),
        FakePhone.agentSession("claude:s2", "Move the blog to the new theme", "waiting_for_you", host = "Visual Studio Code"),
        FakePhone.agentSession("codex:s3", "Add dark mode to settings", "working", withFerb = true),
        FakePhone.agentSession("codex:s4", "", "idle", host = ""),
    )

    private val open = SessionUi(
        "claude:s2", sessions[1],
        items = listOf(
            FakePhone.item(41, "user", "Move the blog to the new theme and keep the old URLs working"),
            FakePhone.item(42, "assistant", "I'll switch the theme in the config, then add redirects for the old post URLs."),
            FakePhone.item(43, "tool", "Read: config/site.toml", "Read"),
            FakePhone.item(44, "tool", "Edit: config/site.toml", "Edit"),
            FakePhone.item(45, "interrupted", ""),
            FakePhone.item(46, "user", "Go on, but check the build first"),
            FakePhone.item(47, "assistant", "The theme builds. Now I need to run the build to check the redirects."),
        ),
        more = true,
        cards = listOf(SessionCard("card-1", "permission", "Bash", "npm run build", "",
            listOf(SessionChoice("yes", "Yes"), SessionChoice("always", "Yes, and don't ask again"), SessionChoice("no", "No")))),
        loaded = true,
        draft = "Then deploy it to staging",
    )

    @Composable
    private fun Phone(sheet: @Composable () -> Unit = {}, page: @Composable () -> Unit) {
        Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
            CompositionLocalProvider(LocalConnection provides connection, com.ferb.app.ui.LocalSessionToday provides TODAY) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false, sheet = { sheet(); LabAndroidChrome() }) { page() }
            }
        }
    }

    private fun shoot(path: String) {
        compose.mainClock.autoAdvance = false
        compose.mainClock.advanceTimeByFrame()
        compose.onNodeWithTag(PHONE).captureRoboImage("src/test/screenshots/$path", roborazziOptions = options)
    }

    @Composable
    private fun tab(ui: SessionsUi) = SessionsTabPage(ui, PhoneInsets.Lab, null, {}, {})

    // decision: PHN-801
    @Test
    // decision: HAND-692, HAND-697, HAND-705
    fun the_sessions_tab_lists_each_with_its_place_and_state() {
        // a terminal running a program nobody listed: its window's words, "<program> · <folder> · in <app>"; and an app
        // open with no chat says so in one quiet line
        val terminal = FakePhone.agentSession("term:110", "", "idle", "Terminal").copy(agent = "terminal", agentName = "agy", program = "agy",
            folder = "/Users/owner/Desktop", folderName = "Desktop", window = "Desktop — agy — 120×30")
        compose.setContent { Phone { tab(SessionsUi(true, true, sessions + terminal, noChats = listOf("Cursor"))) } }
        shoot("sessions-list.png")
    }

    @Test
    fun sessions_off_says_what_turning_on_does() {
        compose.setContent { Phone { tab(SessionsUi(true, false, emptyList())) } }
        shoot("sessions-off.png")
    }

    @Test
    fun no_sessions_running() {
        compose.setContent { Phone { tab(SessionsUi(true, true, emptyList())) } }
        shoot("sessions-none.png")
    }

    // decision: HAND-663
    @Test
    fun one_session_its_conversation_question_and_message_box() {
        compose.setContent { Phone(sheet = { SessionScreenPage(open, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        shoot("session-open.png")
    }

    // decision: PHN-800
    @Test
    fun a_session_s_menu_holds_where_it_runs_and_its_actions() {
        compose.setContent {
            Phone(sheet = { SessionScreenPage(open.copy(session = sessions[1].copy(controlledHere = true), menu = true), emptySet(), PhoneInsets.Lab) }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        shoot("session-menu.png")
    }

    // decision: PHN-800
    @Test
    fun a_message_the_mac_refused_shows_as_not_sent_with_why_and_try_again() {
        val why = "FERB couldn't tell that the terminal in front in Visual Studio Code is this session's, so it typed nothing. " +
            "Click into the session's terminal once, then send again."
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(open.copy(cards = emptyList(), draft = "", outgoing = SessionSending("Then deploy it to staging", SessionSendPhase.NOT_SENT, Note(R.string.err_mac_said, why))),
                    emptySet(), PhoneInsets.Lab)
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        shoot("session-not-sent.png")
    }

    // decision: HAND-683
    @Test
    fun when_the_mac_cant_tell_the_terminal_he_chooses_one() {
        val pick = TerminalPick(
            "Then deploy it to staging",
            "FERB couldn't tell which terminal in Visual Studio Code is this session's, so it typed nothing. Choose it, and FERB types only there.",
            listOf(
                SessionChoice("front", "The terminal in front in Visual Studio Code now (Terminal 1, 2.1.330)"),
                SessionChoice("w7:0:Terminal 1, 2.1.330", "config.toml — blog · Terminal 1, 2.1.330"),
                SessionChoice("w7:1:Terminal 2, zsh", "config.toml — blog · Terminal 2, zsh"),
            ),
        )
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(open.copy(cards = emptyList(), draft = ""), emptySet(), PhoneInsets.Lab)
                PickTerminalSheet(pick, PhoneInsets.Lab, busy = false, onPick = {}, onNo = {})
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        shoot("session-pick-terminal.png")
    }

    @Test
    fun a_session_nothing_can_send_to_says_why() {
        val s = sessions[0].copy(canSend = false, noSend = "No app shows this session, so FERB can't type into it.", controlledHere = true)
        compose.setContent {
            Phone(sheet = { SessionScreenPage(open.copy(session = s, cards = emptyList(), more = false, draft = ""), emptySet(), PhoneInsets.Lab) }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        shoot("session-no-send.png")
    }

    // decision: GATE-622
    @Test
    fun the_macs_question_before_a_send_shows_the_exact_words() {
        val yes = SessionYes("Type this into Claude Code in Visual Studio Code? FERB will bring Visual Studio Code to the front to do it.",
            "Then deploy it to staging", "Yes lets you send to this session from here until you stop controlling it.")
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(open, emptySet(), PhoneInsets.Lab)
                SessionYesSheet(yes, "Yes, send it", PhoneInsets.Lab, busy = false, onYes = {}, onCancel = {})
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        shoot("session-confirm.png")
    }

    @Test
    fun hand_this_session_to_ferb() {
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(open, emptySet(), PhoneInsets.Lab)
                HandOverSheet(PhoneInsets.Lab, busy = false, onGo = {}, onClose = {})
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        shoot("session-hand.png")
    }

    // decision: HAND-691
    @Test
    fun a_new_session_is_a_folder_and_an_optional_command_with_no_agent_to_pick() {
        val n = NewSessionUi(false, listOf("/Users/owner/code/web-app", "/Users/owner/code/blog", "/Users/owner/code/notes-app"),
            folder = "/Users/owner/code/blog", text = "agy")
        compose.setContent { Phone(sheet = { NewSessionPage(n, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        shoot("session-new.png")
    }

    // decision: GATE-622
    @Test
    fun a_new_session_shows_the_exact_command_before_it_starts() {
        val n = NewSessionUi(false, listOf("/Users/owner/code/blog"), folder = "/Users/owner/code/blog",
            yes = SessionYes("Open a new Terminal window in blog and type this?", "agy",
                "FERB opens it in the background and types only this command, then Return. You can type more into it from here."))
        compose.setContent { Phone(sheet = { NewSessionPage(n, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        shoot("session-new-command.png")
    }

    // decision: PHN-797, PHN-454
    @Test
    fun watching_a_session_has_no_stop() {
        val bitmap = android.graphics.Bitmap.createBitmap(320, 200, android.graphics.Bitmap.Config.ARGB_8888).apply {
            val c = android.graphics.Canvas(this)
            c.drawColor(android.graphics.Color.rgb(30, 30, 34))
            val p = android.graphics.Paint().apply { color = android.graphics.Color.rgb(120, 200, 140) }
            for (i in 0 until 6) c.drawRect(16f, 20f + i * 26, 280f - i * 25, 32f + i * 26, p)
        }
        val view = LiveView("claude:s2", bitmap, "working", null)
        compose.setContent {
            Phone(sheet = { LivePage(view, PhoneInsets.Lab, {}, null, null, title = "Watching Move the blog to the new theme") }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        shoot("session-watch.png")
    }

    // ---- sending, the list as the main chat's, the page's touches (PHN-812, PHN-813, PHN-815) -------------------------

    // decision: PHN-812
    @Test
    fun a_message_on_its_way_shows_at_once_as_sending_and_the_send_button_turns() {
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(open.copy(cards = emptyList(), draft = "", outgoing = SessionSending("Then deploy it to staging", SessionSendPhase.SENDING, after = 47)),
                    setOf("session-send"), PhoneInsets.Lab)
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        shoot("session-sending.png")
    }

    // decision: PHN-816
    @Test
    fun a_session_opening_shows_the_waiting_look_never_an_error() {
        compose.setContent {
            Phone(sheet = { SessionScreenPage(SessionUi("claude:s2", sessions[1]), emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) }
        }
        compose.waitForIdle()
        compose.onNodeWithTag("session-waiting").assertExists()
        compose.onAllNodesWithText("FERB can't find that session.", substring = true).assertCountEquals(0)
        shoot("session-waiting.png")
    }

    // decision: PHN-816
    @Test
    fun the_mac_s_not_found_shows_as_its_words_without_the_waiting_look() {
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(SessionUi("claude:s2", sessions[1], problem = Note(R.string.err_mac_said, "FERB can't find that session.")), emptySet(), PhoneInsets.Lab)
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        compose.waitForIdle()
        compose.onAllNodesWithTag("session-waiting").assertCountEquals(0)
        compose.onNodeWithText("FERB can't find that session.", substring = true).assertExists()
    }

    /** A conversation longer than the screen, as the owner's (his screenshot of 2026-11-23). */
    private val long = open.copy(
        items = (1..24).map { i ->
            if (i % 3 == 0) FakePhone.item(i, "user", "Step $i: go on, and tell me what is left.")
            else FakePhone.item(i, "assistant", "Done with step $i. I read the config, changed the theme's layout and checked that " +
                "the old post URLs still answer. The build passes; next I'll look at the redirects and the feed.")
        },
        cards = emptyList(), draft = "", more = false,
    )

    // decision: PHN-813
    @Test
    fun scrolled_up_the_session_shows_the_main_chat_s_arrow_back_to_the_latest() {
        compose.setContent { Phone(sheet = { SessionScreenPage(long, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        compose.onAllNodesWithTag("session-jump").assertCountEquals(0)
        compose.onNodeWithTag("session-list").performTouchInput { swipeDown(startY = top + 40f, endY = bottom - 40f, durationMillis = 300) }
        compose.waitForIdle()
        compose.onNodeWithTag("session-jump").assertExists()
        shoot("session-scrolled-up.png")
    }

    // decision: PHN-813
    @Test
    fun the_arrow_takes_him_back_to_the_end_and_goes() {
        compose.setContent { Phone(sheet = { SessionScreenPage(long, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        compose.onNodeWithTag("session-list").performTouchInput { swipeDown(startY = top + 40f, endY = bottom - 40f, durationMillis = 300) }
        compose.waitForIdle()
        compose.onNodeWithTag("session-jump").performClick()
        compose.waitForIdle()
        compose.onAllNodesWithTag("session-jump").assertCountEquals(0)
        compose.onNodeWithText("Done with step 23", substring = true).assertIsDisplayed()
    }

    // decision: PHN-813
    @Test
    fun a_new_line_while_scrolled_up_says_new_message_and_doesn_t_move_the_list() {
        var ui by androidx.compose.runtime.mutableStateOf(long)
        compose.setContent { Phone(sheet = { SessionScreenPage(ui, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        compose.onNodeWithTag("session-list").performTouchInput { swipeDown(startY = top + 40f, endY = bottom - 40f, durationMillis = 300) }
        compose.waitForIdle()
        ui = long.copy(items = long.items + FakePhone.item(25, "assistant", "The redirects work."))
        compose.waitForIdle()
        compose.onNodeWithText("New message").assertExists()
        compose.onAllNodesWithText("The redirects work.").assertCountEquals(0)
    }

    /** Taps beside and under the message box: before this fix they reached the Sessions tab's New session under the page. */
    // decision: PHN-815
    @Test
    fun taps_beside_and_under_the_message_box_never_reach_new_session_under_the_page() {
        var newTapped = 0
        var covered by androidx.compose.runtime.mutableStateOf(true)
        compose.setContent {
            Phone(sheet = { if (covered) SessionScreenPage(open.copy(cards = emptyList()), emptySet(), PhoneInsets.Lab) }) {
                SessionsTabPage(SessionsUi(true, true, sessions), PhoneInsets.Lab, null, {}, {}, onNew = { newTapped++ })
            }
        }
        compose.waitForIdle()
        val box = compose.onNodeWithTag("session-composer").fetchSemanticsNode().boundsInRoot
        // the message box's own rounded ends and its foot: on the box, but beside its field and its send button
        val points = listOf(
            androidx.compose.ui.geometry.Offset(box.left + 6f, box.center.y),
            androidx.compose.ui.geometry.Offset(box.right - 3f, box.center.y),
            androidx.compose.ui.geometry.Offset(box.center.x, box.bottom - 3f),
        )
        // each point is over the tab's New session button (the page under the session)
        val pill = compose.onNodeWithText("New session").fetchSemanticsNode().boundsInRoot
        for (p in points) org.junit.Assert.assertTrue("$p over New session ($pill)", pill.contains(p))
        for (p in points) compose.onNodeWithTag(PHONE).performTouchInput { click(p) }
        compose.waitForIdle()
        org.junit.Assert.assertEquals("no tap reached New session under the session", 0, newTapped)
        // the same tap with the session closed does reach it: the points were the button's
        covered = false
        compose.waitForIdle()
        compose.onNodeWithTag(PHONE).performTouchInput { click(points[0]) }
        compose.waitForIdle()
        org.junit.Assert.assertEquals(1, newTapped)
    }

    @Composable
    private fun Sized(sheet: @Composable () -> Unit, page: @Composable () -> Unit) {
        Box(Modifier.fillMaxSize().testTag(PHONE)) {
            CompositionLocalProvider(LocalConnection provides connection, com.ferb.app.ui.LocalSessionToday provides TODAY) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false, sheet = { sheet(); LabAndroidChrome() }) { page() }
            }
        }
    }

    private fun conversationAt(path: String) {
        compose.setContent {
            Sized(sheet = {
                SessionScreenPage(long.copy(session = sessions[0], outgoing = SessionSending("Then deploy it to staging", SessionSendPhase.SENDING, after = 24)),
                    setOf("session-send"), PhoneInsets.Lab)
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        shoot(path)
    }

    // decision: PHN-815, PHN-813
    @Test @Config(qualifiers = "w360dp-h740dp-port-mdpi") fun the_conversation_at_360() = conversationAt("session-conversation-360.png")

    // decision: PHN-815, PHN-813
    @Test @Config(qualifiers = "w412dp-h860dp-port-mdpi") fun the_conversation_at_412() = conversationAt("session-conversation-412.png")

    // ---- the live fixes (owner 2026-11-23: HAND-702, HAND-703, PHN-821) -----------------------------------------------

    /** A reply modelled on the owner's reference (Cursor): a thought, words, a step closed, a table, a list, code, its time. */
    private val rich = SessionUi(
        "claude:s5", FakePhone.agentSession("claude:s5", "Look through the export folder", "idle"),
        items = listOf(
            FakePhone.item(1, "user", "What's in this folder? I forgot."),
            FakePhone.item(2, "assistant", "", parts = listOf(
                FakePhone.part("thinking", label = "Thought 1s"),
                FakePhone.part("text", "I'll look through this folder and tell you what it is."),
                FakePhone.part("tool", "  Length      Date    Time    Name\n---------  ---------- -----   ----\n    19357  01-01-1980 00:00   projects/a1b2.json",
                    "List zip sizes and contents", "ls, echo, unzip, head"),
                FakePhone.part("thinking", label = "Thought 4s"),
                FakePhone.part("text", "This is a **data export**, the bundle you get when you download an account. Nothing in it is unzipped yet.\n\n" +
                    "| File | What's inside |\n|---|---|\n| `chats-000.zip` (3 MB) | 101 chats from the last two years |\n" +
                    "| `pages-000.zip` (425 MB) | Pages, their versions and comments |\n| `settings-000.zip` | The account's settings |\n\n" +
                    "- Six zips in all\n- Saved on the Desktop\n\nRun `unzip -l` on one to see what it holds."),
                FakePhone.part("meta", "Just now"),
            )),
        ),
        loaded = true,
    )

    private fun richAt(path: String) {
        compose.setContent { Sized(sheet = { SessionScreenPage(rich, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        compose.onNodeWithTag("session-reply").assertExists()
        compose.onNodeWithText("Thought 1s").assertExists()
        compose.onNodeWithText("List zip sizes and contents · ls, echo, unzip, head").assertExists()
        compose.onAllNodesWithTag("session-step-output").assertCountEquals(0)
        compose.onNodeWithTag("session-meta").assertExists()
        shoot(path)
    }

    // decision: HAND-703
    @Test @Config(qualifiers = "w360dp-h860dp-port-mdpi") fun a_rich_reply_at_360() = richAt("session-rich-360.png")

    // decision: HAND-703
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi") fun a_rich_reply_at_412() = richAt("session-rich-412.png")

    // decision: HAND-703
    @Test
    fun a_step_opens_on_a_tap_to_its_output_and_one_with_nothing_to_open_has_no_chevron() {
        val ui = rich.copy(items = listOf(FakePhone.item(2, "assistant", "", parts = listOf(
            FakePhone.part("tool", "total 3\nchats-000.zip", "List zip sizes and contents", "ls"),
            FakePhone.part("tool", "", "Explored", "2 searches"),
        ))))
        compose.setContent { Phone(sheet = { SessionScreenPage(ui, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        compose.onAllNodesWithTag("session-step-output").assertCountEquals(0)
        compose.onNodeWithText("List zip sizes and contents · ls").performClick()
        compose.waitForIdle()
        compose.onNodeWithTag("session-step-output").assertExists()
        compose.onNodeWithText("Explored · 2 searches").performClick()
        compose.waitForIdle()
        compose.onAllNodesWithTag("session-step-output").assertCountEquals(1)
    }

    private val handed = sessions[1].copy(withFerb = true, goal = "Keep going until the build passes", ferbTask = "task-9",
        ferbNote = "FERB is on it: \u201cKeep going until the build passes\u201d")

    private fun takeBackAt(path: String) {
        compose.setContent {
            Sized(sheet = { SessionScreenPage(open.copy(session = handed, cards = emptyList(), menu = true, draft = ""), emptySet(), PhoneInsets.Lab) }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        compose.waitForIdle()
        compose.onNodeWithText("Take it back").assertExists()
        compose.onAllNodesWithText("Hand to FERB").assertCountEquals(0)
        shoot(path)
    }

    // decision: HAND-702
    @Test @Config(qualifiers = "w360dp-h740dp-port-mdpi") fun the_menu_with_take_it_back_at_360() = takeBackAt("session-take-back-360.png")

    // decision: HAND-702
    @Test @Config(qualifiers = "w412dp-h860dp-port-mdpi") fun the_menu_with_take_it_back_at_412() = takeBackAt("session-take-back-412.png")

    // decision: HAND-702
    @Test
    fun take_it_back_keeps_the_menu_open_and_turns_while_the_mac_answers() {
        var taken = 0
        var closed = 0
        var busy by androidx.compose.runtime.mutableStateOf(emptySet<String>())
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(open.copy(session = handed, cards = emptyList(), menu = true), busy, PhoneInsets.Lab,
                    onTakeBack = { taken++ }, onMenu = { if (!it) closed++ })
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        compose.waitForIdle()
        compose.onNodeWithTag("session-take-back").performClick()
        compose.waitForIdle()
        org.junit.Assert.assertEquals(1, taken)
        org.junit.Assert.assertEquals("the menu stays open until the Mac answers", 0, closed)
        busy = setOf("session-take-back")
        compose.waitForIdle()
        compose.onNodeWithText("Taking it back…").assertExists()
        compose.onNodeWithTag("session-take-back").performClick()
        compose.waitForIdle()
        org.junit.Assert.assertEquals("no second tap while it goes", 1, taken)
    }

    // decision: HAND-702
    @Test
    fun while_ferb_is_starting_the_menu_says_so_and_offers_nothing_to_tap() {
        var handTaps = 0
        val starting = sessions[1].copy(withFerb = true, goal = "Keep going until the build passes")
        compose.setContent {
            Phone(sheet = { SessionScreenPage(open.copy(session = starting, cards = emptyList(), menu = true), emptySet(), PhoneInsets.Lab, onHand = { handTaps++ }) }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        compose.waitForIdle()
        compose.onAllNodesWithText("Hand to FERB").assertCountEquals(0)
        compose.onAllNodesWithText("Take it back").assertCountEquals(0)
        compose.onNodeWithTag("session-ferb-starting").performClick()
        compose.waitForIdle()
        org.junit.Assert.assertEquals(0, handTaps)
        compose.onNodeWithText("FERB is starting…").assertExists()
    }

    private fun noteAt(path: String) {
        compose.setContent {
            Sized(sheet = { SessionScreenPage(open.copy(session = handed, cards = emptyList(), draft = "", more = false), emptySet(), PhoneInsets.Lab) }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        compose.waitForIdle()
        compose.onNodeWithText("FERB is on it: \u201cKeep going until the build passes\u201d").assertExists()
        shoot(path)
    }

    // decision: HAND-702
    @Test @Config(qualifiers = "w360dp-h740dp-port-mdpi") fun ferbs_word_on_the_hand_over_at_360() = noteAt("session-ferb-note-360.png")

    // decision: HAND-702
    @Test @Config(qualifiers = "w412dp-h860dp-port-mdpi") fun ferbs_word_on_the_hand_over_at_412() = noteAt("session-ferb-note-412.png")

    // decision: HAND-702
    @Test
    fun ferbs_word_sits_under_the_header_when_there_is_no_message_box() {
        val s = handed.copy(canSend = false, noSend = "No app shows this session, so FERB can't type into it.",
            ferbNote = "FERB finished. The session is yours again.")
        compose.setContent { Phone(sheet = { SessionScreenPage(open.copy(session = s, cards = emptyList()), emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        val note = compose.onNodeWithTag("session-ferb-note").fetchSemanticsNode().boundsInRoot
        val header = compose.onNodeWithTag("session-header").fetchSemanticsNode().boundsInRoot
        val list = compose.onNodeWithTag("session-list").fetchSemanticsNode().boundsInRoot
        org.junit.Assert.assertTrue("under the header ($note, $header)", note.top >= header.bottom - 1f && note.bottom <= list.top + 1f)
    }

    // decision: HAND-702
    @Test
    fun hand_over_turns_while_it_goes_and_a_refusal_shows_the_macs_reason_under_the_button() {
        var busy by androidx.compose.runtime.mutableStateOf(true)
        var problem by androidx.compose.runtime.mutableStateOf<String?>(null)
        compose.setContent {
            Phone(sheet = {
                SessionScreenPage(open, emptySet(), PhoneInsets.Lab)
                HandOverSheet(PhoneInsets.Lab, busy = busy, onGo = {}, onClose = {}, problem = problem, initial = "Keep going until the build passes")
            }) { tab(SessionsUi(true, true, sessions)) }
        }
        compose.waitForIdle()
        compose.onNodeWithText("Handing it to FERB…").assertExists()
        busy = false
        problem = "FERB can't take this session: it is not running."
        compose.waitForIdle()
        compose.onNodeWithText("Hand it over").assertExists()
        compose.onNodeWithText("FERB can't take this session: it is not running.").assertExists()
        compose.onNodeWithText("Keep going until the build passes").assertExists()
    }

    private val folders = listOf("/Users/you/code/web-app", "/Users/you/code/blog", "/Users/you/code/notes-app")

    private fun startingAt(path: String) {
        val n = NewSessionUi(false, folders, folder = "/Users/you/code/blog", text = "agy")
        compose.setContent { Sized(sheet = { NewSessionPage(n, setOf("session-prepare"), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        compose.onNodeWithText("Opening a terminal…").assertExists()
        shoot(path)
    }

    // decision: PHN-822
    @Test @Config(qualifiers = "w360dp-h740dp-port-mdpi") fun a_new_session_starting_at_360() = startingAt("session-new-starting-360.png")

    // decision: PHN-822
    @Test @Config(qualifiers = "w412dp-h860dp-port-mdpi") fun a_new_session_starting_at_412() = startingAt("session-new-starting-412.png")

    // decision: PHN-822
    @Test
    fun while_it_starts_nothing_on_the_form_takes_a_tap_and_a_refusal_shows_under_start() {
        var picked = 0
        var next = 0
        var busy by androidx.compose.runtime.mutableStateOf(setOf("session-prepare"))
        var n by androidx.compose.runtime.mutableStateOf(NewSessionUi(false, folders, folder = "/Users/you/code/blog"))
        compose.setContent {
            Phone(sheet = { NewSessionPage(n, busy, PhoneInsets.Lab, onFolder = { picked++ }, onNext = { next++ }, onBrowse = { picked++ }) }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        compose.waitForIdle()
        compose.onNodeWithText("notes-app").performClick()
        compose.onNodeWithText("Another folder…").performClick()
        compose.onNodeWithTag("new-start").performClick()
        compose.waitForIdle()
        org.junit.Assert.assertEquals(0 to 0, picked to next)
        busy = emptySet()
        n = n.copy(problem = Note(R.string.err_mac_said, "That folder isn't there any more. Choose another one."))
        compose.waitForIdle()
        compose.onNodeWithText("That folder isn't there any more. Choose another one.").assertExists()
        compose.onNodeWithText("Start").assertExists()
    }

    private val picker = NewSessionUi(false, folders, folder = "/Users/you/code/blog", browse = com.ferb.app.model.FolderPickUi(listOf(
        com.ferb.mobile.Folder("/Users/you", null, false, emptyList(), 0u, false, 0u, null),
        com.ferb.mobile.Folder("/Users/you/code", "/Users/you", false, listOf(
            com.ferb.mobile.FileEntry("blog", "/Users/you/code/blog", true, false, 0u, 0, false),
            com.ferb.mobile.FileEntry("README.md", "/Users/you/code/README.md", false, false, 2_000u, 1_767_348_000, false),
            com.ferb.mobile.FileEntry("site", "/Users/you/code/site", true, false, 0u, 0, false),
            com.ferb.mobile.FileEntry("tools", "/Users/you/code/tools", true, false, 0u, 0, false),
            com.ferb.mobile.FileEntry("web-app", "/Users/you/code/web-app", true, false, 0u, 0, false),
        ), 5u, false, 0u, null),
    )))

    private fun pickerAt(path: String) {
        compose.setContent { Sized(sheet = { NewSessionPage(picker, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        compose.waitForIdle()
        compose.onAllNodesWithText("README.md").assertCountEquals(0)
        compose.onNodeWithText("Use this folder").assertExists()
        shoot(path)
    }

    // decision: PHN-821
    @Test @Config(qualifiers = "w360dp-h740dp-port-mdpi") fun the_folder_picker_at_360() = pickerAt("session-folder-picker-360.png")

    // decision: PHN-821
    @Test @Config(qualifiers = "w412dp-h860dp-port-mdpi") fun the_folder_picker_at_412() = pickerAt("session-folder-picker-412.png")

    // decision: PHN-821
    @Test
    fun in_the_picker_a_folder_opens_on_a_tap_and_use_this_folder_chooses_the_one_showing() {
        val opened = ArrayList<String>()
        var used = 0
        compose.setContent {
            Phone(sheet = { NewSessionPage(picker, emptySet(), PhoneInsets.Lab, onBrowseOpen = { p, _ -> opened += p }, onBrowseUse = { used++ }) }) {
                tab(SessionsUi(true, true, sessions))
            }
        }
        compose.waitForIdle()
        compose.onNodeWithText("site").performClick()
        compose.onNodeWithText("Use this folder").performClick()
        compose.waitForIdle()
        org.junit.Assert.assertEquals(listOf("/Users/you/code/site"), opened)
        org.junit.Assert.assertEquals(1, used)
        compose.onNodeWithTag("browse-path").assertExists()
    }

    // decision: PHN-822
    @Test
    fun a_new_terminal_on_its_way_shows_one_quiet_line_in_the_list() {
        compose.setContent { Phone { tab(SessionsUi(true, true, sessions, coming = setOf("claude:s1"))) } }
        compose.waitForIdle()
        compose.onNodeWithText("Your new terminal will show here in a moment.").assertExists()
    }
}
