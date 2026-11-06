#if FERB_PLAYGROUND
import Foundation
import NotchCore

/// Play 0.27.1 (APP-754, owner 2026-11-06: "jsut make a fake message adn whoel UI play"): a scripted, invented
/// conversation played through the REAL notch and chat, beat by beat with pauses, so the owner only watches. Every
/// event goes through the same seams the daemon uses (chat.send's stream, the bus, chat.status, the notification
/// poster); the owner's side (sending, dropping files, answering questions, settings) goes through NotchModel's own
/// methods. Nothing calls a model, the real daemon, ~/.ferb or the network. Each beat may carry a check of the UI state
/// it should leave (`PlayProbe`), which the hermetic test runs (PlaygroundPlayTests) and the player notes in the log.

/// What the play needs from the app around the notch (the playground's PanelCoordinator and window, or a test's stand-in).
@MainActor
protocol PlayHost: AnyObject {
    func openNotch()
    func closeNotch()
    func showSettings(_ tab: SettingsTab)
    func closeSettings()
    /// Where FERB opens (APP-745), Keep on top and Hide the pill: the playground's own preferences.
    func setChatHost(_ host: ChatHost)
    func setKeepOnTop(_ on: Bool)
    func setHidePill(_ on: Bool)
}

/// One beat: what the caption says, how long before it (seconds at 1x), what it does and what it should leave.
@MainActor
struct PlayBeat {
    let caption: String
    let wait: Double
    let run: (PlaygroundPlayer) async -> Void
    var check: ((PlayProbe) -> String?)? = nil
}

/// What a check reads: the real model's state after a beat.
@MainActor
struct PlayProbe {
    let model: NotchModel
    let ids: PlayIDs

    var turn: ConversationTurn? { model.conversation.turns.first { $0.requestID == ids.request } }
    var card: TurnCard? { turn?.card }
    func turn(_ request: String) -> ConversationTurn? { model.conversation.turns.first { $0.requestID == request } }

    /// Each row as the card draws it now (RowDisplay, APP-718), with its question and live line.
    var displays: [RowDisplay] {
        guard let turn, let card = turn.card else { return [] }
        let questions = model.rowQuestions(turn)
        let live = model.runningTasks[card.taskID] != nil
        return card.shownRows.map { row in
            RowDisplay.make(row: row, steps: card.steps, rows: card.shownRows, live: live,
                            hasQuestion: !(questions[row.part] ?? []).isEmpty,
                            liveLine: row.stepTaskID.flatMap { model.stepLive[$0] })
        }
    }

    func display(_ part: String) -> RowDisplay? {
        guard let index = card?.shownRows.firstIndex(where: { $0.part == part }) else { return nil }
        let all = displays
        return index < all.count ? all[index] : nil
    }

    // MARK: The checks every beat of the card shares

