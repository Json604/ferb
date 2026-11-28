#if canImport(UIKit)
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import XCTest
@testable import FerbAppUI

/// The chat drawn as FERB.app draws it (kb PHN-720..PHN-729; Android: ShownTest, ParityShotsTest), through the real
/// phone library on the simulator: the owner's research answer, each card kind (never a generic row), an unknown kind,
/// the files once, a question's words, a reminder's box, the work row of a one-part task and its pictures' rules, and
/// goldens of each (Screenshots/app/parity).
@MainActor
final class ParityTests: XCTestCase {
    private var pos = 10

    private func entry(_ id: String, _ kind: String, _ request: String, _ body: [String: Any], task: String? = nil, state: String = "done",
                       answers: String? = nil) -> [String: Any] {
        pos += 1
        return ["id": id, "conversation_id": "C1", "pos": pos, "seq": pos, "rev": 1, "kind": kind, "state": state, "open": kind == "question",
                "request_id": request, "task_id": task ?? NSNull(), "approval_id": body["approval_id"] ?? NSNull(), "ref": NSNull(),
                "answers": answers ?? NSNull(), "placement": kind == "answer" ? "inline" : NSNull(), "origin": NSNull(), "client_id": "phone-1",
                "submission_id": NSNull(), "journal_id": NSNull(), "created_at": "2026-01-02T09:00:00+00:00", "updated_at": "2026-01-02T09:00:00+00:00",
                "day": "2026-01-02", "body": body, "deleted": false]
    }

    private func message(_ id: String, _ request: String, _ text: String) -> [String: Any] {
        entry(id, "message", request, ["text": text, "attachments": [String](), "hidden": [String]()], task: "t-1")
    }

    private func answer(_ id: String, _ request: String, _ of: String, _ text: String, lines: [String] = [], cards: [[String: Any]] = [],
                        artifacts: [String] = []) -> [String: Any] {
        entry(id, "answer", request, ["from": "task", "text": text, "status": "done", "verified": true, "artifacts": artifacts, "note": NSNull(),
                                      "lines": lines, "cards": cards], task: "t-1", answers: of)
    }

    private func view(_ entries: [[String: Any]]) throws -> TranscriptView {
        try FixtureMac.transcript(FixtureMac(FixtureConversation(file: "invented", scenario: "parity", conversation: "C1", entries: entries)))
    }

    private let sitesLine = "Sites contacted: 6 sites"
    private let hosts = ["developer.chrome.com", "developer.mozilla.org", "developers.cloudflare.com", "w3c.github.io", "www.google.com", "www.linkedin.com"]

    private func research() throws -> TranscriptView {
        let text = "**Yes, a website can sometimes infer automated interaction, but it usually cannot directly see “an accessibility API or extension clicked this.”** Detectability depends mainly on how the action is generated.\n\n"
            + "- **Accessibility/native-browser actions:** the site generally receives ordinary focus, keyboard and pointer events.\n"
            + "- **DOM or synthetic events:** a page can sometimes tell them apart (`isTrusted` is false).\n"
            + "- **Account rules:** LinkedIn prohibits unauthorized automated access. [LinkedIn User Agreement](https://www.linkedin.com/legal/user-agreement)\n\n"
            + (1...14).map { "Paragraph \($0) of the answer goes on about fingerprints, rate limits and the session as a whole, so that the answer is long." }.joined(separator: "\n\n")
            + "\n\n**Bottom line:** a site usually cannot conclusively identify extension-driven accessibility interaction from one normal action.\n\n" + sitesLine
        return try view([
            message("m1", "r1", "search online and then tell"),
            entry("p1", "plan", "r1", ["text": "I’ll research current technical and policy sources online, then give you a sourced answer.",
                                       "plan_line": "I’ll research current technical and policy sources online, then give you a sourced answer.", "lines": [String](),
                                       "name": "Research automation detection"], task: "t-1"),
            answer("a1", "r1", "m1", text, lines: [sitesLine], cards: hosts.map { ["type": "link", "host": $0, "line": sitesLine] }),
        ])
    }

    private func answers(_ rows: [ChatRow]) -> [(String, AnswerUi)] {
        rows.compactMap { if case .answer(let k, let ui) = $0 { return (k, ui) } else { return nil } }
    }

    // decision: PHN-720, PHN-721, PHN-723
    func testTheResearchAnswerIsRichTextWithOneSitesRowAsOnTheMac() throws {
        let rows = ChatModel.rows(try research())
        let a = try XCTUnwrap(answers(rows).first).1
        XCTAssertEqual(a.sites, hosts)
        let words = ChatModel.words(.answer(key: "k", ui: a))
        XCTAssertFalse(words.contains("Sites contacted"))
        XCTAssertFalse(words.contains("(https://"))
        XCTAssertFalse(rows.contains { if case .cards = $0 { return true }; if case .results = $0 { return true }; return false })
        guard case .paragraph(let lead) = a.blocks[0] else { return XCTFail("a paragraph first") }
        XCTAssertTrue(lead[0].bold)
        XCTAssertTrue(a.blocks.contains { if case .item(_, _, let r) = $0 { return r.contains { $0.link == "https://www.linkedin.com/legal/user-agreement" } } else { return false } })
        XCTAssertTrue(a.lines.isEmpty && a.files.isEmpty)
    }

    private func cards(_ cards: [[String: Any]], text: String = "All set.") throws -> [ResultShown] {
        let rows = ChatModel.rows(try view([message("m1", "r1", "do it"), answer("a1", "r1", "m1", text, cards: cards)]))
        let a = try XCTUnwrap(answers(rows).first).1
        return a.above + a.below
    }

