import XCTest
@testable import NotchCore

final class DaemonSessionTests: XCTestCase {
    func testBackendIdentityAndUnverifiedResultArePreserved() {
        var session = NotchSession()
        session.draft = "Find the report"
        let local = session.beginWork()!
        session.bindBackendTask("real-task-id", localID: local)
        XCTAssertEqual(session.backendTaskID, "real-task-id")
        XCTAssertTrue(session.finishBackendWork(taskID: "real-task-id", status: "done_unverified", answer: "Saved; contents not checked", artifacts: ["/tmp/Actual report.pdf"]))
        XCTAssertEqual(session.phase, .attention)
        XCTAssertEqual(session.result?.answer, "Saved; contents not checked")
        XCTAssertEqual(session.result?.documentURL?.lastPathComponent, "Actual report.pdf")
        XCTAssertEqual(session.result?.backendTaskID, "real-task-id")
    }

    func testApprovalPausesPresentationWithoutLosingBackendTaskAndStaleOutcomeIsIgnored() {
        var session = NotchSession()
        session.resumeBackendTask("task-new", goal: "Sign in")
        session.waitForOwner("Enter code")
        XCTAssertEqual(session.phase, .attention)
        XCTAssertEqual(session.backendTaskID, "task-new")
        XCTAssertFalse(session.finishBackendWork(taskID: "task-old", status: "done", answer: "Wrong result", artifacts: []))
        session.resumeAfterOwnerAnswer()
        XCTAssertEqual(session.phase, .working)
    }

    func testVoicePreviewDoesNotCreateARealRequest() {
        var session = NotchSession()
        session.draft = "Existing draft"
        session.beginVoicePreview()
        session.finishVoicePreview()
        XCTAssertEqual(session.draft, "Existing draft")
    }
}
