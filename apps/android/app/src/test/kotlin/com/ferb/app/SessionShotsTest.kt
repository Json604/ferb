package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import com.ferb.app.model.LiveView
import com.ferb.app.model.NewSessionUi
import com.ferb.app.model.SessionUi
import com.ferb.app.model.Unsent
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
    private companion object { const val PHONE = "phone" }

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
            CompositionLocalProvider(LocalConnection provides connection) {
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
    fun the_sessions_tab_lists_each_with_its_place_and_state() {
        compose.setContent { Phone { tab(SessionsUi(true, true, sessions)) } }
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
                SessionScreenPage(open.copy(cards = emptyList(), draft = "", unsent = Unsent("Then deploy it to staging", Note(R.string.err_mac_said, why))),
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

    @Test
    fun a_new_session_its_agent_folder_and_first_words() {
        val n = NewSessionUi(false, listOf("/Users/owner/code/web-app", "/Users/owner/code/blog", "/Users/owner/code/notes-app"),
            folder = "/Users/owner/code/blog", text = "Move the blog to the new theme")
        compose.setContent { Phone(sheet = { NewSessionPage(n, emptySet(), PhoneInsets.Lab) }) { tab(SessionsUi(true, true, sessions)) } }
        shoot("session-new.png")
    }

    // decision: GATE-622
    @Test
    fun a_new_session_shows_the_exact_command_before_it_starts() {
        val n = NewSessionUi(false, listOf("/Users/owner/code/blog"), folder = "/Users/owner/code/blog",
            yes = SessionYes("Open Terminal on your Mac and run this?", "cd /Users/owner/code/blog && claude 'Move the blog to the new theme'", ""))
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
}