    // decision: PHN-724
    func testEveryCardKindIsDrawnAsTheMacDrawsItNeverAsAGenericRow() throws {
        let at = "2026-01-02T09:00:00+00:00"
        guard case .reminder(_, let title, _, let due, let repeats, _) = try cards([["type": "reminder", "title": "Stretch", "timer_id": "t9", "due": at, "repeat": "every day"]]).first
        else { return XCTFail("a reminder card") }
        XCTAssertEqual(title, "Stretch"); XCTAssertTrue(repeats); XCTAssertTrue(due?.hasPrefix("Due ") == true)
        guard case .event(let timerMeta, _, _, _, _, _, _, _, _) = try cards([["type": "event", "kind": "timer", "what": "Tea", "id": "x1", "when": at]]).first
        else { return XCTFail("a timer card") }
        XCTAssertEqual(timerMeta, "Timer set")
        guard case .event(let changedMeta, _, _, _, _, _, _, let changes, _) = try cards([["type": "event", "kind": "schedule", "what": "News", "id": "x3", "changed": true,
                                                                                            "was_repeat": "every day", "repeat": "every Monday"]]).first
        else { return XCTFail("a schedule card") }
        XCTAssertEqual(changedMeta, "Schedule changed"); XCTAssertEqual(changes, ["Was: every day", "Now: every Monday"])
        guard case .note(let noteMeta, _, _) = try cards([["type": "note", "item_id": "m1", "text": "Likes tea", "state": "proposed"]]).first
        else { return XCTFail("a note card") }
        XCTAssertEqual(noteMeta, "Saved in my words · waits for your Keep")
        XCTAssertEqual(try cards([["type": "payment", "amount": "$5.00", "merchant": "Cafe", "site": "cafe.example", "card": "Visa (personal)"]]).first,
                       .payment(title: "Cafe", site: "cafe.example", savedCard: "Visa (personal)", amount: "$5.00"))
        guard case .send(let initials, _, let verb, _, let account) = try cards([["type": "send", "recipient": "Ana Lopez", "kind": "mailbox_send", "site": "mail.example", "account": "me@x.example"]]).first
        else { return XCTFail("a send card") }
        XCTAssertEqual(initials, "AL"); XCTAssertEqual(verb, "Emailed · mail.example"); XCTAssertEqual(account, "me@x.example")
        guard case .schedules(let meta, let rows) = try cards([["type": "schedules", "schedules": [["id": "s", "name": "News", "repeat": "every day", "state": "paused"]]]]).first
        else { return XCTFail("a schedules card") }
        XCTAssertEqual(meta, "1 schedule"); XCTAssertEqual(rows.first?.state, "paused"); XCTAssertEqual(rows.first?.rule, "Every day")
    }

    // decision: PHN-724
    func testAKindThisBuildDoesntKnowDrawsNothingAndItsLineStaysInTheWords() throws {
        let rows = ChatModel.rows(try view([message("m1", "r1", "pay it"),
                                            answer("a1", "r1", "m1", "Receipt saved\nAll done.", cards: [["type": "receipt", "line": "Receipt saved"]])]))
        let a = try XCTUnwrap(answers(rows).first).1
        XCTAssertTrue(a.above.isEmpty && a.below.isEmpty)
        XCTAssertTrue(ChatModel.words(.answer(key: "k", ui: a)).contains("Receipt saved"))
    }

    // decision: PHN-535, PHN-724
    func testAFileCardAndTheTasksFilesShowEachFileOnce() throws {
        let rows = ChatModel.rows(try view([message("m1", "r1", "make it"),
                                            answer("a1", "r1", "m1", "Made it.", cards: [["type": "file", "path": "/U/Desktop/a.pdf"]], artifacts: ["/U/Desktop/a.pdf", "/U/Desktop/b.txt"])]))
        let files = try XCTUnwrap(answers(rows).first).1.files
        XCTAssertEqual(files.map(\.title), ["a.pdf", "b.txt"])
        XCTAssertEqual(files.map(\.index), [0, 1])
    }

    // decision: PHN-770
    func testAnOpenQuestionShowsTheWordsTheMacsInlineQuestionLeadsWith() throws {
        let rows = ChatModel.rows(try view([message("m1", "r1", "send it"),
                                            entry("q1", "question", "r1", ["approval_id": "ap-1", "kind": "approval", "title": "Send the email to Ana?",
                                                                           "reason": "Ana is someone new.", "options": ["No", "Yes"]], task: "t-1", state: "open")]))
        let q = rows.compactMap { if case .asking(_, _, let t, let r, _) = $0 { return (t, r) } else { return nil } }.first
        // FERB.app's card: its header ("title") over its one question (the reason, with no summary): kb PHN-739
        XCTAssertEqual(q?.0, "Ana is someone new.")
        XCTAssertEqual(q?.1, "Send the email to Ana?")
    }

    // decision: PHN-722
    func testPlainWordsLaidOutHereEqualTheLibrarys() throws {
        var checked = 0
        var texts: [String] = ["One line", "Two\nlines\n\nand a paragraph", "  indented", "1 apple"]
        for c in try Fixtures.conversations() {
            for item in try FixtureMac.transcript(FixtureMac(c)).items {
                switch item {
                case .turn(let t): texts += [t.answer?.text, t.firstReply?.planLine, t.question].compactMap { $0 }
                case .notice(let n): texts.append(n.text)
                default: break
                }
            }
        }
        for t in texts {
            guard let fast = Present.plainOnly(t) else { continue }
            let lib = FerbMobile.richText(source: t).map { b -> String in if case .paragraph(let s) = b { return s.map(\.text).joined() } else { return "?" } }
            XCTAssertEqual(lib, fast.map { b -> String in if case .paragraph(let r) = b { return r.map(\.text).joined() } else { return "!" } }, t)
            checked += 1
        }
        XCTAssertGreaterThan(checked, 20)
    }

