import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Past-bug guard (TEST-503), owner 2026-11-14 on the Rust build: "why are these unselectable??? but when i close and
/// open chat again, they are visible then". A plan opened three questions at once (which comparison, which profile,
/// the date) and a safety check came beside them. The app had one flag for "an answer is on its way": while any one
/// answer's call was out, every open question's buttons and Send were disabled (drawn dimmed) and a typed answer's
/// Return was dropped without a word, until that one call came back. Each question now waits only on its own answer.
@MainActor
final class QuestionAnswerInFlightTests: XCTestCase {
    // decision: APP-581, APP-910
    func testAnswerOnItsWayLeavesTheOtherQuestionsAnswerable() async throws {
        let rpc = HeldAnswersRPC()
        rpc.held = ["ap-comparison"]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("AnswerInFlight"))
        for (id, question, options) in [("ap-comparison", "Which comparison should I put in the Google Sheet?", ["Laptops", "Framework"]),
                                        ("ap-profile", "Which browser profile should I use?", ["Work", "Personal"]),
                                        ("ap-date", "Do you want the date added at the top?", ["No", "Yes"])] {
            model.receiveBusEvent(["topic": "approval.requested", "data": [
                "approval_id": id, "kind": "task", "capability": "plan.ask", "reason": question,
                "task_id": "task-1.worker", "request_id": "req-1", "options": options
            ] as [String: Any]])
        }
        XCTAssertEqual(model.pendingApprovals.map(\.id), ["ap-comparison", "ap-profile", "ap-date"])

        // The first answer's call stays out (the daemon hasn't answered it yet).
        let first = try XCTUnwrap(model.pendingApprovals.first { $0.id == "ap-comparison" })
        model.chooseApproval(first, index: 0)
        await settle { rpc.reached("ap-comparison") }
        XCTAssertTrue(rpc.reached("ap-comparison"))
        XCTAssertTrue(model.isSending(first), "the question being answered waits for its answer")

        // The other questions are not waiting on it: their buttons work and a typed answer goes.
        let profile = try XCTUnwrap(model.pendingApprovals.first { $0.id == "ap-profile" })
        let date = try XCTUnwrap(model.pendingApprovals.first { $0.id == "ap-date" })
        XCTAssertFalse(model.isSending(profile), "another question's buttons stay enabled")
        XCTAssertFalse(model.isSending(date))
        model.chooseApproval(profile, index: 1)
        await settle { rpc.reached("ap-profile") }
        XCTAssertTrue(rpc.reached("ap-profile"), "another question's choice is sent while the first answer is out")
        model.answerApproval(date, text: "Yes, add today's date")
        await settle { rpc.reached("ap-date") }
        XCTAssertTrue(rpc.reached("ap-date"), "a typed answer to another question is sent, never dropped")
        await settle { !model.pendingApprovals.contains { $0.id == "ap-profile" || $0.id == "ap-date" } }
        XCTAssertEqual(model.pendingApprovals.map(\.id), ["ap-comparison"], "both were answered while the first was out")
        XCTAssertEqual(rpc.calls.filter { $0.0 == "chat.approve" }.compactMap { $0.1["approval_id"] as? String },
                       ["ap-comparison", "ap-profile"])
        XCTAssertEqual(rpc.calls.first { $0.0 == "chat.answer" }?.1["approval_id"] as? String, "ap-date")

        // The same question can't be answered twice while its own answer is out.
        model.chooseApproval(first, index: 1)
        XCTAssertEqual(rpc.calls.filter { $0.0 == "chat.approve" && $0.1["approval_id"] as? String == "ap-comparison" }.count, 1)

        rpc.release("ap-comparison")
        await settle { model.pendingApprovals.isEmpty && model.sendingAnswers.isEmpty }
        XCTAssertTrue(model.pendingApprovals.isEmpty)
        XCTAssertTrue(model.sendingAnswers.isEmpty)
    }

    private func settle(until done: () -> Bool) async {
        for _ in 0..<2000 where !done() {
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
    }
}

/// Answers every call at once except an answer to a question in `held`, which stays out until `release`.
private final class HeldAnswersRPC: DaemonRPCTransport, @unchecked Sendable {
    private let lock = NSLock()
    private let recorded = RecordedCalls()
    private var waiting: [String: CheckedContinuation<Void, Never>] = [:]
    private var seen: Set<String> = []
    var held: Set<String> = []
    var calls: [(String, [String: Any])] { recorded.all }

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        let id = params["approval_id"] as? String ?? ""
        if ["chat.approve", "chat.answer"].contains(method) {
            lock.lock()
            seen.insert(id)
            let hold = held.contains(id)
            lock.unlock()
            if hold {
                await withCheckedContinuation { continuation in
                    lock.lock()
                    if held.contains(id) { waiting[id] = continuation; lock.unlock() }
                    else { lock.unlock(); continuation.resume() }        // released before it began to wait
                }
            }
            return ["approval_id": id, "state": "granted"]
        }
        if method == "chat.status" { return ["running": [], "approvals": [], "results": []] }
        return ["ok": true]
    }

    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { $0.finish() }
    }

    /// Whether the answer to `id` reached the daemon.
    func reached(_ id: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return seen.contains(id)
    }

    func release(_ id: String) {
        lock.lock()
        let continuation = waiting.removeValue(forKey: id)
        held.remove(id)
        lock.unlock()
        continuation?.resume()
    }
}
