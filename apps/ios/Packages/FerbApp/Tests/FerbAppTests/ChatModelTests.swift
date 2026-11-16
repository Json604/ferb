import FerbAppModel
import FerbDesign
import FerbMobile
import XCTest

/// Every conversation of the engine's chat fixtures (the Mac's own scenarios: gaps, sealed IDs, legacy rows, deleted
/// turns, another device's turn, questions answered and left open), synced by the real phone library from a fixture
/// Mac and turned into the app's rows: nothing is lost on the way to the screen (Android's ChatModelTest).
final class ChatModelTests: XCTestCase {
    private func text(_ rows: [ChatRow]) -> String {
        rows.map { r -> String in
            switch r {
            case .owner(_, let t, _, _), .ferb(_, let t), .line(_, let t, _): return t
            case .task(let t): return t.headline + t.parts.map(\.title).joined(separator: ", ")
            default: return ""
            }
        }.joined(separator: "\n")
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
                if let a = t.answer, !t.answeredBelow {
                    answers += 1
                    let words = ChatModel.plain(a.displayText ?? a.text)
                    XCTAssertTrue(words.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || drawn.contains(words),
                                  "\(c.scenario): the answer '\(words.prefix(40))' is drawn")
                }
            }
            let open = Questions.open(view.questions)
            asks += open.count
            for q in open {
                XCTAssertTrue(rows.contains { if case .asking(_, let id, _) = $0 { return id == q.approvalId } else { return false } },
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
        XCTAssertTrue(rows.contains { if case .ferb(_, let t) = $0 { return t.contains("Not done:") } else { return false } })
    }

    // decision: ENG-12
    func testLegacyRowsAndATurnFromAnotherDeviceAreDrawnLikeAnyTurn() throws {
        let all = try Fixtures.conversations()
        for name in ["legacy_then_new", "turn_from_another_client"] {
            let c = try XCTUnwrap(all.first { $0.scenario == name })
            let rows = ChatModel.rows(try FixtureMac.transcript(FixtureMac(c)))
            XCTAssertTrue(rows.contains { if case .owner = $0 { return true } else { return false } }, "\(name) has owner rows")
            XCTAssertTrue(rows.contains { if case .ferb = $0 { return true } else { return false } }, "\(name) has FERB's rows")
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
        XCTAssertEqual(ChatModel.plain("**Done:** [the report](https://x.example)"), "Done: the report (https://x.example)")
        XCTAssertEqual(ChatModel.plain("## Title\n- one"), "Title\n• one")
    }
}
