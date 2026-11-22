#if canImport(UIKit)
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import XCTest
@testable import FerbAppUI

/// FERB's review of what an import brought in (kb MEM-460..457) on the phone: its question's card is the Mac's list,
/// laid out part by part, answered item by item or a part at once; the answer's review replaces the list; a failure
/// leaves it as it was; and a golden of the chat with the card (Screenshots/app/parity/question-review.png).
@MainActor
final class ImportReviewTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func b(_ answer: String, _ label: String) -> ReviewButton { ReviewButton(answer: answer, label: label) }
    private func i(_ id: String, _ text: String, _ note: String = "") -> ReviewItem { ReviewItem(id: id, text: text, note: note, answers: []) }
    /// One already always on: its own Keep / Remove (MEM-464).
    private func on(_ id: String, _ text: String) -> ReviewItem {
        ReviewItem(id: id, text: text, note: "Already always on", answers: [b("approve", "Keep"), b("decline", "Remove")])
    }

    /// The owner's example (invented data): the three parts as the Mac words them. `setup` adds a second already-on
    /// item to "Your setup", which then has its own Keep all / Remove all.
    private func review(workExtra: [ReviewItem] = [], setup: Bool = false) -> MemoryReview {
        MemoryReview(parts: [
            ReviewPart(part: "always_on", title: "Keep these in mind all the time?",
                       hint: "I'll remember all of them either way. The ones you approve, I'll keep in mind in everything I do.",
                       groups: [
                           ReviewGroup(title: "How you like me to work", items: [
                               on("a1", "You prefer short, concise text in chat."),
                               i("a2", "You want to talk things through before coding starts."),
                               i("a3", "You like replies in bullet points."),
                           ] + workExtra, all: []),
                           ReviewGroup(title: "Your setup", items: [
                               on("a4", "You test on a Motorola Edge 60 Fusion over wireless debugging."),
                           ] + (setup ? [on("a6", "You use Arc as your main browser.")] : []),
                                       all: setup ? [b("approve", "Keep all"), b("decline", "Remove all")] : []),
                       ],
                       answers: [b("approve", "Approve"), b("decline", "Decline")],
                       all: [b("approve", "Approve all"), b("decline", "Decline all")]),
            ReviewPart(part: "still_true", title: "Are these still true?", hint: "You told me these a while ago.",
                       groups: [ReviewGroup(title: "", items: [i("s1", "You value sound feedback in apps.", "Last said 20 Sep 2026")], all: [])],
                       answers: [b("yes", "Yes"), b("no", "No, not any more"), b("unsure", "Not sure")],
                       all: [b("yes", "Yes to all"), b("no", "No to all")]),
            ReviewPart(part: "offers", title: "Still want help with these?", hint: "I won't start any of them on my own.",
                       groups: [ReviewGroup(title: "", items: [i("o1", "Book the dentist for March")], all: [])],
                       answers: [b("open", "Yes"), b("done", "Done already"), b("not_interested", "No thanks")],
                       all: []),
        ], count: 6 + UInt64(workExtra.count) + (setup ? 1 : 0))
    }

    private let body = #"{"approval_id":"q-review","capability":"memory.review","kind":"owner_question","reason":"Can you check a few things I brought in?\n4 things to keep in mind all the time, 1 older thing that may not be true any more and 1 thing I could help with. Answer each one here, or just tell me.","options":[],"summary":"Can you check a few things I brought in?","detail":"4 things to keep in mind all the time, 1 older thing that may not be true any more and 1 thing I could help with. Answer each one here, or just tell me."}"#

    private func question() -> Question {
        Questions.parse(QuestionItem(entry: "e-q-review", approvalId: "q-review", requestId: "r-1", taskId: nil, state: "open", open: true,
                                     bodyJson: body, shown: "end_of_chat", row: nil))
    }

    /// The parts, their lines, the groups (an empty title is none), the items and their notes, each item's buttons and
    /// the part's, in the Mac's order and words; a group of more than three folds to three with "Show all N".
    // decision: MEM-460
    func testTheCardLaysOutTheMacsPartsGroupsItemsNotesAndButtons() {
        let parts = ReviewLayout.parts(review(), opened: [])
        XCTAssertEqual(parts.map(\.id), ["always_on", "still_true", "offers"])
        XCTAssertEqual(parts.map(\.title), ["Keep these in mind all the time?", "Are these still true?", "Still want help with these?"])
        XCTAssertEqual(parts[1].hint, "You told me these a while ago.")
        XCTAssertEqual(parts[0].groups.map(\.title), ["How you like me to work", "Your setup"])
        XCTAssertEqual(parts[1].groups.map(\.title), [nil])
        XCTAssertEqual(parts[0].groups[0].items.map(\.text), ["You prefer short, concise text in chat.",
                                                              "You want to talk things through before coding starts.",
                                                              "You like replies in bullet points."])
        XCTAssertEqual(parts[0].groups[0].items.map(\.note), ["Already always on", nil, nil])
        XCTAssertEqual(parts[1].groups[0].items[0].note, "Last said 20 Sep 2026")
        XCTAssertNil(parts[0].groups[0].toggle, "three items: nothing folded")
        XCTAssertEqual(parts[1].groups[0].items[0].answers.map(\.label), ["Yes", "No, not any more", "Not sure"])
        XCTAssertEqual(parts[2].groups[0].items[0].answers.map(\.answer), ["open", "done", "not_interested"])
        // an item already always on has its own Keep / Remove; the others the part's Approve / Decline (MEM-464)
        XCTAssertEqual(parts[0].groups[0].items.map { $0.answers.map(\.label) },
                       [["Keep", "Remove"], ["Approve", "Decline"], ["Approve", "Decline"]])
        XCTAssertEqual(parts[0].groups[0].items[0].answers.map(\.answer), ["approve", "decline"])
        XCTAssertEqual(parts[0].groups.map(\.all), [[], []], "no group buttons unless the Mac gives them")
        XCTAssertEqual(parts[0].all.map(\.label), ["Approve all", "Decline all"])
        XCTAssertEqual(parts[1].all, [], "one item: no buttons for all of them")
        XCTAssertEqual(parts[2].all, [])
        XCTAssertEqual(parts[0].ids, ["a1", "a2", "a3", "a4"])

        // a fourth item in the group: three shown and "Show all 4"; opened, all four and "Show less"
        let long = review(workExtra: [i("a5", "You like dark mode.")])
        let folded = ReviewLayout.parts(long, opened: [])[0].groups[0]
        XCTAssertEqual(folded.items.map(\.id), ["a1", "a2", "a3"])
        XCTAssertEqual(folded.total, 4)
        XCTAssertEqual(folded.toggle, "Show all 4")
        let open = ReviewLayout.parts(long, opened: [folded.id])[0].groups[0]
        XCTAssertEqual(open.items.map(\.id), ["a1", "a2", "a3", "a5"])
        XCTAssertEqual(open.toggle, "Show less")
        XCTAssertEqual(ReviewLayout.parts(long, opened: [])[0].ids, ["a1", "a2", "a3", "a5", "a4"], "all answers the folded ones too")
        XCTAssertEqual(folded.ids, ["a1", "a2", "a3", "a5"])

        // "Your setup" with two already on: its own Keep all / Remove all, for its two ids
        let setup = ReviewLayout.parts(review(setup: true), opened: [])[0].groups[1]
        XCTAssertEqual(setup.all.map(\.label), ["Keep all", "Remove all"])
        XCTAssertEqual(setup.ids, ["a4", "a6"])
    }

    /// The Mac's question with the review's capability is drawn as the review (the library's words say so).
    // decision: MEM-460
    func testTheReviewsQuestionIsDrawnAsTheReview() {
        let q = question()
        XCTAssertEqual(q.shown?.review, true)
        XCTAssertEqual(q.shown?.question, "Can you check a few things I brought in?")
        XCTAssertTrue(q.choices.isEmpty, "his typed answer still works: the box shows")
        let other = Questions.parse(questionItem("q-2"))
        XCTAssertEqual(other.shown?.review, false)
    }

    /// An item's button sends its part, its id and the answer; an all button sends every id of its part; the review
    /// that comes back replaces the list; a failure leaves the list as it was and says why.
    // decision: MEM-460, MEM-462
    func testAButtonAnswersItsItemsAndTheReviewThatComesBackReplacesTheList() async {
        rig.phone.review = review()
        let m = rig.model()
        rig.session.unlocked()
        await m.loadMemoryReview().value
        XCTAssertTrue(rig.phone.calls.contains("review"))
        XCTAssertEqual(m.review.parts.map(\.id), ["always_on", "still_true", "offers"])

        await m.answerMemoryReview(part: "always_on", itemIds: ["a2"], answer: "approve").value
        XCTAssertEqual(rig.phone.reviewAnswers.last?.part, "always_on")
        XCTAssertEqual(rig.phone.reviewAnswers.last?.ids, ["a2"])
        XCTAssertEqual(rig.phone.reviewAnswers.last?.answer, "approve")
        XCTAssertEqual(m.review.parts[0].groups[0].items.map(\.id), ["a1", "a3"], "the answered item left the list")

        // "Decline all" under the part: every item of it, the one in the other group too
        let all = m.review.parts[0]
        XCTAssertEqual(all.all.map(\.label), ["Approve all", "Decline all"])
        await m.answerMemoryReview(part: all.id, itemIds: all.ids, answer: all.all[1].answer).value
        XCTAssertEqual(rig.phone.reviewAnswers.last?.part, "always_on")
        XCTAssertEqual(rig.phone.reviewAnswers.last?.ids, ["a1", "a3", "a4"])
        XCTAssertEqual(rig.phone.reviewAnswers.last?.answer, "decline")
        XCTAssertEqual(m.review.parts.map(\.id), ["still_true", "offers"])

        // a group's own "Remove all": that group's ids only
        rig.phone.review = review(setup: true)
        await m.loadMemoryReview().value
        let setup = m.review.parts[0].groups[1]
        await m.answerMemoryReview(part: "always_on", itemIds: setup.ids, answer: setup.all[1].answer).value
        XCTAssertEqual(rig.phone.reviewAnswers.last?.ids, ["a4", "a6"])
        XCTAssertEqual(rig.phone.reviewAnswers.last?.answer, "decline")
        XCTAssertEqual(m.review.parts[0].groups.map(\.title), ["How you like me to work"])
        rig.phone.review = review()
        await m.loadMemoryReview().value
        for p in ["always_on", "still_true"] {
            let ids = m.review.parts.first { $0.id == p }!.ids
            await m.answerMemoryReview(part: p, itemIds: ids, answer: "decline").value
        }
        XCTAssertEqual(m.review.parts.map(\.id), ["offers"])

        let before = m.review.review
        rig.phone.reviewRefusal = .Offline(detail: "The Mac can't be reached.", linkCode: nil)
        await m.answerMemoryReview(part: "offers", itemIds: ["o1"], answer: "done").value
        XCTAssertEqual(m.review.review, before, "a failure leaves the list as it was")
        XCTAssertEqual(m.ui.note, Note("err_offline"))

        // "Show all N" opens a group on the model, "Show less" folds it again
        m.review.toggle("still_true/0")
        XCTAssertEqual(m.review.opened, ["still_true/0"])
        m.review.toggle("still_true/0")
        XCTAssertEqual(m.review.opened, [])
    }

    /// The chat with the review's card: the owner's message, then FERB's question with the list and the typed answer's
    /// box. Copied to `FERB_SHOTS_DIR` when it is set, to look at.
    // decision: MEM-460
    func testGoldenTheChatWithTheReviewsCard() async throws {
        try await golden(review(), "question-review", shot: "iphone-review-chat", height: 1300)
    }

    /// The same with "Your setup" holding two already always on, with its own Keep all / Remove all (MEM-464).
    // decision: MEM-460, MEM-464
    func testGoldenAGroupAlreadyAlwaysOnHasItsOwnButtons() async throws {
        try await golden(review(setup: true), "question-review-setup", shot: "iphone-review-setup", height: 1450)
    }

    private func golden(_ r: MemoryReview, _ name: String, shot: String, height: CGFloat) async throws {
        rig.phone.review = r
        let m = rig.model()
        rig.session.unlocked()
        await m.loadMemoryReview().value
        let q = question()
        let page = PhoneScreen(moment: .lab(22.5), glide: false) {
            // the chat's own rows: his message, then FERB's question (kb PHN-770)
            VStack(spacing: 12) {
                ForEach([ChatRow.owner(key: "m1", text: "Bring in what ChatGPT knows about me", files: [], note: nil),
                         ChatRow.asking(key: "q", approvalId: q.approvalId, title: q.title, reason: nil, hidden: false)]) {
                    ChatRowView(model: m, row: $0, today: 0, olderLoading: false)
                }
            }
            .environment(\.questions, [q])
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
