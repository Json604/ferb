package com.ferb.app

import android.app.Application
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeDown
import androidx.compose.ui.test.swipeUp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.AnswerUi
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Present
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import java.io.File
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Scrolling a long chat of the shapes FERB writes (kb PHN-716): the engine's recorded conversations, and long Markdown
 * answers of one to three screens (headings, lists, links, code, tables) between the owner's messages and day starts.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class ScrollWorkTest {
    @get:Rule val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }

    @After fun off() = Runs.uninstall()

    companion object {
        /** A long answer of FERB's: [screens] screens of Markdown, as its replies come. */
        fun markdown(i: Int, screens: Int): String = buildString {
            append("## Plan for request $i\n\n")
            repeat(screens * 3) { p ->
                append("FERB looked through **the folder** and found what you asked for in paragraph $p of answer $i. ")
                append("It compared the two versions, kept the newer one and wrote the summary to [the notes](https://example.com/n/$i/$p). ")
                append("Nothing else on the Mac was changed, and the backup from this morning is still there.\n\n")
                if (p % 3 == 1) append("- first point $p\n- second point with `code $p`\n- third point, a little longer than the others\n\n")
                if (p % 4 == 2) append("```\nls -la ~/Documents/project-$i\n```\n\n")
            }
            if (i % 5 == 0) append("| Name | Size | Kind |\n|---|---|---|\n| a.txt | 1 KB | text |\n| b.png | 2 MB | image |\n\n")
        }

        /** About [n] rows: the recorded conversations' own rows, and long answers, messages and day starts between them. */
        fun mixedRows(n: Int): List<ChatRow> {
            val recorded = Fixtures.conversations().flatMapIndexed { c: Int, conv: FixtureConversation ->
                runCatching { ChatModel.rows(FixtureMac.transcript(FixtureMac(conv))) }.getOrDefault(emptyList())
                    .filter { it !is ChatRow.Older && it !is ChatRow.Typing && it !is ChatRow.Day }
                    .map { prefixed(it, "x$c-") }
            }
            val out = ArrayList<ChatRow>()
            var r = 0
            var i = 0
            while (out.size < n) {
                // day starts come from the days map (ChatModel.withDays), not as rows here
                out += ChatRow.Owner("o$i", "Can you look at the project folder and tell me what changed since Monday? ($i)", emptyList(), null)
                val blocks = Present.rich(markdown(i, 1 + i % 3))
                out += ChatRow.Answer("a$i", AnswerUi(true, blocks, "", null, emptyList(), emptyList(), emptyList(), emptyList(), emptyList(), emptyList(), emptyList(), null))
                if (recorded.isNotEmpty()) repeat(2) { out += recorded[r++ % recorded.size].let { prefixed(it, "r" + r + "-") } }
                i++
            }
            return out.take(n)
        }

        private fun prefixed(row: ChatRow, p: String): ChatRow = when (row) {
            is ChatRow.Owner -> row.copy(key = p + row.key)
            is ChatRow.Ferb -> row.copy(key = p + row.key)
            is ChatRow.Line -> row.copy(key = p + row.key)
            is ChatRow.Rich -> row.copy(key = p + row.key)
            is ChatRow.Answer -> row.copy(key = p + row.key)
            is ChatRow.Notice -> row.copy(key = p + row.key)
            is ChatRow.Results -> row.copy(key = p + row.key)
            is ChatRow.Task -> row.copy(key = p + row.key)
            is ChatRow.Cards -> row.copy(key = p + row.key)
            is ChatRow.Asking -> row.copy(key = p + row.key)
            is ChatRow.Outbox -> row.copy(key = p + row.key)
            is ChatRow.WithFiles -> row.copy(key = p + row.key)
            else -> row
        }
    }

    private fun open(rows: List<ChatRow>) {
        val ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = rows)
        val vm = FerbViewModel(FakePhone(), Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-scroll").toFile()) { null })
        compose.setContent {
            val moment = remember { labMoment(12.5) }
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false) { Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, moment, host) }
            }
        }
        compose.waitForIdle()
    }

    private fun fling(times: Int, down: Boolean) = repeat(times) {
        compose.onNodeWithTag("chat-list").performTouchInput { if (down) swipeDown(durationMillis = 120) else swipeUp(durationMillis = 120) }
        compose.waitForIdle()
    }

    /**
     * Every row is composed once as it comes into view, and nothing around the list recomposes while it flings: the
     * chat, its message box and the follow ran once per row passing the top before (478 times for 493 rows).
     */
    // decision: PHN-710, PHN-716
    @Test
    fun a_fling_through_500_mixed_rows_composes_each_row_once_and_not_the_chat() {
        val rows = mixedRows(500)
        open(rows)
        Runs.install()
        fling(40, down = true)
        val composed = Runs.of("com.ferb.app.ui.ChatRowView")
        assertTrue("40 flings composed $composed rows of ${rows.size}", composed in 100..rows.size)
        assertTrue("the chat recomposed ${Runs.of("com.ferb.app.ui.Chat")} times while it flung:\n${Runs.top()}", Runs.of("com.ferb.app.ui.Chat") <= 5)
        assertTrue("the message box recomposed ${Runs.of("com.ferb.app.ui.Composer")} times", Runs.of("com.ferb.app.ui.Composer") <= 5)
    }

    /**
     * The budget for a message coming into view (counted, not timed): its words built once and its fold measured once
     * the first time; a message scrolling back in builds and measures nothing (RichCache), so a fling back over what he
     * already read costs only the rows' own layout.
     */
    // decision: PHN-716
    @Test
    fun a_message_scrolling_back_in_is_neither_built_nor_measured_again() {
        val rows = mixedRows(300)
        open(rows)
        fling(15, down = true)
        fling(15, down = false)
        val builds = com.ferb.design.components.RichCache.builds
        val measures = com.ferb.design.components.RichCache.measures
        fling(15, down = true)
        fling(15, down = false)
        assertEquals("texts built again", builds, com.ferb.design.components.RichCache.builds)
        assertEquals("folds measured again", measures, com.ferb.design.components.RichCache.measures)
    }
}