    // decision: PHN-725
    func testAOneLinePartTaskRunsInTheWorkRowWithWhatItDoesNow() throws {
        let v = try view([message("m1", "r1", "search online and then tell"),
                          entry("p1", "plan", "r1", ["text": "I’ll research it.", "plan_line": "I’ll research it.", "lines": [String](),
                                                     "name": "Research automation detection"], task: "t-1")])
        let r = try XCTUnwrap(RunningModel.of(v))
        XCTAssertEqual(r.words.lead, "1 active task")
        XCTAssertTrue(r.running)
        XCTAssertEqual(r.tasks.first?.title, "Research automation detection")
        XCTAssertEqual(r.tasks.first?.line, "Starting task…")
        XCTAssertEqual(r.tasks.first?.taskId, "t-1")
        XCTAssertNotNil(r.since)
    }

    // decision: PHN-726
    func testThePicturesComeAtTheirPaceForAtMostFourAndStopWhenTheyGo() async throws {
        let phone = FakePhone()
        let pics = Thumbnails(api: phone)
        // the work row's two and the card rows' pictures together: at most Thumbnails.most (4) at once
        pics.show(["t1", "t2", Thumbnails.key("t3", "t3.s1"), "t4", "t5"])
        for _ in 0..<50 where phone.thumbnails.count < 4 { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertEqual(Set(phone.calls.filter { $0.contains("thumbnail") }),
                       ["thumbnail:t1:2000", "thumbnail:t2:2000", "step-thumbnail:t3:t3.s1:2000", "thumbnail:t4:2000"])
        pics.show([])
        XCTAssertTrue(pics.pictures.isEmpty)
    }

    // decision: PHN-726
    func testARunningRowShowsItsStepsPictureAndTheWorkRowLeavesItsCircleOut() throws {
        let card: [String: Any] = ["request_id": "r1", "task_id": "t-1", "total": 2,
                                   "rows": [["part": "a", "label": "Review source materials", "state": "working", "step": "s1", "step_task_id": "t-1.s1"],
                                            ["part": "b", "label": "Write the summary", "state": "working", "step": "s2", "step_task_id": "t-1.s2"]]]
        var v = try view([message("m1", "r1", "two parts"),
                          entry("p1", "plan", "r1", ["text": "Two parts.", "plan_line": "Two parts.", "lines": [String](), "card": card], task: "t-1")])
        let lv = try XCTUnwrap(v.live.first)
        v.live = [LiveView(entry: lv.entry, requestId: lv.requestId, state: "running", typing: false, streamingPlan: nil, streamingReply: nil, replyNotice: nil,
                           line: nil, activity: nil,
                           rows: [LiveRow(id: "a", kind: "retrying", words: "Trying again (attempt 2) · reading", spinner: true, until: nil),
                                  LiveRow(id: "b", kind: "waiting", words: "Waiting for Review source materials", spinner: false, until: nil)],
                           checklist: [], openQuestions: [], current: "Reading", cardLine: "0 of 2 done · 1 running · 1 waiting")]
        let task = try XCTUnwrap(ChatModel.rows(v).compactMap { if case .task(let t) = $0 { return t } else { return nil } }.first)
        XCTAssertEqual(task.watch, ["t-1.s1", nil], "only the part whose step works shows a picture")
        XCTAssertEqual(task.status, "0 of 2 done · 1 running · 1 waiting")
        let rig = Rig()
        defer { rig.tearDown() }
        let m = rig.model()
        m.rowPictures(task.key, [Thumbnails.key("t-1", "t-1.s1")])
        XCTAssertEqual(m.tasksWithRowPictures, ["t-1"], "the work row leaves this task's circle out (APP-891)")
        m.rowPictures(task.key, [])
        XCTAssertTrue(m.tasksWithRowPictures.isEmpty)
    }

    /// Every sheet and page follows the owner's Block screenshots switch (owner 2026-11-23): Watch live, a task's short
    /// view and the Mac's camera too. Only a private answer's box hides from captures whatever the
    /// switch says.
    // decision: PHN-810, PHN-811, PHN-110
    func testEveryPageFollowsTheSwitchAndOnlyAPrivateAnswerAlwaysHides() throws {
        let rig = Rig()
        defer { rig.tearDown() }
        let m = rig.model()
        let every: [AppSheet] = [.relay, .profile, .forget, .typeCode, .scan, .ownRelay, .notifications, .help, .stopAll, .settings,
                                 .connection, .days(key: "k"), .live(taskId: "t-1"), .task(taskId: "t-1", worker: nil),
                                 .task(taskId: "t-1", worker: "t-1.s1"), .sites(row: "r"), .voice(.onDevice), .location(requestId: "l"), .city,
                                 .background, .insomnia, .camera]
        for on in [true, false] {
            m.blockScreenshots(on)
            XCTAssertEqual(m.settings.blockScreenshots, on)
            XCTAssertFalse(m.mustSecure, "no sheet: the switch alone")
            for sheet in every {
                m.show(sheet)
                XCTAssertFalse(m.mustSecure, "\(sheet) follows the switch (\(on))")
            }
            m.show(nil)
            m.hiddenShown("a-1", true)
            XCTAssertTrue(m.mustSecure, "a private answer's box hides from captures whatever the switch says")
            m.show(.live(taskId: "t-1"))
            XCTAssertTrue(m.mustSecure)
            m.hiddenShown("a-1", false)
            XCTAssertFalse(m.mustSecure)
            m.show(nil)
        }
    }

    private func finishedCard() throws -> [ChatRow] {
        let second = (1...12).map { "Line \($0) of the second part's answer, with a source: [Chrome docs](https://developer.chrome.com/docs) and **a bold term**." }.joined(separator: "\n")
        let card: [String: Any] = ["request_id": "r1", "task_id": "t-1", "total": 2, "done": 2, "line": "2 of 2 done", "lines": ["Sites contacted: developer.chrome.com"],
                                   "rows": [["part": "a", "label": "Find the extension rules", "state": "done", "step": "s1", "text": "Extensions may automate pages only with the user's consent. See [the policy](https://developer.chrome.com/policy)."],
                                            ["part": "b", "label": "Check what sites can see", "state": "done", "step": "s2", "text": second, "note": "the rate limits"]]]
        let v = try view([message("m1", "r1", "two parts"),
                          entry("p1", "plan", "r1", ["text": "Two parts.", "plan_line": "Two parts.", "lines": [String](), "card": card], task: "t-1"),
                          answer("a1", "r1", "m1", "1. Extensions…\n2. Sites…\n\nSites contacted: developer.chrome.com", lines: ["Sites contacted: developer.chrome.com"],
                                 cards: [["type": "link", "host": "developer.chrome.com", "line": "Sites contacted: developer.chrome.com"]])])
        return ChatModel.rows(v)
    }

    // decision: PHN-733
    func testAFinishedCardShowsEachPartsAnswerAsTheMacDoes() throws {
        let rows = try finishedCard()
        let t = try XCTUnwrap(rows.compactMap { if case .task(let t) = $0 { return t } else { return nil } }.first)
        XCTAssertNotNil(t.parts[0].rich, "a done part shows its answer, not only its tick")
        XCTAssertNil(t.parts[0].detail)
        XCTAssertEqual(t.parts[1].notes.first?.label, "Couldn't confirm: ")
        XCTAssertEqual(t.footer, "2 of 2 done")
        XCTAssertTrue(t.footerLines.isEmpty, "the sites line is the chip under the card")
        XCTAssertTrue(rows.contains { if case .answer(_, let ui) = $0 { return ui.sites == ["developer.chrome.com"] } else { return false } })
    }

    // decision: PHN-733
    func testGoldenAFinishedTwoPartCardFoldedAndOpen() throws {
        let rig = Rig()
        defer { rig.tearDown() }
        let m = rig.model()
        let rows = try finishedCard().filter { if case .owner = $0 { return false }; if case .rich = $0 { return false }; return true }
        let v = VStack(spacing: 12) { ForEach(rows) { ChatRowView(model: m, row: $0, today: chatDayOf(m.ui), olderLoading: false) } }
        for (hour, name) in [(6.5, "dawn"), (12.5, "noon"), (22.5, "night")] { shot(v, "card-finished-folded-\(name)", height: 1000, hour: hour) }
        shot(v.environment(\.foldsOpen, true), "card-finished-open-night", height: 1100)
    }

    // MARK: goldens (Screenshots/app/parity)

    /// On the lab's sky at `hour` (dawn 6.5, noon 12.5, night 22.5): FERB's dark surface must part from each (kb PHN-734).
    private func shot<V: View>(_ v: V, _ name: String, height: CGFloat = 874, hour: Double = 22.5) {
        let page = PhoneScreen(moment: .lab(hour), glide: false) {
            v.padding(.horizontal, 16).padding(.top, 24).frame(maxHeight: .infinity, alignment: .top)
        }
        .frame(width: 402, height: height)
        .environment(\.chatViewport, 700)
        .environment(\.cardClock, Date(timeIntervalSince1970: 1_767_340_800))
        .stillMotion()
        AppSnapshot.assert(page, size: CGSize(width: 402, height: height), path: "app/parity/\(name).png")
    }

    // decision: PHN-721, PHN-723
    func testGoldenTheResearchAnswerFolded() throws {
        let rig = Rig()
        defer { rig.tearDown() }
        let m = rig.model()
        let rows = ChatModel.rows(try research())
        let (key, ui) = try XCTUnwrap(answers(rows).first)
        shot(VStack(spacing: 12) { ForEach(rows.filter { if case .rich = $0 { return true }; return false }) { ChatRowView(model: m, row: $0, today: chatDayOf(m.ui), olderLoading: false) }; AnswerRow(model: m, key: key, a: ui) }, "answer-folded")
    }

    // decision: PHN-724
    func testGoldenEveryCardKind() throws {
        let at = "2026-01-02T09:00:00+00:00"
        let all = try cards([
            ["type": "reminder", "title": "Stretch", "timer_id": "t9", "due": at, "repeat": "every day", "next_run": "2026-01-03T09:00:00+00:00"],
            ["type": "event", "kind": "timer", "what": "Tea", "id": "x1", "when": at],
            ["type": "event", "kind": "schedule", "what": "Morning news", "id": "x3", "changed": true, "was_repeat": "every day at 8:00", "repeat": "every Monday at 8:00"],
            ["type": "note", "item_id": "m1", "text": "Prefers window seats", "state": "saved"],
            ["type": "payment", "amount": "$24.00", "merchant": "Blue Bottle", "site": "bluebottle.example", "card": "Visa (personal)"],
            ["type": "send", "recipient": "Ana Lopez", "kind": "mailbox_send", "site": "mail.example", "account": "me@example.com"],
        ])
        let v = VStack(spacing: 8) { ForEach(Array(all.enumerated()), id: \.offset) { _, c in ResultCardView(c) } }.ferbSurface()
        for (hour, name) in [(6.5, "dawn"), (12.5, "noon"), (22.5, "night")] { shot(v, "cards-\(name)", height: 1100, hour: hour) }
    }

    // decision: PHN-723, PHN-725
    func testGoldenTheSitesRowTheWorkRowAndAShortView() throws {
        let r = RunningUi(words: .of(active: 3, paused: 0, kept: 0), running: true, since: Date(timeIntervalSince1970: 1_767_340_800 - 42),
                          tasks: [RunTask(entry: "m1", taskId: "t-1", title: "Research automation detection", line: "Searching for extension detection", waiting: false, paused: false),
                                  RunTask(entry: "m2", taskId: "t-2", title: "Compare desk prices", line: "Reading the Flexispot page", waiting: false, paused: false),
                                  RunTask(entry: "m3", taskId: "t-3", title: "Book the dentist", line: "Starting task…", waiting: false, paused: false)])
        let pic = UIGraphicsImageRenderer(size: CGSize(width: 40, height: 24)).image { c in UIColor.systemBlue.setFill(); c.fill(CGRect(x: 0, y: 0, width: 40, height: 24)) }
        shot(VStack(spacing: 24) {
            SitesChip(sites: hosts, onOpen: { _ in }, onAll: {})
            ActiveWorkRow(words: r.words, running: true, since: r.since,
                          tasks: r.tasks.enumerated().map { i, t in WorkTask(id: t.entry, title: t.title, line: t.line, picture: i < 2 ? pic : nil) },
                          onPicture: { _ in }, onMore: {}, onViewTasks: {})
            TaskShortView(task: WorkTask(id: "t-1", title: "Research automation detection", line: "Searching for extension detection"),
                          empty: nil, onWatch: {}, onStop: {})
                .padding(16).background(.white, in: RoundedRectangle(cornerRadius: 28))
            PartsCard(headline: "On it. Three parts.", progress: "1 of 3", parts: [
                PlanPart(kind: .done, title: "Open your three desks", detail: "3 pages read"),
                PlanPart(kind: .running, title: "Compare price and delivery", detail: "Reading the Flexispot page"),
                PlanPart(kind: .waiting, title: "Put the cheapest in your cart", detail: "Waiting for Compare price and delivery"),
            ], status: "1 of 3 done · 1 running · 1 waiting", watching: [false, true, false], pictures: [nil, pic, nil]).ferbSurface()
        }, "work-and-sites", height: 1000)
    }

    /// FERB.app's card: amber while it asks, the steps a waiting row waits on under it, the connection line's time.
    // decision: PHN-736, PHN-737
    func testACardAsksInAmberShowsItsWaitingRowsStepsAndTheLineTicks() throws {
        let card: [String: Any] = ["request_id": "r1", "task_id": "t-1", "total": 1,
                                   "plan": [["id": "s1", "label": "Read the notes", "step_task_id": "t-1.s1"],
                                            ["id": "s2", "label": "Write the email", "step_task_id": "t-1.s2", "depends_on": ["s1"]]],
                                   "rows": [["part": "a", "label": "Draft the email", "state": "asks", "step": "s2"]]]
        var v = try view([message("m1", "r1", "draft it"),
                          entry("p1", "plan", "r1", ["text": "On it.", "plan_line": "On it.", "lines": [String](), "card": card], task: "t-1")])
        var lv = try XCTUnwrap(v.live.first)
        lv.rows = [LiveRow(id: "a", kind: "waiting", words: "Waiting for Read the notes", spinner: false, until: nil)]
        lv.feeders = [LiveFeeder(part: "a", step: "s1", label: "Read the notes", stepTaskId: "t-1.s1",
                                 line: LiveRow(id: "s1", kind: "running", words: "Reading", spinner: true, until: nil))]
        lv.line = LiveLine(text: "Reconnected to the service", startedAt: "2026-01-02T09:00:00+00:00")
        v.live = [lv]
        let rows = ChatModel.rows(v)
        let t = try XCTUnwrap(rows.compactMap { if case .task(let t) = $0 { return t } else { return nil } }.first)
        XCTAssertEqual(t.tint, .asking)
        XCTAssertEqual(t.parts.first?.sub.map(\.title), ["Read the notes"])
        // the running step shows its own picture (FERB.app's feederLines)
        XCTAssertEqual(t.subWatch, [["t-1.s1"]])
        let since = rows.compactMap { if case .line(let k, _, _, let s) = $0, k.hasSuffix(":line") { return s } else { return nil } }.first
        XCTAssertEqual(since, Date(timeIntervalSince1970: 1_767_344_400))
    }

    /// Two rows waiting on the same running step: its picture shows under the first only (FERB.app's feederLines,
    /// APP-891's once), a step that ended shows none, and each keeps its own mark.
    // decision: PHN-736, PHN-737, PHN-851
    func testAStepTwoRowsWaitOnShowsItsPictureOnceAndAnEndedStepNone() throws {
        let card: [String: Any] = ["request_id": "r1", "task_id": "t-1", "total": 2,
                                   "plan": [["id": "s1", "label": "Read the notes", "step_task_id": "t-1.s1"],
                                            ["id": "s0", "label": "Open the folder", "step_task_id": "t-1.s0"],
                                            ["id": "s2", "label": "Write the email", "step_task_id": "t-1.s2", "depends_on": ["s0", "s1"]],
                                            ["id": "s3", "label": "Write the memo", "step_task_id": "t-1.s3", "depends_on": ["s1"]]],
                                   "rows": [["part": "a", "label": "Draft the email", "state": "waiting", "step": "s2"],
                                            ["part": "b", "label": "Draft the memo", "state": "waiting", "step": "s3"]]]
        var v = try view([message("m1", "r1", "draft both"),
                          entry("p1", "plan", "r1", ["text": "On it.", "plan_line": "On it.", "lines": [String](), "card": card], task: "t-1")])
        var lv = try XCTUnwrap(v.live.first)
        lv.rows = [LiveRow(id: "a", kind: "waiting", words: nil, spinner: false, until: nil),
                   LiveRow(id: "b", kind: "waiting", words: nil, spinner: false, until: nil)]
        let reading = LiveRow(id: "s1", kind: "running", words: "Reading", spinner: true, until: nil)
        lv.feeders = [LiveFeeder(part: "a", step: "s0", label: "Open the folder", stepTaskId: "t-1.s0",
                                 line: LiveRow(id: "s0", kind: "done", words: nil, spinner: false, until: nil)),
                      LiveFeeder(part: "a", step: "s1", label: "Read the notes", stepTaskId: "t-1.s1", line: reading),
                      LiveFeeder(part: "b", step: "s1", label: "Read the notes", stepTaskId: "t-1.s1", line: reading)]
        v.live = [lv]
        let t = try XCTUnwrap(ChatModel.rows(v).compactMap { if case .task(let t) = $0 { return t } else { return nil } }.first)
        XCTAssertEqual(t.parts.map { $0.sub.map(\.kind) }, [[.done, .running], [.running]])
        XCTAssertEqual(t.subWatch, [[nil, "t-1.s1"], [nil]])
    }

    /// A reply still coming says so; the work row also counts the Mac's tasks started elsewhere.
    // decision: PHN-736
    func testAStreamedReplyIsADraftAndTheWorkRowCountsTheMacsOtherTasks() throws {
        var v = try view([entry("m1", "message", "r1", ["text": "hello", "attachments": [String](), "hidden": [String]()])])
        var lv = try XCTUnwrap(v.live.first)
        lv.streamingReply = "Hel"
        v.live = [lv]
        XCTAssertTrue(ChatModel.rows(v).contains { if case .small(_, let text, _) = $0 { return text == "Draft · still responding" } else { return false } })
        // the Mac's live status says what runs elsewhere (kb PHN-792)
        let r = try XCTUnwrap(RunningModel.of(v, status: running([StatusTask(taskId: "t-9", name: "Morning news", goal: "Read the news",
                                                                              startedAt: nil, held: false)])))
        XCTAssertEqual(r.words.lead, "1 active task")
        XCTAssertEqual(r.tasks.map(\.title), ["Morning news"])
    }

    private func running(_ tasks: [StatusTask]) -> PhoneStatus {
        PhoneStatus(locked: false, words: true, stopped: false, questions: [], running: tasks, results: [], screenUse: nil)
    }

    private func macRow(_ id: String, _ status: String) -> TaskRow {
        TaskRow(taskId: id, name: "Morning news", goal: "Read the news", status: status, createdAt: nil, updatedAt: nil, answer: "",
                stepsDone: nil, stepsTotal: nil)
    }

    /// The owner's phone at 10:25 on 2026-11-21: "1 active task" with its spinner after every task had finished. The Tasks
    /// tab had read `tasks.list` while the task ran and was never read again; once the chat's turn had its answer the stale
    /// row counted. The Mac's live status, pushed on every change, decides now.
    // decision: PHN-792
    func testATaskTheMacNoLongerRunsNeverCountsFromTheTasksTabsLastRead() throws {
        let done = try view([message("m1", "r1", "start a 3 part parallel work"),
                             entry("p1", "plan", "r1", ["text": "Starting three parts.", "plan_line": "Starting three parts.", "lines": [String]()], task: "t-1"),
                             answer("a1", "r1", "m1", "All three parts are done.")])
        XCTAssertTrue(done.live.isEmpty)
        let stale = [macRow("t-1", "working")]
        XCTAssertNil(RunningModel.of(done, status: running([]), mac: stale))
        XCTAssertNil(RunningModel.of(done, status: nil, mac: stale))
        // one the Mac runs that this chat doesn't show counts; the Tasks tab's list only says it is paused
        let elsewhere = [StatusTask(taskId: "t-2", name: "Morning news", goal: "Read the news", startedAt: nil, held: false)]
        let r = try XCTUnwrap(RunningModel.of(done, status: running(elsewhere), mac: stale))
        XCTAssertEqual(r.words.lead, "1 active task")
        XCTAssertEqual(r.tasks.map(\.taskId), ["t-2"])
        let paused = try XCTUnwrap(RunningModel.of(done, status: running(elsewhere), mac: [macRow("t-2", "paused")]))
        XCTAssertEqual(paused.words.lead, "1 paused task")
    }

    private func states() -> some View {
        let words: (String) -> [FerbDesign.RichBlock] = { [.paragraph([RichRun($0)])] }
        return VStack(spacing: 12) {
            PartsCard(headline: "Microsoft outreach email", progress: "0 of 3", parts: [
                PlanPart(kind: .running, title: "Review source materials", detail: "Trying again (attempt 3) · reading the outreach notes from the shared folder"),
                PlanPart(kind: .waiting, title: "Draft the email", detail: "Waiting for Review source materials",
                         sub: [PlanPart(kind: .running, title: "Find the contact", detail: "Searching the address book")]),
                PlanPart(kind: .waiting, title: "Send it to the team", detail: "Waiting for Draft the email"),
            ], status: "0 of 3 done · 1 running · 2 waiting", watching: [true, false, false], pictures: [nil, nil, nil],
               subWatching: [[], [true], []])
            PartsCard(headline: "Book the dentist", progress: "1 of 2", parts: [
                PlanPart(kind: .done, title: "Find a free slot", rich: words("Tuesday at 10 is free.")),
                PlanPart(kind: .asking, title: "Book it", detail: "Needs you", tone: .amber),
            ], tint: .asking)
            PartsCard(headline: "Tidy the Desktop", progress: "1 of 3", parts: [
                PlanPart(kind: .failed, title: "Move the screenshots", detail: "Couldn't do it: the folder is protected.", tone: .red),
                PlanPart(kind: .failed, title: "Empty the old downloads", detail: "Stopped by you."),
                PlanPart(kind: .done, title: "Sort the PDFs", rich: words("12 PDFs moved to Documents/PDFs."),
                         notes: [PartNote(text: "Updated: you asked for Documents", tone: .blue)], updated: true),
            ], tint: .failed, footer: "1 of 3 done", footerLines: ["1 stopped by you"])
        }.ferbSurface()
    }

    // decision: PHN-737, PHN-734, PHN-735
    func testGoldenTheCardOfPartsInEachStateOnThreeSkies() throws {
        for (hour, name) in [(6.5, "dawn"), (12.5, "noon"), (22.5, "night")] { shot(states(), "card-states-\(name)", height: 900, hour: hour) }
    }

    /// Update fingerprint's card (PHN-849): the owner's words with the scanner's button, a No's reason, and while the
    /// Mac asks, the six digits to compare.
    // decision: PHN-849
    func testGoldenTheFingerprintCard() throws {
        shot(VStack(spacing: 12) {
            FingerprintCard(step: .changed(problem: nil), busy: false, bio: .faceID) {}
            FingerprintCard(step: .changed(problem: Note("err_mac_said", "The fingerprint update was not approved on the Mac. On the Mac, choose Update fingerprint again and say Yes when it asks")),
                            busy: false, bio: .faceID) {}
            FingerprintCard(step: .waiting(code: "123 456"), busy: false) {}
            FingerprintCard(step: .waiting(code: nil), busy: false) {}
        }, "fingerprint-card", height: 1000, hour: 12.5)
    }

    // decision: PHN-737, PHN-738
    func testGoldenEightPartsAndASinglePartChecklist() throws {
        let eight = (1...8).map { n -> PlanPart in
            n <= 3 ? PlanPart(kind: .done, title: "Part \(n)", rich: [.paragraph([RichRun("Part \(n) is done.")])])
                : n == 4 ? PlanPart(kind: .running, title: "Part 4", detail: "Reading the venue's page")
                : PlanPart(kind: .waiting, title: "Part \(n)", detail: "Waiting for Part 4")
        }
        shot(VStack(spacing: 12) {
            PartsCard(headline: "Plan the offsite", progress: "3 of 8", parts: eight, status: "3 of 8 done · 1 running · 4 waiting")
            PartsCard(headline: "Check the train times", progress: "0 of 2", parts: [
                PlanPart(kind: .running, title: "Open the timetable", detail: "Reading the 18:05 departures"),
                PlanPart(kind: .waiting, title: "Tell you the next three trains")])
        }.ferbSurface(), "card-eight-and-checklist", height: 900)
    }

    // MARK: questions (kb PHN-739)

    private func question(_ id: String, _ body: String) -> Question {
        Questions.parse(QuestionItem(entry: "e-\(id)", approvalId: id, requestId: "r-1", taskId: "t-1", state: "open", open: true,
                                     bodyJson: body, shown: "end_of_chat", row: nil))
    }

    /// One body per kind, as the gateway sends them (engine/fixtures/gateway/scenarios.json's shapes); Android's ParityShotsTest has the same.
    private let kinds: [(String, String)] = [
        ("sink", #"{"approval_id":"q-sink","capability":"ferb.confirm","kind":"owner_question","reason":"FERB: this types ps@microsoft.com, which FERB found only in outside content (the web page on mail.google.com; the web page on mailmeteor.com), not in your words, your answers or your own files. The step: browser.act action=type tab=t69 element=s13e633 text=ps@microsoft.com mode=insert. Reason given: \"Add Pratyay as the first main To recipient.\". Go ahead?","options":["Yes","No"],"summary":"Type this email address on mail.google.com?","why":"Add Pratyay as the first main To recipient.","detail":"FERB found it only in outside content (the web page on mail.google.com; the web page on mailmeteor.com), not in your words, your answers or your own files.","exact":"ps@microsoft.com","title":"Check before FERB goes on","labels":["Type it","Don't type it"],"corrects":true}"#),
        ("money", #"{"approval_id":"q-money","capability":"ferb.confirm","kind":"owner_question","reason":"FERB: this pays 5 to x@y.com. The step: probe.pay to=x@y.com. Go ahead?","options":["Yes","No"],"summary":"Pay 5 to x@y.com?","detail":"Money leaves your account. Yes covers only this payment.","exact":"5 → x@y.com","picture_problem":"FERB couldn't capture the payment page.","title":"Check before FERB goes on","labels":["Pay 5","Don't pay"],"corrects":true}"#),
        ("terminal", #"{"approval_id":"q-terminal","capability":"ferb.confirm","kind":"owner_question","reason":"FERB: this runs a command. The step: shell.run. Go ahead?","options":["Yes","No"],"summary":"Run this command in Terminal?","detail":"FERB types it into Terminal and presses Return.","exact":"brew upgrade --cask firefox","title":"Check before FERB goes on","labels":["Run it","Don't run"],"note":"Your Yes covers Terminal for the rest of this task."}"#),
        ("retry", #"{"approval_id":"q-retry","capability":"ferb.retry","kind":"owner_question","reason":"FERB: the last try may have sent it. Try again?","options":["Yes","No"],"summary":"Try sending again?","why":"I found no sent copy; try again?","detail":"FERB lost track of the last try, so it may already have worked; trying again could send it twice.","title":"Check before FERB retries","labels":["Try again","Don't retry"]}"#),
        ("task", #"{"approval_id":"q-task","capability":"owner.ask","kind":"owner_question","reason":"Which size should I order?","options":["Small","Medium","Large"],"from_task":true,"title":"Your task has a question"}"#),
        ("code", #"{"approval_id":"q-code","capability":"browser.login_code","kind":"owner_login_code","reason":"x","options":[],"summary":"otp.example.com asks for a login code","why":"the site sent a code","detail":"The page asks for “Enter the 6-digit code”. Type the code the site sent you.","title":"Private — only the page gets this","field_label":"Code"}"#),
    ]

    /// The owner's case: the Mac's question, its reason and the address; never the step line.
    // decision: PHN-739
    func testAQuestionReadsAsTheMacsCardInTheSheetAndTheRow() throws {
        let q = question("q-sink", kinds[0].1)
        XCTAssertEqual(q.title, "Type this email address on mail.google.com?")
        XCTAssertEqual(q.shown?.header, "Check before FERB goes on")
        XCTAssertEqual(q.shown?.why, "The task's reason: Add Pratyay as the first main To recipient.")
        XCTAssertEqual(q.choices.map(\.label), ["Don't type it", "Type it"])
        var v = try view([message("m1", "r1", "write to Pratyay")])
        v.questions = [QuestionItem(entry: "e-q", approvalId: "q-sink", requestId: "r1", taskId: "t-1", state: "open", open: true,
                                    bodyJson: kinds[0].1, shown: "end_of_chat", row: nil)]
        let row = ChatModel.rows(v).compactMap { if case .asking(_, _, let t, let r, _) = $0 { return (t, r) } else { return nil } }.first
        XCTAssertEqual(row?.0, "Type this email address on mail.google.com?")
        XCTAssertEqual(row?.1, "Check before FERB goes on")
    }

    // decision: PHN-739, PHN-770
    func testGoldenEachKindOfQuestionReadsAsTheMacsCard() throws {
        let rig = Rig()
        defer { rig.tearDown() }
        let m = rig.model()
        for (name, body) in kinds {
            let q = question("q-\(name)", body)
            // the box in the chat (kb PHN-770), as the chat draws it
            let box = QuestionBox(model: m, approvalId: q.approvalId, title: q.title, reason: nil)
                .environment(\.questions, [q])
            shot(box, "question-\(name)", height: 874)
        }
    }

    private func answerEntry(_ id: String, _ request: String, _ of: String, _ text: String, lines: [String], note: String?, status: String) -> [String: Any] {
        entry(id, "answer", request, ["from": "task", "text": text, "status": status, "verified": true, "artifacts": [String](), "note": note ?? NSNull(),
                                      "lines": lines, "cards": [[String: Any]]()], task: "t-1", answers: of)
    }

    /// An answer delivered with parts not done: its lines small and grey, "Not done:" and "Couldn't confirm" amber, the
    /// status words small and amber under it (FERB.app's sizes and colours; kb PHN-773, APP-978), on three skies.
    // decision: PHN-773, APP-978
    func testGoldenAnAnswerWithGapsSaysItsLinesAndStatusSmallerInTheirColours() throws {
        let uncertain = "I'm not sure whether my last step on mail.example went through: I couldn't tell how it ended. I haven't tried it again, so nothing can happen twice."
        let text = "Left the unsent draft open for your review. It includes the requested content and leaves Sam out.\n\n"
            + "Not done: the main people are in To and the others in CC — you asked to stop before the duplicate was removed.\n\n"
            + "Couldn't confirm: the draft saved.\n" + uncertain
        let rows = ChatModel.rows(try view([message("m1", "r1", "Draft the email to the team"),
                                            answerEntry("a1", "r1", "m1", text, lines: [uncertain], note: "the draft saved", status: "done_with_gaps")]))
        XCTAssertTrue(rows.contains { if case .small(_, "Delivered, with parts not done", true) = $0 { return true } else { return false } })
        let rig = Rig()
        defer { rig.tearDown() }
        let m = rig.model()
        let v = VStack(spacing: 12) { ForEach(rows.filter { if case .owner = $0 { return false }; return true }) { ChatRowView(model: m, row: $0, today: 0, olderLoading: false) } }
        for (hour, name) in [(6.5, "dawn"), (12.5, "noon"), (22.5, "night")] { shot(v, "answer-gaps-\(name)", height: 700, hour: hour) }
    }
}
#endif