    /// ORCH-543: every part the count includes has a row; APP-752: no clock in a row or the status line.
    func cardAgrees() -> String? {
        guard let card else { return "no card" }
        let rows = card.shownRows
        if card.total != rows.count { return "the count says \(card.total) but the card shows \(rows.count) rows" }
        if card.done != rows.filter(\.isDone).count {
            return "the count says \(card.done) done but \(rows.filter(\.isDone).count) rows are done"
        }
        let texts = displays.compactMap(\.words) + [CardStatusLine.text(card: card, displays: displays)]
        if let timed = texts.first(where: { $0.range(of: #"\d+:\d\d"#, options: .regularExpression) != nil }) {
            return "a timer shows: \(timed)"
        }
        return nil
    }

    func expect(_ part: String, _ kind: RowDisplay.Kind) -> String? {
        guard let shown = display(part)?.kind else { return "no row \(part)" }
        return shown == kind ? nil : "row \(part) shows \(shown.rawValue), expected \(kind.rawValue)"
    }

    static func all(_ checks: String?...) -> String? { checks.compactMap { $0 }.first }
}

/// The play's ids: a new run gets new ones, so a restart never mixes with the run before.
struct PlayIDs: Equatable {
    let run: Int
    var request: String { "play-\(run)-request" }
    var task: String { "play-\(run)-task" }
    var steer: String { "play-\(run)-steer" }
    var long: String { "play-\(run)-long" }
    var drop: String { "play-\(run)-drop" }
    var timer: String { "play-\(run)-timer" }
    func step(_ id: String) -> String { task + ".step." + id }
    func approval(_ id: String) -> String { "play-\(run)-approval-" + id }
}

/// One owner message the play sends: the fake daemon answers it from here instead of a sample state.
final class PlayFeed {
    let requestID: String
    /// Events said at once after the ack (a reply); nil keeps the turn open for the player's events (the task).
    let events: [[String: Any]]?
    let stream: AsyncStream<[String: Any]>
    let continuation: AsyncStream<[String: Any]>.Continuation

    init(requestID: String, events: [[String: Any]]?) {
        self.requestID = requestID
        self.events = events
        (stream, continuation) = AsyncStream<[String: Any]>.makeStream()
    }
}

/// Plays the script: Play, Pause, Restart. One beat at a time; `step()` runs the next beat now (the test's way in).
@MainActor
final class PlaygroundPlayer: ObservableObject {
    let workspace: PlaygroundWorkspace
    let model: NotchModel
    weak var host: PlayHost?
    @Published private(set) var caption = ""
    @Published private(set) var index = 0
    @Published private(set) var paused = false
    @Published private(set) var running = false
    @Published private(set) var failures: [String] = []
    /// Where the play shows the chat (the controls' Notch / Window); the window-mode beats come back to it.
    @Published var mode: ChatHost = .notch
    /// The play holds before the first beat whose caption starts with this (playground.sh --pause-before=…), once.
    var pauseBefore: String?
    private(set) var ids = PlayIDs(run: 1)
    private(set) var beats: [PlayBeat] = []
    private var task: Task<Void, Never>?
    private var seq = 0
    private var feed: PlayFeed?

    init(workspace: PlaygroundWorkspace, model: NotchModel, host: PlayHost?) {
        self.workspace = workspace
        self.model = model
        self.host = host
        beats = PlayScript.beats()
    }

    var count: Int { beats.count }
    var probe: PlayProbe { PlayProbe(model: model, ids: ids) }

    // MARK: Controls

    func play() {
        if running { paused = false; return }
        running = true
        paused = false
        task = Task { [weak self] in
            guard let self else { return }
            while self.index < self.beats.count, !Task.isCancelled {
                if let hold = self.pauseBefore, self.beats[self.index].caption.hasPrefix(hold) {
                    self.pauseBefore = nil
                    self.paused = true
                    self.caption = "Paused before: " + self.beats[self.index].caption
                    self.workspace.state.note("play: paused before \(self.index + 1)")
                }
                await self.workspace.state.pause(self.beats[self.index].wait)
                while self.paused, !Task.isCancelled { await self.workspace.state.pause(0.2) }
                guard !Task.isCancelled else { return }
                await self.step()
            }
            self.running = false
            if !Task.isCancelled { self.caption = "Play 0.27.1 finished" + (self.failures.isEmpty ? "" : " (\(self.failures.count) checks failed: see the log)") }
        }
    }

    func pause() { paused.toggle() }

    /// Starts again from the first beat with new ids; the chat keeps the earlier run above.
    func restart(andPlay: Bool = true) {
        task?.cancel()
        task = nil
        feed?.continuation.finish()
        feed = nil
        workspace.endPlay(task: ids.task)
        ids = PlayIDs(run: ids.run + 1)
        index = 0
        seq = 0
        failures = []
        running = false
        paused = false
        caption = ""
        if andPlay { play() }
    }

    /// Runs the next beat now and its check; returns the check's failure, if any.
    @discardableResult
    func step() async -> String? {
        guard index < beats.count else { return nil }
        let beat = beats[index]
        caption = "\(index + 1)/\(beats.count) · " + beat.caption
        workspace.state.note("play: " + caption)
        await beat.run(self)
        index += 1
        guard let check = beat.check else { return nil }
        // the real model applies events on the main actor as they arrive: give the stream a moment, then judge
        var failure: String?
        for _ in 0..<60 {
            failure = check(probe)
            if failure == nil { break }
            await Task.yield()
            try? await Task.sleep(for: .milliseconds(10))
        }
        if let failure {
            failures.append("\(beat.caption): \(failure)")
            workspace.state.note("play check failed: \(beat.caption): \(failure)")
        }
        return failure
    }

    // MARK: What beats do

    /// The owner sends `text` through the real composer path; the fake daemon answers it from a feed.
    func send(_ text: String, request: String, reply: [[String: Any]]? = nil) {
        let feed = PlayFeed(requestID: request, events: reply)
        if reply == nil { self.feed = feed }
        workspace.playTurns.append(feed)
        model.send(text)
    }

    /// The owner's dropped files and words sent from the composer (APP-737: the drop alone sends nothing).
    func sendComposer(_ words: String, request: String, reply: [[String: Any]]) {
        workspace.playTurns.append(PlayFeed(requestID: request, events: reply))
        model.session.draft = words
        model.startWork()
    }

    /// An event on the task's turn, as the daemon streams it.
    func push(_ event: [String: Any]) {
        var event = event
        event["request_id"] = ids.request
        event["task_id"] = event["task_id"] ?? ids.task
        feed?.continuation.yield(event)
    }

    func nextSeq() -> String {
        seq += 1
        return String(format: "2026-11-06T10:00:00.%06d+00:00", seq)
    }

    static var now: String { ISO8601DateFormatter().string(from: Date()) }

    func step(_ id: String, _ status: String, part: String?, waitsOn: [String] = [], attempt: Int = 1) {
        var event: [String: Any] = ["type": "step", "step": id, "status": status, "waits_on": waitsOn, "attempt": attempt,
                                    "started_at": Self.now, "step_task_id": ids.step(id), "seq": nextSeq()]
        if let part { event["part"] = part }
        push(event)
    }

    func live(_ id: String, _ text: String) {
        push(["type": "progress", "step_task_id": ids.step(id), "text": text, "started_at": Self.now])
    }

    func part(_ id: String, _ label: String, _ state: String, text: String? = nil, note: String? = nil,
              step: String? = nil, done: Int, total: Int, direct: Bool = false) {
        var event: [String: Any] = ["type": "part", "part": id, "label": label, "state": state, "seq": nextSeq(),
                                    "card_request_id": ids.request, "done": done, "total": total]
        if let text { event["text"] = text }
        if let note { event["note"] = note }
        if let step { event["step"] = step; event["step_task_id"] = ids.step(step) }
        if direct { event["direct"] = true }
        if state == "done" { event["verified"] = note == nil }
        push(event)
    }

    /// A question the task asks (bus: approval.requested), kept by chat.status until it is answered.
    func ask(_ approval: [String: Any]) {
        var approval = approval
        approval["task_id"] = ids.task
        workspace.playApprovals.insert(approval["approval_id"] as? String ?? "")
        workspace.approvals.append(approval)
        workspace.emit("approval.requested", approval)
    }

    /// The owner's answer to a question, through the card's own controls' model calls.
    func choose(_ id: String, option: Int) async {
        if let approval = await waitForApproval(id) { model.chooseApproval(approval, index: option) }
    }

    func answer(_ id: String, _ text: String) async {
        if let approval = await waitForApproval(id) { model.answerApproval(approval, text: text) }
    }

    func correct(_ id: String, _ text: String) async {
        if let approval = await waitForApproval(id) { model.correctApproval(approval, text: text) }
    }

    private func waitForApproval(_ id: String) async -> DaemonApproval? {
        for _ in 0..<200 {
            if let found = model.pendingApprovals.first(where: { $0.id == id }), !model.sendingAnswer { return found }
            try? await Task.sleep(for: .milliseconds(25))
        }
        failures.append("question \(id) never reached the notch")
        return nil
    }

    /// Ends the task's turn with its outcome.
    func finish(_ outcome: [String: Any]) {
        push(outcome)
        feed?.continuation.finish()
        feed = nil
        workspace.endPlay(task: ids.task)
    }

    /// Invented scratch files in the playground's own temporary home, for the drop.
    func scratchFiles() -> [URL] {
        let folder = workspace.state.home.appendingPathComponent("Play files", isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return [("trip-notes.txt", "PLAYGROUND SAMPLE\nTrain at 9:10, hotel near the station.\n"),
                ("packing-list.md", "PLAYGROUND SAMPLE\n- charger\n- rain jacket\n")].map { name, text in
            let url = folder.appendingPathComponent(name)
            try? text.write(to: url, atomically: true, encoding: .utf8)
            return url
        }
    }
}

/// The script: every 0.27.1 change that shows in the UI, in one invented conversation.
@MainActor
enum PlayScript {
    static let request = "Read the email list on my Desktop, write a one-line summary to summary.txt, tell me today's date "
        + "in Tokyo, find two reviews of the Arc keyboard, order the one you'd pick, and put a typing practice slot in my calendar"
    static let longRows = (1...12).map { "\($0). Sample sender \($0) <sender\($0)@example.com>: invented update about the launch." }
        .joined(separator: "\n")

    static func rows(_ ids: PlayIDs) -> [[String: Any]] {
        [["part": "emails", "label": "Read email list", "state": "working", "step": "read", "step_task_id": ids.step("read")],
         ["part": "summary", "label": "Write summary", "state": "working", "step": "write", "step_task_id": ids.step("write")],
         ["part": "tokyo", "label": "Tokyo date", "state": "done", "direct": true,
          "text": "Today in Tokyo it's Saturday, 7 November.", "seq": "2026-11-06T10:00:00.000000+00:00"],
         ["part": "reviews", "label": "Keyboard reviews", "state": "working", "step": "reviews",
          "step_task_id": ids.step("reviews")],
         ["part": "order", "label": "Order the keyboard", "state": "working", "step": "order", "step_task_id": ids.step("order")],
         ["part": "practice", "label": "Typing practice slot", "state": "working", "step": "practice",
          "step_task_id": ids.step("practice")]]
    }

    // swiftlint:disable:next function_body_length
    static func beats() -> [PlayBeat] {
        [
            PlayBeat(caption: "FERB says it was updated, once", wait: 0.6, run: { p in
                p.model.preferences.set("0.27.0", forKey: NotchModel.lastVersionKey)
                p.model.noteUpdate(info: ["CFBundleShortVersionString": "0.27.1", "CFBundleVersion": "play"])
                p.model.noteUpdate(info: ["CFBundleShortVersionString": "0.27.1", "CFBundleVersion": "play-again"])
            }, check: { probe in
                probe.model.conversation.notices.filter { $0.id == "update-0.27.1" }.count == 1 ? nil : "no single Updated line"
            }),
            PlayBeat(caption: "FERB opens (in the notch, or the sticky window)", wait: 1.2, run: { p in
                p.host?.setChatHost(p.mode)
                p.host?.openNotch()
            }),
            PlayBeat(caption: "A request with six parts", wait: 1.0, run: { p in
                p.send(request, request: p.ids.request)
                p.workspace.startPlay(task: p.ids.task, request: p.ids.request, goal: request)
            }, check: { probe in probe.turn == nil ? "the message didn't show" : nil }),
            PlayBeat(caption: "The plan: one row per part; Tokyo's date is answered at once as its own done row", wait: 1.6, run: { p in
                p.push(["type": "plan", "goal": request, "name": "Inbox, keyboard and practice",
                        "text": "I'll read the list, write the summary, research and order a keyboard, and find a practice slot.\nWorking in ~/Desktop.",
                        "plan_line": "I'll read the list, write the summary, research and order a keyboard, and find a practice slot.",
                        "lines": ["Working in ~/Desktop."],
                        "direct": [["part": "tokyo", "label": "Tokyo date", "text": "Today in Tokyo it's Saturday, 7 November."]],
                        "card": ["request_id": p.ids.request, "rows": rows(p.ids), "done": 1, "total": 6,
                                 "steps": ["read": ["status": "starting"], "write": ["status": "waiting", "waits_on": ["read"]],
                                           "reviews": ["status": "starting"], "order": ["status": "waiting", "waits_on": ["reviews"]],
                                           "practice": ["status": "waiting", "waits_on": ["slot"]]]]])
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("tokyo", .done),
                                               probe.turn?.shownDirect.isEmpty == true ? nil : "the date shows above the card",
                                               probe.expect("summary", .waiting), probe.expect("practice", .waiting)) }),
            PlayBeat(caption: "Rows start, then run with a spinner and a live line (no timer)", wait: 1.4, run: { p in
                p.step("read", "running", part: "emails")
                p.live("read", "reading Desktop/imp mails.txt")
                p.step("reviews", "running", part: "reviews")
                p.live("reviews", "reading reviews.example")
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("emails", .running), probe.expect("reviews", .running)) }),
            PlayBeat(caption: "A row waiting on a step that has no row says so", wait: 1.6, run: { p in
                p.step("practice", "waiting", part: "practice", waitsOn: ["slot"])
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("practice", .waiting)) }),
            PlayBeat(caption: "You steer mid-run", wait: 1.6, run: { p in
                p.send("Keep the summary under 15 words", request: p.ids.steer, reply: [
                    ["type": "reply", "request_id": p.ids.steer, "text": "Got it. I told the summary step: under 15 words."]])
            }, check: { probe in probe.turn(probe.ids.steer) == nil ? "the steer didn't show" : nil }),
            PlayBeat(caption: "Checking the result", wait: 1.8, run: { p in p.step("read", "checking", part: "emails") },
                     check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("emails", .checking)) }),
            PlayBeat(caption: "A long row collapses with Show more", wait: 1.4, run: { p in
                p.step("read", "done", part: "emails")
                p.part("emails", "Read email list", "done", text: "I read all 12 entries in `Desktop/imp mails.txt`:\n" + longRows,
                       step: "read", done: 2, total: 6)
                p.step("write", "starting", part: "summary")
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("emails", .done), probe.expect("summary", .starting)) }),
            PlayBeat(caption: "The summary row runs", wait: 1.2, run: { p in
                p.step("write", "running", part: "summary")
                p.live("write", "writing Desktop/summary.txt")
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("summary", .running)) }),
            PlayBeat(caption: "A short, confirmed answer stays open and unmarked", wait: 1.8, run: { p in
                p.step("write", "checking", part: "summary")
                p.step("write", "done", part: "summary")
                p.part("summary", "Write summary", "done", text: "Saved the one-line summary to `Desktop/summary.txt`.",
                       step: "write", done: 3, total: 6)
            }, check: { probe in
                guard let row = probe.card?.row("summary") else { return "no summary row" }
                if row.note != nil || row.text?.contains("couldn't confirm") == true { return "a confirmed row is marked" }
                return PlayProbe.all(probe.cardAgrees(), probe.expect("summary", .done))
            }),
            PlayBeat(caption: "The step it waited on can't run, so the row won't run", wait: 1.6, run: { p in
                p.step("practice", "wont_run", part: "practice", waitsOn: ["slot"])
                p.part("practice", "Typing practice slot", "failed",
                       text: "Couldn't do: the calendar step it needed couldn't be done.", step: "practice", done: 3, total: 6)
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("practice", .wontRun)) }),
            PlayBeat(caption: "A row trying again", wait: 1.6, run: { p in
                p.step("reviews", "retrying", part: "reviews", attempt: 2)
                p.live("reviews", "reading keyboards.example")
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("reviews", .retrying)) }),
            PlayBeat(caption: "A check-in: only its buttons", wait: 1.6, run: { p in
                p.ask(["approval_id": p.ids.approval("checkin"), "kind": "owner_question", "part": "reviews",
                       "source_task_id": p.ids.step("reviews"), "capability": "owner.ask", "options_only": true,
                       "reason": "I've tried 5 times to find two reviews. Keep going?", "summary": "Keep going?",
                       "options": ["Yes, keep going", "No, stop"]])
            }, check: { probe in probe.model.pendingApprovals.contains { $0.id == probe.ids.approval("checkin") }
                ? nil : "the check-in didn't show" }),
            PlayBeat(caption: "You pick Yes", wait: 2.2, run: { p in await p.choose(p.ids.approval("checkin"), option: 0) },
                     check: { probe in probe.model.pendingApprovals.isEmpty ? nil : "the check-in is still open" }),
            PlayBeat(caption: "The task asks in its row and comes forward", wait: 1.6, run: { p in
                p.host?.closeNotch()
                p.ask(["approval_id": p.ids.approval("layout"), "kind": "owner_question", "part": "reviews", "from_task": true,
                       "source_task_id": p.ids.step("reviews"), "capability": "owner.ask",
                       "reason": "Which layout should the reviews cover?", "summary": "Which layout?",
                       "options": ["Full size", "75%"]])
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("reviews", .needsYou)) }),
            PlayBeat(caption: "You answer in the row's box", wait: 2.4, run: { p in
                await p.answer(p.ids.approval("layout"), "The 75% one, with a knob")
                p.step("reviews", "running", part: "reviews", attempt: 2)
            }),
            PlayBeat(caption: "A really unconfirmed part gets its Couldn't confirm line; links in the row", wait: 1.8, run: { p in
                p.step("reviews", "done", part: "reviews", attempt: 2)
                p.part("reviews", "Keyboard reviews", "done",
                       text: "Two reviews of the Arc 75: [reviews.example](https://reviews.example/arc-75) likes the knob; "
                           + "[keyboards.example](https://keyboards.example/arc) says the battery lasts a month.",
                       note: "the battery life (the review gives no test)", step: "reviews", done: 4, total: 6)
                p.step("order", "running", part: "order")
                p.live("order", "filling the cart on shop.example")
            }, check: { probe in
                probe.card?.row("reviews")?.note == nil ? "the unconfirmed part has no Couldn't confirm line"
                    : PlayProbe.all(probe.cardAgrees(), probe.expect("reviews", .done), probe.expect("order", .running))
            }),
            PlayBeat(caption: "FERB's safety check: Yes / No with its box", wait: 1.6, run: { p in
                p.step("order", "needs_you", part: "order")
                p.ask(["approval_id": p.ids.approval("pay"), "kind": "confirmation", "part": "order",
                       "source_task_id": p.ids.step("order"), "capability": "ferb.confirm", "corrects": true,
                       "reason": "Pay $89.00 to shop.example for the Arc 75 with your sample card?", "summary": "Pay $89.00?",
                       "options": ["Yes", "No"]])
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("order", .needsYou)) }),
            PlayBeat(caption: "\"Yes but the cheaper one\" is a correction, not a Yes", wait: 2.4, run: { p in
                await p.correct(p.ids.approval("pay"), "Yes but the cheaper one")
                p.step("order", "running", part: "order")
                p.live("order", "switching to the $69 Arc 75 Lite")
            }, check: { probe in PlayProbe.all(probe.cardAgrees(), probe.expect("order", .running)) }),
            PlayBeat(caption: "The notch is closed when the answer lands", wait: 1.4, run: { p in
                p.step("order", "done", part: "order")
                p.part("order", "Order the keyboard", "done", text: "Ordered the Arc 75 Lite for $69.00 on shop.example.",
                       step: "order", done: 5, total: 6)
                p.host?.closeNotch()
            }),
            PlayBeat(caption: "It opens itself and scrolls to the answer: index, file card, sites chip with a boxed +2",
                     wait: 1.6, run: { p in
                var card: [String: Any] = ["request_id": p.ids.request, "rows": [[String: Any]](), "done": 5, "total": 6,
                                           "line": "5 of 6 done", "lines": [String]()]
                card["rows"] = p.model.conversation.turns.first { $0.requestID == p.ids.request }?.card?.rows.map { row in
                    var value: [String: Any] = ["part": row.part, "label": row.label, "state": row.state, "direct": row.direct,
                                                "seq": row.seq ?? ""]
                    if let text = row.text { value["text"] = text }
                    if let note = row.note { value["note"] = note }
                    if let step = row.step { value["step"] = step }
                    return value
                } ?? []
                p.finish(["type": "outcome", "status": "done_unverified", "verified": false,
                          "answer": "Read email list: I read all 12 entries.\nWrite summary: Saved the one-line summary.\n"
                              + "Tokyo date: Saturday, 7 November.\nKeyboard reviews: Two reviews of the Arc 75 "
                              + "(not all of it confirmed)\nOrder the keyboard: Ordered the Arc 75 Lite for $69.00.\n"
                              + "Typing practice slot: Couldn't do: the calendar step it needed couldn't be done.\n5 of 6 done",
                          "card": card,
                          "cards": [["type": "file", "path": p.workspace.artifact.path, "name": "summary.txt"],
                                    ["type": "link", "host": "reviews.example"], ["type": "link", "host": "keyboards.example"],
                                    ["type": "link", "host": "shop.example"]]])
            }, check: { probe in
                guard let card = probe.card else { return "no card" }
                if card.line == nil { return "no footer" }
                if card.total != card.shownRows.count { return "the footer's count doesn't match the rows" }
                let sites = VisitedSites(hosts: CardLayout.links(probe.turn?.cards ?? []).compactMap(\.host))
                if HoverLook.overflow(sites.remainingCount) != "+2" { return "the sites chip isn't \"+2\"" }
                if !probe.model.expanded && !probe.model.chatInWindow { return "the notch didn't open itself" }
                return nil
            }),
            PlayBeat(caption: "A long chat answer shows half the chat with Show all", wait: 3.0, run: { p in
                let text = (1...30).map { "Line \($0): an invented detail from the email list, kept for the full answer." }
                    .joined(separator: "\n\n")
                p.send("What did the emails say, in full?", request: p.ids.long, reply: [
                    ["type": "reply", "request_id": p.ids.long, "text": text]])
            }, check: { probe in probe.turn(probe.ids.long)?.answer.isEmpty == false ? nil : "the long answer didn't arrive" }),
            PlayBeat(caption: "Files dropped on FERB wait in the composer", wait: 2.4, run: { p in
                _ = p.model.dropFiles(p.scratchFiles())
            }, check: { probe in probe.model.session.attachments.count == 2 ? nil : "the dropped files aren't in the composer" }),
            PlayBeat(caption: "…until you send them", wait: 2.2, run: { p in
                p.sendComposer("What's in these two?", request: p.ids.drop, reply: [
                    ["type": "reply", "request_id": p.ids.drop,
                     "text": "Your trip notes say the train leaves at 9:10; the packing list has a charger and a rain jacket."]])
            }, check: { probe in probe.model.session.attachments.isEmpty ? nil : "the files didn't go with the message" }),
            PlayBeat(caption: "Timers and schedules have short names; a timer says Timer", wait: 1.8, run: { p in
                p.send("Set a 10 minute tea timer and a weekday morning brief", request: p.ids.timer, reply: [
                    ["type": "reply", "request_id": p.ids.timer, "text": "Done: a 10-minute timer and a weekday morning brief.",
                     "cards": [["type": "event", "kind": "timer", "id": "play-tea", "what": "Tea",
                                "when": PlaygroundSamples.time(600)],
                               ["type": "event", "kind": "schedule", "id": "play-brief", "what": "Morning brief",
                                "when": PlaygroundSamples.next(hour: 9, weekdaysOnly: true), "repeat": "every weekday at 9 AM"]]]])
            }, check: { probe in
                (probe.turn(probe.ids.timer)?.cards ?? []).contains { $0.isTimer } ? nil : "no timer card"
            }),
            PlayBeat(caption: "The timer arrives compact on the closed notch", wait: 2.4, run: { p in
                p.host?.closeNotch()
                p.workspace.onNotification?(["id": "reminder:play-\(p.ids.run)-tea", "category": "reminder", "title": "Tea",
                                             "body": "Your tea is ready.", "when_hidden": false,
                                             "target": ["kind": "reminder", "timer_id": "play-\(p.ids.run)-tea"],
                                             "card": ["type": "reminder", "title": "Tea", "kind": "timer",
                                                      "timer_id": "play-\(p.ids.run)-tea"]])
            }, check: { probe in
                probe.model.conversation.notices.contains { $0.id.hasSuffix("play-\(probe.ids.run)-tea") }
                    ? nil : "the timer didn't arrive"
            }),
            PlayBeat(caption: "Settings: record a shortcut; double-tap right Option can be switched off", wait: 3.6, run: { p in
                p.host?.showSettings(.notch)
            }),
            PlayBeat(caption: "Settings → Browsers: each profile with its account", wait: 3.6, run: { p in
                p.host?.showSettings(.browsers)
            }),
            PlayBeat(caption: "Window mode: the same chat in the sticky window", wait: 3.6, run: { p in
                p.host?.closeSettings()
                p.host?.setChatHost(.window)
                p.host?.openNotch()
            }),
            PlayBeat(caption: "Keep on top off, then on", wait: 2.4, run: { p in
                p.host?.setKeepOnTop(false)
                await p.workspace.state.pause(1.6)
                p.host?.setKeepOnTop(true)
            }),
            PlayBeat(caption: "Hide the pill: the menu bar icon carries its colour", wait: 2.4, run: { p in
                p.host?.setHidePill(true)
                await p.workspace.state.pause(2.4)
                p.host?.setHidePill(false)
            }),
            PlayBeat(caption: "Back where the play started", wait: 2.4, run: { p in
                p.host?.setChatHost(p.mode)
                p.host?.openNotch()
            }),
        ]
    }
}
#endif
