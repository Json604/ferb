#if canImport(UIKit)
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import XCTest
@testable import FerbAppUI

/// FERB's review of what it learned (kb MEM-460..457, MEM-485, MEM-486) on the phone: the Mac's list laid out part by
/// part, every line shown with its tick; Approve all sends the ticked lines and the unticked ones in one call and the
/// review that comes back replaces the list; a failure leaves it as it was; the day's note opens the list in a sheet;
/// opening the app tells the Mac. Goldens: Screenshots/app/parity/question-review.png (the card),
/// review-note.png (the day's note), review-sheet.png (its sheet, a line unticked).
@MainActor
final class ImportReviewTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func b(_ answer: String, _ label: String) -> ReviewButton { ReviewButton(answer: answer, label: label) }
    private func i(_ id: String, _ text: String, _ note: String = "") -> ReviewItem { ReviewItem(id: id, text: text, note: note, answers: []) }

    /// The owner's example (invented data): the three parts as the Mac words them, with the list's own buttons.
    private func review(workExtra: [ReviewItem] = []) -> MemoryReview {
        MemoryReview(parts: [
            ReviewPart(part: "always_on", title: "Keep these in mind all the time?",
                       hint: "I'll remember all of them either way. The ones you approve, I'll keep in mind in everything I do.",
                       groups: [
                           ReviewGroup(title: "How you like me to work", items: [
                               i("a1", "The owner prefers short, concise text in chat."),
                               i("a2", "The owner wants to talk things through before coding starts."),
                               i("a3", "The owner likes replies in bullet points."),
                           ] + workExtra, all: []),
                           ReviewGroup(title: "Your setup", items: [
                               i("a4", "The owner tests on a Motorola Edge 60 Fusion over wireless debugging."),
                           ], all: []),
                       ],
                       answers: [b("approve", "Approve"), b("decline", "Decline")],
                       all: [b("approve", "Approve all"), b("decline", "Decline all")]),
            ReviewPart(part: "still_true", title: "Are these still true?", hint: "You told me these a while ago.",
                       groups: [ReviewGroup(title: "", items: [i("s1", "The owner is in Goa this week.", "I said I'd check this on 25 Nov 2026")], all: [])],
                       answers: [b("yes", "Yes"), b("no", "No, not any more"), b("unsure", "Not sure")],
                       all: [b("yes", "Yes to all"), b("no", "No to all")]),
            ReviewPart(part: "offers", title: "Still want help with these?", hint: "I won't start any of them on my own.",
                       groups: [ReviewGroup(title: "", items: [i("o1", "Book the dentist for March")], all: [])],
                       answers: [b("open", "Yes"), b("done", "Done already"), b("not_interested", "No thanks")],
                       all: []),
        ], count: 6 + UInt64(workExtra.count), all: [b("approve", "Approve all"), b("decline", "Decline all")],
           some: "Approve {n}", hint: "Untick anything that isn't right.")
    }

    private let body = #"{"approval_id":"q-review","capability":"memory.review","kind":"owner_question","reason":"Can you check a few things I learned about you?\n4 things to keep in mind all the time, 1 thing I said I'd check with you and 1 thing I could help with. Untick anything that isn't right, or just tell me.","options":[],"summary":"Can you check a few things I learned about you?","detail":"4 things to keep in mind all the time, 1 thing I said I'd check with you and 1 thing I could help with. Untick anything that isn't right, or just tell me."}"#

    private func question() -> Question {
        Questions.parse(QuestionItem(entry: "e-q-review", approvalId: "q-review", requestId: "r-1", taskId: nil, state: "open", open: true,
                                     bodyJson: body, shown: "end_of_chat", row: nil))
    }

    /// The parts, their lines, the groups (an empty title is none), every line and its note, in the Mac's order and
    /// words; nothing folded however many (Approve all answers only what he can see).
    // decision: MEM-460, MEM-488
    func testTheListLaysOutTheMacsPartsGroupsAndEveryLine() {
        let parts = ReviewLayout.parts(review(workExtra: [i("a5", "The owner likes dark mode."), i("a6", "The owner reads on Kindle.")]))
        XCTAssertEqual(parts.map(\.id), ["always_on", "still_true", "offers"])
        XCTAssertEqual(parts.map(\.title), ["Keep these in mind all the time?", "Are these still true?", "Still want help with these?"])
        XCTAssertEqual(parts[1].hint, "You told me these a while ago.")
        XCTAssertEqual(parts[0].groups.map(\.title), ["How you like me to work", "Your setup"])
        XCTAssertEqual(parts[1].groups.map(\.title), [nil])
        XCTAssertEqual(parts[0].groups[0].items.map(\.id), ["a1", "a2", "a3", "a5", "a6"])
        XCTAssertEqual(parts[1].groups[0].items[0].note, "I said I'd check this on 25 Nov 2026")
        XCTAssertEqual(parts[0].groups[0].items[0].note, nil)
        XCTAssertEqual(parts[0].ids, ["a1", "a2", "a3", "a5", "a6", "a4"])
    }

    /// The Mac's question with the review's capability is drawn as the review (the library's words say so).
    // decision: MEM-460
    func testTheReviewsQuestionIsDrawnAsTheReview() {
        let q = question()
        XCTAssertEqual(q.shown?.review, true)
        XCTAssertEqual(q.shown?.question, "Can you check a few things I learned about you?")
        XCTAssertTrue(q.choices.isEmpty, "his typed answer still works: the box shows")
        let other = Questions.parse(questionItem("q-2"))
        XCTAssertEqual(other.shown?.review, false)
    }

    /// Every line ticked until he unticks it; Approve all sends the ticked and the unticked in one call and names how
    /// many it approves while some are unticked; Decline all sends every line; the reply replaces the list; a failure
    /// leaves it as it was and says why.
    // decision: MEM-488
    func testApproveAllSendsTheTickedAndTheUntickedInOneCall() async {
        rig.phone.review = review()
        let m = rig.model()
        rig.session.unlocked()
        await m.loadMemoryReview().value
        XCTAssertTrue(rig.phone.calls.contains("review"))
        XCTAssertTrue(m.review.ids.allSatisfy(m.review.isTicked))
        XCTAssertEqual(m.review.approveLabel, "Approve all")
        XCTAssertEqual(m.review.declineLabel, "Decline all")
        XCTAssertEqual(m.review.hint, "Untick anything that isn't right.")
        m.review.toggle("a3")
        m.review.toggle("s1")
        XCTAssertFalse(m.review.isTicked("a3"))
        XCTAssertEqual(m.review.approveLabel, "Approve 4")

        let before = m.review.review
        rig.phone.reviewRefusal = .Offline(detail: "The Mac can't be reached.", linkCode: nil)
        await m.answerMemoryReviewAll(approve: true).value
        XCTAssertEqual(m.review.review, before, "a failure leaves the list as it was")
        XCTAssertEqual(m.ui.note, Note("err_offline"))

        await m.answerMemoryReviewAll(approve: true).value
        XCTAssertEqual(rig.phone.reviewAll.last?.approve, ["a1", "a2", "a4", "o1"])
        XCTAssertEqual(rig.phone.reviewAll.last?.decline, ["a3", "s1"])
        XCTAssertEqual(m.review.parts, [])
        XCTAssertEqual(m.review.unticked, [], "nothing listed, nothing unticked")

        rig.phone.review = review()
        await m.loadMemoryReview().value
        m.review.toggle("a1")
        await m.answerMemoryReviewAll(approve: false).value
        XCTAssertEqual(rig.phone.reviewAll.last?.approve, [])
        XCTAssertEqual(rig.phone.reviewAll.last?.decline, ["a1", "a2", "a3", "a4", "s1", "o1"])

        // nothing ticked: no Approve, only Decline all
        rig.phone.review = review()
        await m.loadMemoryReview().value
        for id in m.review.ids { m.review.toggle(id) }
        XCTAssertNil(m.review.approveLabel)
    }

    /// The day's note's Review opens the list in a sheet (the phone has no Memory page), read from the Mac.
    // decision: MEM-485
    func testTheNotesReviewOpensTheListInASheet() async {
        rig.phone.review = review()
        let m = rig.model()
        rig.session.unlocked()
        m.openReview()
        XCTAssertEqual(m.sheet, .review)
        await m.loadMemoryReview().value
        XCTAssertEqual(m.review.parts.count, 3)
    }

    /// Opening the app (in front, unlocked, the Mac reachable) tells the Mac each time; the Mac keeps it to once a day.
    // decision: MEM-485
    func testOpeningTheAppTellsTheMac() async throws {
        _ = rig.model()
        rig.session.foreground(true)
        rig.session.unlocked()
        for _ in 0..<50 where !rig.phone.calls.contains("review-opened") { try await Task.sleep(nanoseconds: 20_000_000) }
        XCTAssertEqual(rig.phone.calls.filter { $0 == "review-opened" }.count, 1)
    }

    /// The chat with the review's card: the owner's message, then FERB's question with the list and the typed answer's
    /// box. Copied to `FERB_SHOTS_DIR` when it is set, to look at.
    // decision: MEM-460, MEM-488
    func testGoldenTheChatWithTheReviewsCard() async throws {
        rig.phone.review = review()
        let m = rig.model()
        rig.session.unlocked()
        await m.loadMemoryReview().value
        let q = question()
        try golden(m, rows: [.owner(key: "m1", text: "Bring in what ChatGPT knows about me", files: [], note: nil),
                             .asking(key: "q", approvalId: q.approvalId, title: q.title, reason: nil, hidden: false)],
                   questions: [q], "question-review", shot: "iphone-review-chat", height: 1300)
    }

    /// The day's note (more than five waiting) in the chat, its Review under its words.
    // decision: MEM-485
    func testGoldenTheDaysNote() async throws {
        let m = rig.model()
        rig.session.unlocked()
        let ui = NoticeUi(boxSays: false, above: [], notes: [], pages: ["Review"], review: "Review")
        try golden(m, rows: [.owner(key: "m1", text: "Good morning", files: [], note: nil),
                             .notice(key: "n", text: "I learned 8 things about you. Review them when you like.", quotes: [], ui: ui)],
                   questions: [], "review-note", shot: "iphone-review-note", height: 500)
    }

    /// The sheet the note's Review opens, one line unticked.
    // decision: MEM-485, MEM-488
    func testGoldenTheReviewSheet() async throws {
        rig.phone.review = review()
        let m = rig.model()
        rig.session.unlocked()
        await m.loadMemoryReview().value
        m.review.toggle("s1")
        let page = ReviewSheet(model: m, insets: PhoneInsets(top: 0, bottom: 20, left: 0, right: 0), close: {}, scrolls: false)
            .frame(width: 402, height: 874)
            .stillMotion()
        AppSnapshot.assert(page, size: CGSize(width: 402, height: 874), path: "app/parity/review-sheet.png")
        if let dir = ProcessInfo.processInfo.environment["FERB_SHOTS_DIR"] {
            let image = AppSnapshot.render(page, size: CGSize(width: 402, height: 874)).cgImage!
            try AppSnapshot.png(image).write(to: URL(fileURLWithPath: dir).appendingPathComponent("iphone-review-sheet.png"))
        }
    }

    private func golden(_ m: AppModel, rows: [ChatRow], questions: [Question], _ name: String, shot: String, height: CGFloat) throws {
        let page = PhoneScreen(moment: .lab(22.5), glide: false) {
            // the chat's own rows (kb PHN-770)
            VStack(spacing: 12) {
                ForEach(rows) { ChatRowView(model: m, row: $0, today: 0, olderLoading: false) }
            }
            .environment(\.questions, questions)
            .padding(.horizontal, 16).padding(.top, 24).frame(maxHeight: .infinity, alignment: .top)
        }
        .frame(width: 402, height: height)
        .stillMotion()
        AppSnapshot.assert(page, size: CGSize(width: 402, height: height), path: "app/parity/\(name).png")
        if let dir = ProcessInfo.processInfo.environment["FERB_SHOTS_DIR"] {
            let image = AppSnapshot.render(page, size: CGSize(width: 402, height: height)).cgImage!
            try AppSnapshot.png(image).write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(shot).png"))
        }
    }
}
#endif
