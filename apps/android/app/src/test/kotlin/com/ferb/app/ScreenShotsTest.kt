package com.ferb.app

import android.graphics.Bitmap
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.CardLine
import com.ferb.app.model.LiveView
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.MacSettings
import com.ferb.app.model.Phase
import com.ferb.app.model.Transfer
import com.ferb.app.model.Way
import com.ferb.app.model.Batch
import com.ferb.mobile.FileEntry
import com.ferb.mobile.Folder
import com.ferb.mobile.MacInfo
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.SettingValue
import com.ferb.app.model.ChatRow
import com.ferb.app.model.Question
import com.ferb.app.model.Work
import com.ferb.app.model.WorkItem
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.app.ui.Message
import com.ferb.app.ui.Sheets
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
import com.ferb.design.sky.SkyMoment
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.OlderState
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.RelayStatus
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.captureRoboImage
import java.io.File
import android.graphics.BitmapFactory
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The app's own screens, drawn by Robolectric's native renderer at the lab's Android size: the chat with the lab's
 * own sample against :design's golden of the lab's chat (the app must look like the lab, PHN-416), and the states only
 * the app has against the app's goldens (app/src/test/screenshots; `:app:recordRoborazziDebug` rewrites them).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class ScreenShotsTest {
    @get:Rule val compose = createComposeRule()

    private val vm by lazy {
        FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-shots").toFile()) { null })
    }
    private val phone = FakePhone()
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }

    private val labRows = listOf(
        ChatRow.Owner("a", "What's left on my calendar today?", emptyList(), null),
        ChatRow.Ferb("b", "Two things: the design review at 4, and a call with the bank at 6:30."),
        ChatRow.Owner("c", "Compare the three standing desks I bookmarked and put the cheapest in my cart", emptyList(), null),
        ChatRow.Task("d", "t-1", "On it. Three parts.", 2, 3, listOf(
            PlanPart(MarkKind.DONE, "Open your three desks", "3 pages read"),
            PlanPart(MarkKind.RUNNING, "Compare price and delivery", "Reading delivery options"),
            PlanPart(MarkKind.WAITING, "Put the cheapest in your cart"),
        )),
    )
    private val asking = Question("ap-1", "t-1", "Run this in Terminal?", "To update Figma before opening the file you asked for.",
        "brew upgrade --cask figma", "owner_question", "ferb.confirm", listOf("Yes", "No"), listOf("Yes, run it", "No"), false)
    private val online = FakePhone.status()
    private val base = UiState(gate = Gate.MAIN, link = online, keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK), questions = listOf(asking))

    @Composable
    private fun Frame(moment: SkyMoment, ui: UiState, tab: PhoneTab = PhoneTab.CHAT, sheet: Sheet? = null) {
        Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
            PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false, sheet = {
                Sheets(vm, ui, sheet, PhoneInsets.Lab, host)
                LabAndroidChrome()
            }) { Main(vm, ui, tab, home = false, PhoneInsets.Lab, moment, host) }
        }
    }

    private fun shoot(path: String) {
        compose.mainClock.autoAdvance = false
        compose.mainClock.advanceTimeByFrame()
        compose.onNodeWithTag(PHONE).captureRoboImage("src/test/screenshots/$path", roborazziOptions = options)
    }

    // decision: PHN-416, PHN-455, PHN-509
    @Test
    fun the_chat_drawn_from_the_labs_sample_is_the_labs_chat() {
        val moment = labMoment(12.5)
        compose.setContent { Frame(moment, base.copy(rows = labRows)) }
        compose.mainClock.autoAdvance = false
        compose.mainClock.advanceTimeByFrame()
        val out = File("build/lab-compare/chat-noon.png")
        compose.onNodeWithTag(PHONE).captureRoboImage(out.path, roborazziOptions = RoborazziOptions(taskType = com.github.takahirom.roborazzi.RoborazziTaskType.Record))
        val ours = BitmapFactory.decodeFile(out.path)
        val golden = BitmapFactory.decodeFile(File(System.getProperty("ferb.design.goldens"), "screens/chat-noon.png").path)
        val changed = changedFraction(ours, golden)
        assertTrue("the app's chat differs from the lab's in ${"%.3f".format(changed * 100)} % of its pixels", changed < 0.001)
    }

    // decision: PHN-401, ORCH-651
    @Test
    fun chat_with_the_outbox_a_notice_and_older_messages_waiting() {
        val rows = listOf(
            ChatRow.Older("older", OlderState.NeedsBetterPath("Older messages need a direct connection or your own relay.")),
            ChatRow.Owner("a", "Tidy my Downloads folder", emptyList(), null),
            ChatRow.Ferb("b", "Done: 214 files moved to Archive.\nNot done: the two installers — they were still open."),
            ChatRow.Cards("c", listOf(CardLine("Archive", "Downloads", file = true))),
            ChatRow.Line("d", "The backup drive is almost full."),
            ChatRow.Asking("q:ap-1", "ap-1", "Run this in Terminal?"),
            ChatRow.Outbox("o:1", OutboxItem("1", "And empty the trash", "2026-01-02T10:00:00+00:00", "waiting", "link", null, false, emptyList())),
            ChatRow.Outbox("o:2", OutboxItem("2", "Rename the scans", "2026-01-02T10:00:00+00:00", "failed", null, "FERB on your Mac turned this down", false, emptyList())),
        )
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = rows, link = FakePhone.status(LinkPhase.WAITING))) }
        shoot("chat-offline-outbox.png")
    }

    // decision: PHN-417, PHN-470
    @Test
    fun a_question_sheet_with_the_macs_words() {
        compose.setContent { Frame(labMoment(22.5), base.copy(rows = labRows), sheet = Sheet.Asking("ap-1")) }
        shoot("question.png")
    }

    /** A screen-use question with an option the Mac adds (kb PHN-607): Yes and No as the lab draws them, the extra under. */
    // decision: PHN-607
    @Test
    fun a_question_with_an_extra_option_from_the_mac() {
        val q = Question("ap-3", "t-1", "Use your screen for this?", "FERB needs to click in Keynote, which works only in front.", null,
            "owner_question", "screen.foreground", listOf("Yes", "No", "Until I'm back"), listOf("Use the screen", "Not now", "Until I'm back at my Mac"), false)
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows, questions = listOf(q)), sheet = Sheet.Asking("ap-3")) }
        shoot("question-extra.png")
    }

    /** What FERB says before the first use of speaking to it, on a phone that recognises speech itself or by its service. */
    // decision: PHN-604
    @Test
    fun speaking_is_explained_before_androids_question() {
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows), sheet = Sheet.Voice(com.ferb.app.model.SpeechKind.ON_DEVICE)) }
        shoot("voice-on-device.png")
    }

    // decision: PHN-604
    @Test
    fun speaking_through_the_phones_service_says_where_the_voice_may_go() {
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows), sheet = Sheet.Voice(com.ferb.app.model.SpeechKind.SERVICE)) }
        shoot("voice-service.png")
    }

    // decision: PHN-110, PHN-502
    @Test
    fun a_private_code_takes_the_labs_keypad() {
        val code = Question("ap-2", null, null, "The 6-digit code Northfield Energy sent you.", null, "owner_login_code", null, emptyList(), emptyList(), true)
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows, questions = listOf(code)), sheet = Sheet.Asking("ap-2")) }
        shoot("private-code.png")
    }

    // decision: PHN-110, PHN-502
    @Test
    fun a_private_password_takes_the_system_keyboard_in_its_password_mode() {
        val pw = Question("ap-3", null, "Your Mac's password", "To install the printer driver you asked for.", null, "owner_admin_password", null, emptyList(), emptyList(), true)
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows, questions = listOf(pw)), sheet = Sheet.Asking("ap-3")) }
        shoot("private-password.png")
    }

    // decision: PHN-420, PHN-421, PHN-422, PHN-8
    @Test
    fun this_phone_with_its_switches() {
        compose.setContent { Frame(labMoment(6.1), base.copy(rows = labRows), sheet = Sheet.Profile) }
        shoot("profile.png")
    }

    // decision: PHN-13
    @Test
    fun tasks_waiting_running_and_earlier() {
        val work = Work(
            listOf(WorkItem(MarkKind.ASKING, "Renew the domain", "Asks before paying $12.99", approvalId = "ap-1")),
            listOf(WorkItem(MarkKind.RUNNING, "Compare standing desks", "Comparing price and delivery", 2 to 3)),
            listOf(WorkItem(MarkKind.DONE, "Back up the thesis folder", "Copied to the external drive"), WorkItem(MarkKind.FAILED, "Download the October invoice", "The site asked for a code that never came")),
        )
        compose.setContent { Frame(labMoment(12.5), base.copy(work = work), tab = PhoneTab.TASKS) }
        shoot("tasks.png")
    }

    // decision: PHN-418, PHN-425, PHN-444
    @Test
    fun the_mac_through_the_shared_relay_and_its_sheet() {
        val relay = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        compose.setContent { Frame(labMoment(22.5), base.copy(link = relay), tab = PhoneTab.MAC, sheet = Sheet.Relay) }
        shoot("mac-relay.png")
    }

    // decision: PHN-425
    @Test
    fun the_mac_with_its_relay_down_and_the_public_one_standing_in() {
        val fallback = FakePhone.status(path = LinkPath.PUBLIC_RELAY).copy(ownRelayUrl = "https://relay.example.net:7443", relay = RelayStatus.OwnFallback(null))
        compose.setContent { Frame(labMoment(12.5), base.copy(link = fallback), tab = PhoneTab.MAC) }
        shoot("mac-own-fallback.png")
    }

    // decision: PHN-113, PHN-115, PHN-127
    @Test
    fun files_places_with_counts_and_photos_on_their_way() {
        vm.transfers.setForTest(listOf(
            Transfer("b1.0", Way.SEND, "IMG_2041.jpg", "/x/a", done = 2_400_000, total = 2_400_000, phase = Phase.DONE, batch = "b1"),
            Transfer("b1.1", Way.SEND, "IMG_2042.jpg", "/x/b", done = 800_000, total = 2_600_000, phase = Phase.RUNNING, batch = "b1"),
        ))
        val recent = com.ferb.mobile.RecentFiles(listOf(
            com.ferb.mobile.FoundFile("Desk comparison.pdf", "~/Downloads/Desk comparison.pdf", false, 320_000u, 1_767_348_000, "made", "2026-01-02T10:00:00+00:00", "t-1", "Compare standing desks"),
            com.ferb.mobile.FoundFile("Lease agreement.pdf", "~/Documents/Lease agreement.pdf", false, 1_100_000u, 1_767_348_000, "fetched", "2026-01-02T09:00:00+00:00", null, null),
        ), 1u)
        val ui = base.copy(files = FilesUi(counts = mapOf("~/Desktop" to 38L, "~/Documents" to 112L, "~/Downloads" to 9L), recent = recent))
        compose.setContent { Frame(labMoment(12.5), ui, tab = PhoneTab.FILES) }
        shoot("files-places.png")
    }

    // decision: PHN-113, PHN-121
    // decision: PHN-538
    @Test
    fun a_search_of_names_says_what_it_left_out() {
        val found = com.ferb.mobile.FoundFiles("~/Documents", "lease", listOf(
            com.ferb.mobile.FoundFile("Leases", "~/Documents/Leases", true, 0u, 0, "", null, null, null),
            com.ferb.mobile.FoundFile("Lease agreement.pdf", "~/Documents/Leases/Lease agreement.pdf", false, 1_100_000u, 1_767_348_000, "", null, null, null),
        ), 5000u, 2u, true, true, 0u, null)
        val folder = Folder("~/Documents", "~", false, emptyList(), 0u, false, 0u, null)
        val ui = base.copy(files = FilesUi(folder = folder, search = SearchUi("lease", "~/Documents", loading = false, found = found)))
        compose.setContent { Frame(labMoment(12.5), ui, tab = PhoneTab.FILES) }
        shoot("files-search.png")
    }

    // decision: PHN-13, PHN-115
    // decision: PHN-533
    @Test
    fun a_whole_folder_on_its_way_says_its_files_and_what_was_left_out() {
        vm.transfers.setForTest(listOf(
            Transfer("d1.0", Way.FETCH, "return.pdf", "/x/d", "~/Documents/Taxes/2025/return.pdf", 2_000_000, 2_000_000, Phase.DONE, batch = "d1", rel = "2025/return.pdf"),
            Transfer("d1.1", Way.FETCH, "receipts.zip", "/x/e", "~/Documents/Taxes/receipts.zip", 1_500_000, 6_000_000, Phase.RUNNING, batch = "d1", rel = "receipts.zip"),
            Transfer("d1.2", Way.FETCH, "notes.txt", "/x/f", "~/Documents/Taxes/notes.txt", 0, 4_000, Phase.PAUSED, batch = "d1", rel = "notes.txt"),
        ), listOf(com.ferb.app.model.Batch("d1", Way.FETCH, "Taxes", "~/Documents/Taxes", skipped = 2, cut = true)))
        val folder = Folder("~/Documents/Taxes", "~/Documents", false, listOf(
            FileEntry("2025", "~/Documents/Taxes/2025", true, false, 0u, 0, false),
            FileEntry("receipts.zip", "~/Documents/Taxes/receipts.zip", false, false, 6_000_000u, 1_767_348_000, false),
        ), 2u, false, 0u, null)
        compose.setContent { Frame(labMoment(12.5), base.copy(files = FilesUi(folder = folder)), tab = PhoneTab.FILES) }
        shoot("files-folder-whole.png")
    }

    // decision: GATE-613, SENSE-420
    // decision: PHN-537
    @Test
    fun the_macs_request_for_the_exact_place_asks_on_the_phone() {
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows), sheet = Sheet.Location("loc-1")) }
        shoot("exact-location.png")
    }

    // decision: SENSE-420, PHN-420
    // decision: PHN-532
    @Test
    fun sharing_the_city_is_explained_before_androids_question() {
        compose.setContent { Frame(labMoment(6.1), base.copy(rows = labRows), sheet = Sheet.City) }
        shoot("share-city.png")
    }

    /** A long folder (PHN-609): "Load more" under the first page while the Mac says there is more. */
    // decision: PHN-609
    @Test
    fun a_long_folder_offers_load_more() {
        val folder = Folder("~/Downloads", "~", false, (1..4).map { FileEntry("Scan $it.pdf", "~/Downloads/Scan $it.pdf", false, false, 300_000u, 1_767_348_000, false) }, 1_240u, true, 0u, "p2")
        compose.setContent { Frame(labMoment(12.5), base.copy(files = FilesUi(folder = folder)), tab = PhoneTab.FILES) }
        shoot("files-load-more.png")
    }

    /** A file sent from the phone says where it is on the Mac: the Mac's words when the library carries them, else the default. */
    // decision: PHN-608
    @Test
    fun a_file_sent_says_where_it_is_on_the_mac() {
        vm.transfers.setForTest(listOf(
            Transfer("b1.0", Way.SEND, "Boarding pass.pdf", "/x/o", null, 240_000, 240_000, Phase.DONE, uploadId = "u-1", batch = "b1"),
        ), listOf(Batch("b1", Way.SEND)))
        compose.setContent { Frame(labMoment(12.5), base, tab = PhoneTab.FILES) }
        shoot("files-sent.png")
    }

    // decision: PHN-113, PHN-121, PHN-11
    @Test
    fun a_folder_with_a_key_place_and_a_fetched_file() {
        vm.transfers.setForTest(listOf(
            Transfer("f1", Way.FETCH, "Lease agreement.pdf", "/x/c", "~/Documents/Lease agreement.pdf", 1_100_000, 1_100_000, Phase.DONE),
        ))
        val folder = Folder("~/Documents", "~", false, listOf(
            FileEntry("Taxes", "~/Documents/Taxes", true, false, 0u, 0, false),
            FileEntry("Keys", "~/Documents/Keys", true, false, 0u, 0, true),
            FileEntry("Lease agreement.pdf", "~/Documents/Lease agreement.pdf", false, false, 1_100_000u, 1_767_348_000, false),
            FileEntry("Thesis draft v7.docx", "~/Documents/Thesis draft v7.docx", false, false, 2_400_000u, 1_767_348_000, false),
        ), 4u, false, 0u, null)
        compose.setContent { Frame(labMoment(12.5), base.copy(files = FilesUi(folder = folder)), tab = PhoneTab.FILES) }
        shoot("files-folder.png")
    }

    // decision: PHN-454
    @Test
    fun watch_live_shows_the_frame_and_the_line() {
        val bitmap = android.graphics.Bitmap.createBitmap(320, 200, android.graphics.Bitmap.Config.ARGB_8888).apply {
            val c = android.graphics.Canvas(this)
            c.drawColor(android.graphics.Color.rgb(236, 239, 243))
            val p = android.graphics.Paint().apply { color = android.graphics.Color.rgb(40, 90, 200) }
            c.drawRect(20f, 20f, 300f, 60f, p)
            p.color = android.graphics.Color.rgb(180, 180, 190)
            for (i in 0 until 4) c.drawRect(20f, 80f + i * 28, 240f - i * 30, 96f + i * 28, p)
        }
        val view = LiveView("t-1", bitmap, "working", "Comparing price and delivery")
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false, sheet = {
                    com.ferb.app.ui.LivePage(view, PhoneInsets.Lab, {}, {}, null)
                    LabAndroidChrome()
                }) { Main(vm, base.copy(rows = labRows), PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host) }
            }
        }
        shoot("watch-live.png")
    }

    // decision: PHN-404, PHN-133
    @Test
    fun ferbs_settings_from_the_phone() {
        val values = listOf(
            SettingValue("sense.window_titles", "true"), SettingValue("privacy.exclude_apps", """["Password manager","Banking"]"""),
            SettingValue("privacy.exclude_domains", "[]"), SettingValue("sense.trail_retention_days", "7"), SettingValue("retention.trace_days", "30"),
            SettingValue("retention.decision_days", "30"), SettingValue("retention.evidence_days", "7"), SettingValue("retention.recovery_days", "7"),
            SettingValue("retention.resource_days", "30"), SettingValue("retention.operational_days", "90"),
        )
        compose.setContent { Frame(labMoment(12.5), base.copy(settings = MacSettings.rows(values)), tab = PhoneTab.MAC, sheet = Sheet.Settings) }
        shoot("settings.png")
    }

    // decision: PHN-125, PHN-302
    @Test
    fun the_mac_by_its_name_says_it_stopped() {
        val stopped = PhoneStatus(false, true, true, emptyList(), emptyList(), emptyList(), null)
        val ui = base.copy(status = stopped, mac = MacInfo("Studio Mac", "2026-11-10T09:00:00+05:30", "Pixel", null))
        compose.setContent { Frame(labMoment(22.5), ui, tab = PhoneTab.MAC) }
        shoot("mac-stopped.png")
    }

    /** FERB may use the Mac's screen until the owner is back (PHN-611): the line and "Ask me again" on the Mac tab. */
    // decision: PHN-611
    @Test
    fun the_mac_says_ferb_may_use_its_screen_until_he_is_back() {
        val use = com.ferb.mobile.ScreenUse(true, "2026-11-17T10:00:00+05:30", "phone-1", "Pixel", true)
        val ui = base.copy(status = PhoneStatus(false, true, false, emptyList(), emptyList(), emptyList(), use),
            mac = MacInfo("Studio Mac", "2026-11-10T09:00:00+05:30", "Pixel", null))
        compose.setContent { Frame(labMoment(12.5), ui, tab = PhoneTab.MAC) }
        shoot("mac-screen-use.png")
    }

    /** The screen-use question when the Mac offers "until I'm back at my Mac" (PHN-610, PHN-611): the third choice and its line. */
    // decision: PHN-611
    @Test
    fun the_screen_use_question_offers_until_i_am_back() {
        val q = Question("ap-4", "t-1", "Use your screen for this?", "FERB needs to click in Keynote, which works only in front.", null,
            "owner_question", "screen.foreground", listOf("Yes", "No"), listOf("Use the screen", "Not now"), false,
            com.ferb.mobile.QuestionNeeds(true, false, false, false, "ch-1", null, true))
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows, questions = listOf(q)), sheet = Sheet.Asking("ap-4")) }
        shoot("question-until-back.png")
    }

    // decision: PHN-125
    @Test
    fun stop_everything_asks_first() {
        compose.setContent { Frame(labMoment(22.5), base, tab = PhoneTab.MAC, sheet = Sheet.StopAll) }
        shoot("stop-all.png")
    }

    // decision: PHN-408
    @Test
    fun forgetting_the_mac_asks_first() {
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows), sheet = Sheet.Forget) }
        shoot("forget.png")
    }

    // decision: PHN-8
    @Test
    fun a_phone_without_a_screen_lock_is_told_how() {
        val moment = labMoment(12.5)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
                PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                    Message(androidx.compose.ui.res.stringResource(R.string.no_lock_title), androidx.compose.ui.res.stringResource(R.string.no_lock_text),
                        androidx.compose.ui.res.stringResource(R.string.no_lock_open_settings), PhoneInsets.Lab) {}
                }
            }
        }
        shoot("no-screen-lock.png")
    }

    // decision: PHN-442, PHN-302
    @Test
    fun a_wrong_code_is_said_in_plain_words() {
        val moment = labMoment(6.1)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
                PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                    com.ferb.app.ui.Pair(vm, false, null, androidx.compose.ui.res.stringResource(R.string.pair_wrong_code), PhoneInsets.Lab)
                }
            }
        }
        shoot("pair-wrong-code.png")
    }

    // decision: PHN-447
    @Test
    fun the_scanner_asks_for_the_camera_in_plain_words() {
        val moment = labMoment(22.5)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
                PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                    com.ferb.app.ui.Scanner(vm, PhoneInsets.Lab) {}
                }
            }
        }
        shoot("scanner-camera.png")
    }

    /**
     * The scanner with the camera on, at dawn, noon and night (kb PHN-618). The camera's picture is a stand-in larger
     * than the frame and pushed up and to the right, as the phone drew it before: nothing of it may show outside the
     * frame's rounded well, the corner marks sit over it, and the hint and the card below stay clear.
     */
    // decision: PHN-617, PHN-618, PHN-619, PHN-447
    @Test
    fun the_scanner_keeps_the_camera_inside_its_frame_at_dawn() = scannerLive("dawn", 6.1)

    // decision: PHN-617, PHN-618, PHN-619, PHN-447
    @Test
    fun the_scanner_keeps_the_camera_inside_its_frame_at_noon() = scannerLive("noon", 12.5)

    // decision: PHN-617, PHN-618, PHN-619, PHN-447
    @Test
    fun the_scanner_keeps_the_camera_inside_its_frame_at_night() = scannerLive("night", 22.5)

    private fun scannerLive(name: String, hour: Double) {
        val maroon = Color(0xFF7A2E4A)
        val moment = labMoment(hour)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
                PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                    com.ferb.app.ui.ScannerPage(PhoneInsets.Lab, com.ferb.app.ui.CameraAccess.ALLOWED, {}, {}, {}) {
                        // a camera picture of its own size and place: a dark room and the Mac's white screen
                        Box(Modifier.wrapContentSize(unbounded = true).offset(60.dp, (-60).dp).requiredSize(420.dp, 560.dp).background(maroon)) {
                            Box(Modifier.align(Alignment.Center).size(150.dp, 110.dp).background(Color.White))
                        }
                    }
                }
            }
        }
        compose.mainClock.autoAdvance = false
        compose.mainClock.advanceTimeByFrame()
        val well = compose.onNodeWithContentDescription("Camera view for the pairing code").fetchSemanticsNode().boundsInRoot
        val check = File("build/scanner-check/$name.png")
        compose.onNodeWithTag(PHONE).captureRoboImage(check.path, roborazziOptions = RoborazziOptions(taskType = com.github.takahirom.roborazzi.RoborazziTaskType.Record))
        val image = BitmapFactory.decodeFile(check.path)
        var inside = 0
        var outside = 0
        for (y in 0 until image.height) for (x in 0 until image.width) {
            if (image.getPixel(x, y) != maroon.toArgb()) continue
            if (x >= well.left && x < well.right && y >= well.top && y < well.bottom) inside++ else outside++
        }
        assertTrue("$name: the picture shows in the well ($inside)", inside > 10_000)
        assertEquals("$name: nothing of the picture outside the well", 0, outside)
        compose.onNodeWithTag(PHONE).captureRoboImage("src/test/screenshots/scanner-live-$name.png", roborazziOptions = options)
    }

    /** The microphone's states as the composer draws them (kb PHN-643): pressed or getting ready, listening loud, finishing. */
    // decision: PHN-642, PHN-643
    @Test
    fun the_microphone_shows_what_it_is_doing() {
        val moment = labMoment(22.5)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, 440.dp).testTag(PHONE)) {
                PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false) {
                    androidx.compose.foundation.layout.Column(
                        Modifier.padding(16.dp),
                        verticalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(28.dp),
                    ) {
                        for ((state, level) in listOf(
                            com.ferb.design.components.MicState.READYING to 0f,
                            com.ferb.design.components.MicState.LISTENING to 0.2f,
                            com.ferb.design.components.MicState.LISTENING to 0.9f,
                            com.ferb.design.components.MicState.FINISHING to 0f,
                        )) {
                            com.ferb.design.components.PhoneComposer(
                                Modifier, placeholder = "Ask FERB", typed = "", onValueChange = {}, onMicDown = {}, onMicUp = {},
                                listening = true, listeningText = state.name, mic = state, micLevel = level,
                            )
                        }
                    }
                }
            }
        }
        shoot("composer-mic.png")
    }

    // accessibility: the chat and its outbox at 150 % text
    @Config(qualifiers = "w412dp-h1000dp-port-mdpi", fontScale = 1.5f)
    @Test
    fun chat_at_150_percent_text() {
        compose.setContent { Frame(labMoment(12.5), base.copy(rows = labRows.take(2) + ChatRow.Outbox("o:1", OutboxItem("1", "And empty the trash", "t", "waiting", "unlock", null, false, emptyList())))) }
        shoot("large-text/chat.png")
    }

    companion object {
        const val PHONE = "phone"
        val options = RoborazziOptions(compareOptions = RoborazziOptions.CompareOptions(changeThreshold = 0.001f))

        /** The share of pixels whose colour differs by more than a rounding step in any channel. */
        fun changedFraction(a: Bitmap, b: Bitmap): Double {
            if (a.width != b.width || a.height != b.height) return 1.0
            var changed = 0
            for (y in 0 until a.height) for (x in 0 until a.width) {
                val p = a.getPixel(x, y)
                val q = b.getPixel(x, y)
                val d = maxOf(Math.abs((p shr 16 and 255) - (q shr 16 and 255)), Math.abs((p shr 8 and 255) - (q shr 8 and 255)), Math.abs((p and 255) - (q and 255)))
                if (d > 2) changed++
            }
            return changed.toDouble() / (a.width * a.height)
        }
    }
}
