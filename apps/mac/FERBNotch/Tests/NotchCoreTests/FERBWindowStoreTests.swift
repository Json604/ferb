import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase E: each window button is one RPC; a failure is the window's problem line, never a crash or a silent
/// empty list.
@MainActor
final class FERBWindowStoreTests: XCTestCase {
    private func task(_ id: String, _ status: String = "done", at: String = "2026-10-25T10:00:00+00:00") -> [String: Any] {
        ["task_id": id, "goal": "goal \(id)", "status": status, "created_at": at, "answer": "", "verdict": NSNull()]
    }

    func testSendDiagnosticsPreparesTheFileThenSendsItOnSend() async {
        // Owner 2026-10-27 / 2026-11-01: Settings → Send diagnostics to developer, like a failure report: prepared,
        // viewable, and posted to the report relay only when the owner presses Send
        let rpc = ScriptedRPC()
        let path = "/Users/me/.ferb/reports/FERB-diagnostic-2026-10-27-020000.txt"
        rpc.replies["reports.diagnostic"] = ["path": path, "summary": "FERB 0.27, macOS 15.6",
                                             "subject": "FERB diagnostics (FERB 0.27, macOS 15.6)"]
        rpc.replies["reports.send"] = ["sent": false, "path": path, "error": "The report service didn't answer"]
        let store = FERBWindowStore(rpc: rpc)
        var opened: [URL] = []
        store.open = { opened.append($0) }
        await store.prepareDiagnostic(appVersion: "0.27 (1)")
        XCTAssertEqual(rpc.params(of: "reports.diagnostic").first?["send"] as? Bool, true)
        guard case .ready(let report)? = store.diagnostic else { return XCTFail("not prepared") }
        XCTAssertTrue(rpc.params(of: "reports.send").isEmpty)
        store.open(report.file)
        XCTAssertEqual(opened.map(\.lastPathComponent), ["FERB-diagnostic-2026-10-27-020000.txt"])
        await store.sendDiagnostic(appVersion: "0.27 (1)")
        XCTAssertEqual(rpc.params(of: "reports.send").first?["path"] as? String, path)
        XCTAssertEqual(rpc.params(of: "reports.send").first?["title"] as? String,
                       "FERB diagnostics (FERB 0.27, macOS 15.6)")
        XCTAssertEqual(store.diagnostic, .unsent(report, reason: "The report service didn't answer"))
        rpc.replies["reports.send"] = ["sent": true, "path": path, "report_id": "x"]
        await store.sendDiagnostic(appVersion: "0.27 (1)")                         // Send again
        XCTAssertEqual(store.diagnostic, .sent)
        XCTAssertEqual(rpc.params(of: "reports.send").count, 2)
    }
}
