package com.ferb.app

import android.graphics.Bitmap
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToIndex
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.Parity.answer
import com.ferb.app.Parity.entry
import com.ferb.app.Parity.message
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.RunTask
import com.ferb.app.model.RunningUi
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.app.ui.Sheets
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.LocalCardClock
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PlanPart
import com.ferb.design.components.WorkWords
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.PreviewFrame
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.ByteArrayOutputStream
import java.io.File

/**
 * The chat drawn as FERB.app draws it (kb PHN-720..PHN-729), goldens in app/src/test/screenshots/parity: the owner's
 * research answer folded and opened with its sites row and their list, every card kind, the work row with and without
 * its pictures, a task's short view, and a card of parts running and ended. Rows come from the real phone library.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class ParityShotsTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val vm by lazy {
        FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-parity-shots").toFile()) { null }, account = testAccount())
    }
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val base = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK), conversation = "c1")
    /** 2026-01-02 08:00 UTC: the invented cards' times read as the next hour, the countdowns fixed. */
    private val clock = 1_767_340_800_000L

    @Composable
    private fun Frame(ui: UiState, sheet: com.ferb.app.Sheet? = null, hour: Double = 22.5) {
        CompositionLocalProvider(LocalCardClock provides clock) {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
                PhoneScreen(labMoment(hour), insets = PhoneInsets.Lab, glide = false, sheet = {
                    if (sheet != null) Sheets(vm, ui, sheet, PhoneInsets.Lab, host) else LabAndroidChrome()
                }) {
                    Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(hour), host)
                }
            }
        }
    }

    private fun shoot(name: String) {
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag("phone").captureRoboImage("src/test/screenshots/parity/$name", roborazziOptions = ScreenShotsTest.options)
    }

    private fun scrollTo(rows: List<ChatRow>, key: String) {
        val i = rows.indexOfFirst { it.key == key }
        compose.onNodeWithTag("chat-list").performScrollToIndex(maxOf(0, i))
    }

    // decision: PHN-720, PHN-721, PHN-723
    @Test
    fun the_research_answer_folded_with_its_sites_row() {
        val rows = ChatModel.rows(Parity.research())
        compose.setContent { Frame(base.copy(rows = rows)) }
        scrollTo(rows, rows.first { it is ChatRow.Answer }.key)
        shoot("answer-folded.png")
    }

    // decision: PHN-721
    @Test
    fun the_research_answer_opened_by_show_all() {
        val rows = ChatModel.rows(Parity.research())
        compose.setContent { Frame(base.copy(rows = rows)) }
        compose.onNodeWithText("Show all").performClick()
        compose.mainClock.advanceTimeBy(500)
        scrollTo(rows, rows.first { it is ChatRow.Answer }.key)
        compose.onNodeWithTag("chat-list").performScrollToIndex(rows.lastIndex)
        shoot("answer-open.png")
    }

    // decision: PHN-723
    @Test
    fun the_sites_list_the_row_opens() {
        val rows = ChatModel.rows(Parity.research())
        val key = rows.first { it is ChatRow.Answer }.key
        compose.setContent { Frame(base.copy(rows = rows), com.ferb.app.Sheet.Sites(key)) }
        shoot("sites-list.png")
    }

    private fun cardsRows(vararg cards: Map<String, Any?>, text: String = "All set."): List<ChatRow> =
        ChatModel.rows(Parity.view(message("m1", "r1", "Set it up"), answer("a1", "r1", "m1", text, cards = cards.toList())))

    // decision: PHN-724
    @Test
    fun reminders_timers_and_schedules_as_the_mac_draws_them() {
        val at = "2026-01-02T09:00:00+00:00"
        val rows = cardsRows(
            mapOf("type" to "reminder", "title" to "Stretch", "timer_id" to "t9", "due" to at, "repeat" to "every day", "next_run" to "2026-01-03T09:00:00+00:00", "line" to "Reminder: Stretch"),
            mapOf("type" to "event", "kind" to "timer", "what" to "Tea", "id" to "x1", "when" to at, "line" to "Timer set"),
            mapOf("type" to "event", "kind" to "reminder", "what" to "Call mum", "id" to "x2", "when" to at),
            mapOf("type" to "event", "kind" to "schedule", "what" to "Morning news", "id" to "x3", "changed" to true, "was_repeat" to "every day at 8:00", "repeat" to "every Monday at 8:00"),
            text = "Reminder: Stretch\nTimer set\nAll set.",
        )
        compose.setContent { Frame(base.copy(rows = rows)) }
        scrollTo(rows, rows.first { it is ChatRow.Answer }.key)
        shoot("cards-timing.png")
    }

    // decision: PHN-724
    @Test
    fun facts_payments_sends_schedules_and_files_as_the_mac_draws_them() {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "Pay and send it"),
            answer("a1", "r1", "m1", "Done. Receipt kept\nAll set.", artifacts = listOf("/Users/me/Desktop/receipt.pdf", "/Users/me/Desktop/notes.txt"), cards = listOf(
                mapOf("type" to "note", "item_id" to "m1", "text" to "Prefers window seats", "state" to "saved"),
                mapOf("type" to "payment", "amount" to "$24.00", "merchant" to "Blue Bottle", "site" to "bluebottle.example", "card" to "Visa (personal)"),
                mapOf("type" to "send", "recipient" to "Ana Lopez", "kind" to "mailbox_send", "site" to "mail.example", "account" to "me@example.com"),
                mapOf("type" to "schedules", "schedules" to listOf(mapOf("id" to "s", "name" to "Morning news", "repeat" to "every day at 8:00", "next_run" to "2026-01-03T08:00:00+00:00"))),
                mapOf("type" to "file", "path" to "/Users/me/Desktop/receipt.pdf"),
                mapOf("type" to "receipt", "line" to "Receipt kept"),
            ))))
        compose.setContent { Frame(base.copy(rows = rows)) }
        scrollTo(rows, rows.first { it is ChatRow.Answer }.key)
        shoot("cards-records.png")
    }

    private fun running(parts: Int = 1) = RunningUi(WorkWords.of(parts, 0, 0), true, clock - 42_000,
        (1..parts).map { RunTask("m$it", "t-$it", if (it == 1) "Research automation detection" else "Compare desk prices", "Searching for extension detection", false, false) })

    // decision: PHN-725
    @Test
    fun the_work_row_of_a_one_part_task() {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "search online and then tell"),
            entry("p1", "plan", "r1", mapOf("text" to "I’ll research current technical and policy sources online, then give you a sourced answer.",
                "plan_line" to "I’ll research current technical and policy sources online, then give you a sourced answer.", "lines" to emptyList<String>(),
                "name" to "Research automation detection"), task = "t-1")))
        compose.setContent { Frame(base.copy(rows = rows, running = running())) }
        shoot("work-row.png")
    }

    // decision: PHN-725, PHN-726
    @Test
    fun the_work_row_with_its_live_pictures() {
        val rows = listOf(ChatRow.Owner("m1", "Compare the desks and research detection", emptyList(), null))
        compose.setContent { Frame(base.copy(rows = rows, running = running(3))) }
        compose.waitForIdle()
        vm.thumbnails.show(listOf("t-1", "t-2"))
        for (id in listOf("t-1", "t-2")) phone.thumbnails[id]?.frame(PreviewFrame(jpeg(), 1, "working", null, true, false, false, null))
        shoot("work-row-pictures.png")
    }

    // decision: PHN-725
    @Test
    fun a_tasks_short_view() {
        compose.setContent { Frame(base.copy(rows = emptyList(), running = running()), com.ferb.app.Sheet.Task("t-1")) }
        shoot("task-short-view.png")
    }

    // decision: PHN-726
    @Test
    fun a_card_of_parts_running_and_ended() {
        val rows = listOf(
            ChatRow.Owner("m1", "Compare the three standing desks", emptyList(), null),
            ChatRow.Task("r", "t-1", "On it. Three parts.", 1, 3, listOf(
                PlanPart(MarkKind.DONE, "Open your three desks", "3 pages read"),
                PlanPart(MarkKind.RUNNING, "Compare price and delivery", "Reading the Flexispot page"),
                PlanPart(MarkKind.WAITING, "Put the cheapest in your cart", "Waiting for Compare price and delivery"),
            ), status = "1 of 3 done · 1 running · 1 waiting"),
            ChatRow.Owner("m2", "And the chairs?", emptyList(), null),
            ChatRow.Task("e", "t-2", "Two parts.", 2, 2, listOf(
                PlanPart(MarkKind.DONE, "Find two chairs", "Done"),
                PlanPart(MarkKind.DONE, "Compare them", "The Aeron is lighter"),
            ), footer = "2 of 2 done", footerLines = listOf("Ran in Safari")),
        )
        compose.setContent { Frame(base.copy(rows = rows)) }
        shoot("task-parts.png")
    }

    private fun finishedCard(): List<ChatRow> {
        val second = (1..12).joinToString("\n") { "Line $it of the second part's answer, with a source: [Chrome docs](https://developer.chrome.com/docs) and **a bold term**." }
        val card = mapOf("request_id" to "r1", "task_id" to "t-1", "total" to 2.0, "done" to 2.0, "line" to "2 of 2 done",
            "lines" to listOf("Sites contacted: developer.chrome.com"),
            "rows" to listOf(
                mapOf("part" to "a", "label" to "Find the extension rules", "state" to "done", "step" to "s1",
                    "text" to "Extensions may automate pages only with the user's consent. See [the policy](https://developer.chrome.com/policy)."),
                mapOf("part" to "b", "label" to "Check what sites can see", "state" to "done", "step" to "s2", "text" to second, "note" to "the rate limits")))
        return ChatModel.rows(Parity.view(message("m1", "r1", "Find the rules and what sites can see"),
            entry("p1", "plan", "r1", mapOf("text" to "Two parts.", "plan_line" to "Two parts.", "lines" to emptyList<String>(), "card" to card), task = "t-1"),
            answer("a1", "r1", "m1", "1. Extensions…\n2. Sites…\n\nSites contacted: developer.chrome.com", lines = listOf("Sites contacted: developer.chrome.com"),
                cards = listOf(mapOf("type" to "link", "host" to "developer.chrome.com", "line" to "Sites contacted: developer.chrome.com")))))
    }

    // decision: PHN-733, PHN-734
    @Test
    fun a_finished_two_part_card_folded_on_three_skies() {
        val rows = finishedCard()
        var hour by androidx.compose.runtime.mutableDoubleStateOf(6.5)
        compose.setContent { Frame(base.copy(rows = rows), hour = hour) }
        for ((h, name) in listOf(6.5 to "dawn", 12.5 to "noon", 22.5 to "night")) {
            hour = h
            shoot("card-finished-folded-$name.png")
        }
    }

    // decision: PHN-733
    @Test
    fun a_finished_two_part_card_opened_by_show_more() {
        val rows = finishedCard()
        compose.setContent { Frame(base.copy(rows = rows)) }
        compose.onNodeWithText("Show more").performClick()
        compose.mainClock.advanceTimeBy(500)
        shoot("card-finished-open.png")
    }

    // decision: PHN-734
    @Test
    fun the_dark_surface_on_dawn_and_noon_skies() {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "Pay and send it"),
            answer("a1", "r1", "m1", "All set.", cards = listOf(
                mapOf("type" to "note", "item_id" to "m1", "text" to "Prefers window seats", "state" to "saved"),
                mapOf("type" to "payment", "amount" to "$24.00", "merchant" to "Blue Bottle", "site" to "bluebottle.example", "card" to "Visa (personal)"),
                mapOf("type" to "send", "recipient" to "Ana Lopez", "kind" to "mailbox_send", "site" to "mail.example", "account" to "me@example.com")))))
        var hour by androidx.compose.runtime.mutableDoubleStateOf(6.5)
        compose.setContent { Frame(base.copy(rows = rows), hour = hour) }
        for ((h, name) in listOf(6.5 to "dawn", 12.5 to "noon")) {
            hour = h
            shoot("cards-records-$name.png")
        }
    }

    private fun states(): List<ChatRow> {
        val dim = com.ferb.design.components.PartTone.DIM
        return listOf(
            ChatRow.Task("run", "t-1", "Microsoft outreach email", 0, 3, listOf(
                PlanPart(MarkKind.RUNNING, "Review source materials", "Trying again (attempt 3) · reading the outreach notes from the shared folder"),
                PlanPart(MarkKind.WAITING, "Draft the email", "Waiting for Review source materials",
                    sub = listOf(PlanPart(MarkKind.RUNNING, "Find the contact", "Searching the address book"))),
                PlanPart(MarkKind.WAITING, "Send it to the team", "Waiting for Draft the email"),
            ), status = "0 of 3 done · 1 running · 2 waiting", watch = listOf("t-1.s1", null, null),
                subWatch = listOf(emptyList(), listOf("t-1.s2a"), emptyList())),
            ChatRow.Task("ask", "t-2", "Book the dentist", 1, 2, listOf(
                PlanPart(MarkKind.DONE, "Find a free slot", rich = listOf(com.ferb.design.components.RichBlock.Paragraph(listOf(com.ferb.design.components.RichRun("Tuesday at 10 is free."))))),
                PlanPart(MarkKind.ASKING, "Book it", "Needs you", tone = com.ferb.design.components.PartTone.AMBER),
            ), tint = com.ferb.design.components.CardTint.ASKING),
            ChatRow.Task("end", "t-3", "Tidy the Desktop", 1, 3, listOf(
                PlanPart(MarkKind.FAILED, "Move the screenshots", "Couldn't do it: the folder is protected.", tone = com.ferb.design.components.PartTone.RED),
                PlanPart(MarkKind.FAILED, "Empty the old downloads", "Stopped by you.", tone = dim),
                PlanPart(MarkKind.DONE, "Sort the PDFs", rich = listOf(com.ferb.design.components.RichBlock.Paragraph(listOf(com.ferb.design.components.RichRun("12 PDFs moved to Documents/PDFs.")))),
                    notes = listOf(com.ferb.design.components.PartNote("Updated: you asked for Documents", tone = com.ferb.design.components.PartTone.BLUE)), updated = true),
            ), footer = "1 of 3 done", footerLines = listOf("1 stopped by you"), tint = com.ferb.design.components.CardTint.FAILED),
        )
    }

    // decision: PHN-737, PHN-734, PHN-735
    @Test
    fun the_card_of_parts_in_each_state_on_three_skies() {
        val rows = listOf<ChatRow>(ChatRow.Owner("m", "Write the Microsoft outreach email", emptyList(), null)) + states()
        var hour by androidx.compose.runtime.mutableDoubleStateOf(6.5)
        compose.setContent { Frame(base.copy(rows = rows), hour = hour) }
        for ((h, name) in listOf(6.5 to "dawn", 12.5 to "noon", 22.5 to "night")) {
            hour = h
            shoot("card-states-$name.png")
        }
    }

    // decision: PHN-737, PHN-738
    @Test
    fun eight_parts_and_a_single_part_checklist() {
        val eight = ChatRow.Task("eight", "t-8", "Plan the offsite", 3, 8, (1..8).map { n ->
            when {
                n <= 3 -> PlanPart(MarkKind.DONE, "Part $n", rich = listOf(com.ferb.design.components.RichBlock.Paragraph(listOf(com.ferb.design.components.RichRun("Part $n is done.")))))
                n == 4 -> PlanPart(MarkKind.RUNNING, "Part $n", "Reading the venue's page")
                else -> PlanPart(MarkKind.WAITING, "Part $n", "Waiting for Part 4")
            }
        }, status = "3 of 8 done · 1 running · 4 waiting")
        val checklist = ChatRow.Task("one", "t-9", "Check the train times", 0, 2, listOf(
            PlanPart(MarkKind.RUNNING, "Open the timetable", "Reading the 18:05 departures"),
            PlanPart(MarkKind.WAITING, "Tell you the next three trains")))
        compose.setContent { Frame(base.copy(rows = listOf(eight, checklist))) }
        shoot("card-eight-and-checklist.png")
    }

    /** A part that waits on steps no part answers: each step a compact line under the part's label, the working one
     *  bold with its own picture, its label kept whole before its words are cut (FERB.app's feeder lines, APP-895). */
    // decision: PHN-737
    // decision: PHN-851
    @Test
    fun a_waiting_part_shows_its_steps_as_compact_lines_under_it() {
        val card = ChatRow.Task("nested", "t-1", "Microsoft outreach email", 0, 3, listOf(
            PlanPart(MarkKind.RUNNING, "Review source materials", "Reading the outreach notes"),
            PlanPart(MarkKind.WAITING, "Draft the email", "Waiting for its steps", sub = listOf(
                PlanPart(MarkKind.DONE, "Open the shared folder"),
                PlanPart(MarkKind.RUNNING, "Look up everyone on the outreach list", "Searching the address book for each name"),
                PlanPart(MarkKind.WAITING, "Check the team's calendar"))),
            PlanPart(MarkKind.WAITING, "Send it to the team", "Waiting for Draft the email"),
        ), status = "0 of 3 done · 2 running · 2 waiting", watch = listOf("t-1.s1", null, null),
            subWatch = listOf(emptyList(), listOf(null, "t-1.s2a", null), emptyList()))
        compose.setContent { Frame(base.copy(rows = listOf(ChatRow.Owner("m", "Write the Microsoft outreach email", emptyList(), null), card))) }
        shoot("card-nested-steps.png")
    }

    /** Update fingerprint's card on the chat: what changed and where on the Mac, then the six digits while the Mac asks. */
    // decision: PHN-849
    @Test
    fun the_update_fingerprint_card_and_its_code() {
        val keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.GONE, strongOnMac = true)
        val rows = listOf<ChatRow>(ChatRow.Owner("m", "Pay the electricity bill", emptyList(), null))
        val waiting = FakePhone.status().copy(fingerprintUpdate = com.ferb.mobile.FingerprintUpdate.Waiting("123 456"))
        var ui by androidx.compose.runtime.mutableStateOf(base.copy(rows = rows, keys = keys, fingerprintUpdating = true, link = waiting))
        compose.setContent { Frame(ui) }
        compose.onNodeWithText("Waiting for your Mac").assertExists()
        shoot("fingerprint-code.png")
        // with the clock held, a state write reaches the composition once its snapshot is applied
        ui = base.copy(rows = rows, keys = keys)
        androidx.compose.runtime.snapshots.Snapshot.sendApplyNotifications()
        shoot("fingerprint-changed.png")
        ui = ui.copy(fingerprintNote = FerbViewModel.fingerprintFailed(
            com.ferb.mobile.LinkProblem("pair_denied", "Your Mac said no, so fingerprint checks stay off.", null)))
        androidx.compose.runtime.snapshots.Snapshot.sendApplyNotifications()
        shoot("fingerprint-failed.png")
    }

    private fun question(id: String, body: String): com.ferb.app.model.Question =
        com.ferb.app.model.Questions.parse(com.ferb.mobile.QuestionItem("e-$id", id, "r-1", "t-1", "open", true, body, "end_of_chat", null))

    /** One body per kind, as the gateway sends them (engine/fixtures/gateway/scenarios.json's shapes). */
    private val kinds = listOf(
        "sink" to """{"approval_id":"q-sink","capability":"ferb.confirm","kind":"owner_question","reason":"FERB: this types ps@microsoft.com, which FERB found only in outside content (the web page on mail.google.com; the web page on mailmeteor.com), not in your words, your answers or your own files. The step: browser.act action=type tab=t69 element=s13e633 text=ps@microsoft.com mode=insert. Reason given: \"Add Pratyay as the first main To recipient.\". Go ahead?","options":["Yes","No"],"summary":"Type this email address on mail.google.com?","why":"Add Pratyay as the first main To recipient.","detail":"FERB found it only in outside content (the web page on mail.google.com; the web page on mailmeteor.com), not in your words, your answers or your own files.","exact":"ps@microsoft.com","title":"Check before FERB goes on","labels":["Type it","Don't type it"],"corrects":true}""",
        "money" to """{"approval_id":"q-money","capability":"ferb.confirm","kind":"owner_question","reason":"FERB: this pays 5 to x@y.com. The step: probe.pay to=x@y.com. Go ahead?","options":["Yes","No"],"summary":"Pay 5 to x@y.com?","detail":"Money leaves your account. Yes covers only this payment.","exact":"5 → x@y.com","picture_problem":"FERB couldn't capture the payment page.","title":"Check before FERB goes on","labels":["Pay 5","Don't pay"],"corrects":true}""",
        "terminal" to """{"approval_id":"q-terminal","capability":"ferb.confirm","kind":"owner_question","reason":"FERB: this runs a command. The step: shell.run. Go ahead?","options":["Yes","No"],"summary":"Run this command in Terminal?","detail":"FERB types it into Terminal and presses Return.","exact":"brew upgrade --cask firefox","title":"Check before FERB goes on","labels":["Run it","Don't run"],"note":"Your Yes covers Terminal for the rest of this task."}""",
        "retry" to """{"approval_id":"q-retry","capability":"ferb.retry","kind":"owner_question","reason":"FERB: the last try may have sent it. Try again?","options":["Yes","No"],"summary":"Try sending again?","why":"I found no sent copy; try again?","detail":"FERB lost track of the last try, so it may already have worked; trying again could send it twice.","title":"Check before FERB retries","labels":["Try again","Don't retry"]}""",
        "task" to """{"approval_id":"q-task","capability":"owner.ask","kind":"owner_question","reason":"Which size should I order?","options":["Small","Medium","Large"],"from_task":true,"title":"Your task has a question"}""",
        "code" to """{"approval_id":"q-code","capability":"browser.login_code","kind":"owner_login_code","reason":"x","options":[],"summary":"otp.example.com asks for a login code","why":"the site sent a code","detail":"The page asks for “Enter the 6-digit code”. Type the code the site sent you.","title":"Private — only the page gets this","field_label":"Code"}""",
    )

    private fun ask(name: String) {
        val q = question(name, kinds.toMap().getValue(name))
        compose.setContent { Frame(base.copy(rows = listOf(ChatRow.Owner("m", "Write to Pratyay", emptyList(), null)), questions = listOf(q))) }
        shoot("question-$name.png")
    }

    // decision: PHN-739
    @Test fun the_sink_rule_question_reads_as_the_macs_card() = ask("sink")
    // decision: PHN-739
    @Test fun a_payment_question_reads_as_the_macs_card() = ask("money")
    // decision: PHN-739
    @Test fun a_terminal_question_reads_as_the_macs_card() = ask("terminal")
    // decision: PHN-739
    @Test fun a_retry_question_reads_as_the_macs_card() = ask("retry")
    // decision: PHN-739
    @Test fun a_tasks_question_reads_as_the_macs_card() = ask("task")
    // decision: PHN-739
    @Test fun a_login_code_reads_as_the_macs_card() = ask("code")

    /** A tap on FERB's own question's notification (a clarify) opens the chat with the cursor in the box. */
    // decision: PHN-739
    @Test
    fun a_clarify_tap_puts_the_cursor_in_the_message_box() {
        compose.setContent { Frame(base.copy(rows = listOf(ChatRow.Owner("m", "Write to Ana", emptyList(), null)))) }
        compose.waitForIdle()
        vm.openChat(focus = true)
        compose.waitForIdle()
        org.junit.Assert.assertFalse("taken", vm.focusAsk.value)
        compose.onNode(androidx.compose.ui.test.isFocused()).assertExists()
    }

    /** The first run's page: the notifications ask in the owner's words, which keeps FERB connected (kb PHN-771). */
    // decision: PHN-771
    @Test
    fun the_first_run_page_says_ferb_stays_connected_for_notifications() {
        val asks = com.ferb.app.ui.FirstRunUi(com.ferb.app.model.FirstRun.Ask.entries.toList(), {}, {})
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                    com.ferb.app.ui.Asks(asks, PhoneInsets.Lab)
                }
            }
        }
        shoot("first-run.png")
    }

    /** An answer delivered with parts not done: its uncertain-step note and code's lines small and grey, "Not done:" and
     *  "Couldn't confirm" amber, the status words small and amber under it (FERB.app's sizes and colours; kb PHN-773,
     *  APP-978), on three skies. */
    // decision: PHN-773, APP-978
    @Test
    fun an_answer_with_gaps_says_its_lines_and_status_smaller_in_their_colours() {
        val uncertain = "I'm not sure whether my last step on mail.example went through: I couldn't tell how it ended. I haven't tried it again, so nothing can happen twice."
        val text = "Left the unsent draft open for your review. It includes the requested content and leaves Sam out.\n\n" +
            "Not done: the main people are in To and the others in CC — you asked to stop before the duplicate was removed.\n\n" +
            "Couldn't confirm: the draft saved.\n" + uncertain
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "Draft the email to the team"),
            answer("a1", "r1", "m1", text, lines = listOf(uncertain), note = "the draft saved", status = "done_with_gaps")))
        var hour by androidx.compose.runtime.mutableDoubleStateOf(6.5)
        compose.setContent { Frame(base.copy(rows = rows), hour = hour) }
        for ((h, name) in listOf(6.5 to "dawn", 12.5 to "noon", 22.5 to "night")) {
            hour = h
            shoot("answer-gaps-$name.png")
        }
        val line = rows.filterIsInstance<ChatRow.Line>().single { it.key.endsWith(":status") }
        org.junit.Assert.assertEquals("Delivered, with parts not done" to true, line.text to line.amber)
    }

    private fun jpeg(): ByteArray {
        val b = Bitmap.createBitmap(160, 100, Bitmap.Config.ARGB_8888)
        val c = android.graphics.Canvas(b)
        c.drawColor(0xFF2B6CB0.toInt())
        c.drawRect(20f, 20f, 140f, 60f, android.graphics.Paint().apply { color = 0xFFF5F5F5.toInt() })
        return ByteArrayOutputStream().also { b.compress(Bitmap.CompressFormat.JPEG, 90, it) }.toByteArray()
    }
}
