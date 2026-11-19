import FerbAppModel
import FerbDesign
import FerbMobile
import XCTest

/// Every conversation of the engine's chat fixtures (the Mac's own scenarios: gaps, sealed IDs, legacy rows, deleted
/// turns, another device's turn, questions answered and left open), synced by the real phone library from a fixture
/// Mac and turned into the app's rows: nothing is lost on the way to the screen (Android's ChatModelTest).
final class ChatModelTests: XCTestCase {
    private func text(_ rows: [ChatRow]) -> String {
        rows.map(ChatModel.words).joined(separator: "\n")
    }

    // decision: PHN-473, PHN-416, PHN-401
    func testEveryFixtureConversationReachesTheScreenWhole() throws {
        let all = try Fixtures.conversations()
        var turns = 0, answers = 0, asks = 0
        for c in all {
            let view = try FixtureMac.transcript(FixtureMac(c))
            let rows = ChatModel.rows(view)
            let keys = rows.map(\.key)
            XCTAssertEqual(keys.count, Set(keys).count, "\(c.scenario): row keys are unique")
            let drawn = text(rows)
            for item in view.items {
                guard case .turn(let t) = item else { continue }
                turns += 1
                if t.from == "owner", let w = t.words?.text, !w.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    XCTAssertTrue(drawn.contains(w), "\(c.scenario): the owner's words '\(w)' are drawn")
                }
                if let a = t.answer, !t.answeredBelow, t.card == nil, !(a.displayText ?? a.text).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    answers += 1
                    // drawn as FERB.app draws it (laid out by the library: kb PHN-723)
                    XCTAssertTrue(rows.contains { if case .answer(let k, _) = $0 { return k == "\(t.entry):answer" } else { return false } },
                                  "\(c.scenario): the answer of \(t.entry) is drawn")
                }
            }
            let open = Questions.open(view.questions)
            asks += open.count
            for q in open {
                XCTAssertTrue(rows.contains { if case .asking(_, let id, _, _, _) = $0 { return id == q.approvalId } else { return false } },
                              "\(c.scenario): open question \(q.approvalId) is in the chat")
            }
            // the Tasks tab reads the same transcript without failing
            _ = WorkModel.of(view, asking: "")
        }
        XCTAssertTrue(turns > 100 && answers > 50 && asks > 0, "the fixtures hold turns (\(turns)), answers (\(answers)) and open questions (\(asks))")
    }

    // decision: ORCH-651, PHN-473
    func testADeliveredWithGapsAnswerKeepsItsNotDoneLines() throws {
        let c = try XCTUnwrap(try Fixtures.conversations().first { $0.scenario == "gaps_single_task" })
        let rows = ChatModel.rows(try FixtureMac.transcript(FixtureMac(c)))
        XCTAssertTrue(rows.contains { if case .answer = $0 { return ChatModel.words($0).contains("Not done:") } else { return false } })
    }

    /// Owner 2026-11-19, "do all": code's "Not done:" lines leave the answer's words and are drawn small under them, the
    /// label amber (FERB.app's annotations); only the block the engine writes is recognised (kb APP-977, APP-978).
    // decision: APP-977, APP-978
    func testNotDoneLinesAreCodesLinesUnderTheWordsNeverInThem() throws {
        let c = try XCTUnwrap(try Fixtures.conversations().first { $0.scenario == "gaps_single_task" })
        let rows = ChatModel.rows(try FixtureMac.transcript(FixtureMac(c)))
        let ui = try XCTUnwrap(rows.compactMap { if case .answer(_, let ui) = $0 { return ui } else { return nil } }.first { !$0.gaps.isEmpty })
        XCTAssertFalse(ui.copy.contains("Not done"), "not in the words: \(ui.copy)")
        XCTAssertEqual(GapLines("Left it.\n\nNot done: the form.").body, "Left it.")
        XCTAssertEqual(GapLines("The page is open.\n\nNot done: one.\nNot done: two.").reasons, ["one.", "two."])
        XCTAssertEqual(GapLines("Not done: The form wasn't sent.").reasons, ["The form wasn't sent."])
        for words in ["Not done: yet.\n\nMore soon.", "Two.\n\nNot done: the rubric.\nThe rest is done.",
                      "Two.\n\nThe form is Not done: it needs your name.", "Two.\n\nNot done:"] {
            XCTAssertEqual(GapLines(words).body, words, words)
            XCTAssertTrue(GapLines(words).reasons.isEmpty, words)
        }
    }

    // decision: ENG-12
    func testLegacyRowsAndATurnFromAnotherDeviceAreDrawnLikeAnyTurn() throws {
        let all = try Fixtures.conversations()
        for name in ["legacy_then_new", "turn_from_another_client"] {
            let c = try XCTUnwrap(all.first { $0.scenario == name })
            let rows = ChatModel.rows(try FixtureMac.transcript(FixtureMac(c)))
            XCTAssertTrue(rows.contains { if case .owner = $0 { return true } else { return false } }, "\(name) has owner rows")
            XCTAssertTrue(rows.contains { switch $0 { case .answer, .rich, .ferb: return true; default: return false } }, "\(name) has FERB's rows")
        }
    }

    func testMarksFollowTheMacsStatesAndNeverColourAlone() {
        XCTAssertEqual(ChatModel.markOf("done_with_gaps"), .done)
        XCTAssertEqual(ChatModel.markOf("needsYou"), .asking)
        XCTAssertEqual(ChatModel.markOf("wontRun"), .failed)
        XCTAssertEqual(ChatModel.markOf("waiting"), .waiting)
        XCTAssertEqual(ChatModel.markOf("retrying"), .running)
    }

    func testMarkdownMarksThatOnlyStyleAreDropped() {
        XCTAssertEqual(ChatModel.plain("**Done:** [the report](https://x.example)"), "Done: the report")
        XCTAssertEqual(ChatModel.plain("## Title\n- one"), "Title\n• one")
    }
}
