import XCTest
import CoreGraphics
@testable import NotchCore

final class NotchCoreTests: XCTestCase {
    func testCompletedDocumentCanBeRecalledWithItsOriginalContext() {
        var session = NotchSession()
        let source = URL(fileURLWithPath: "/tmp/Original notes.txt")
        let document = URL(fileURLWithPath: "/tmp/Example brief.pdf")
        session.draft = "Make a brief from my notes"
        session.attach([source])
        let id = session.beginWork()!
        XCTAssertTrue(session.finishWork(taskID: id, documentURL: document))
        let receipt = session.result!
        session.newRequest()
        session.draft = "Another request"
        session.recall(receipt)
        XCTAssertEqual(session.result?.documentURL, document)
        XCTAssertEqual(session.draft, receipt.request)
        XCTAssertEqual(session.attachments, [source])
    }

    func testAttentionInvalidatesPendingCompletionAndKeepsTheRequest() {
        var session = NotchSession()
        session.draft = "Make a brief"
        let id = session.beginWork()!
        session.requireAttention("The example could not be saved.")
        XCTAssertFalse(session.finishWork(taskID: id))
        XCTAssertEqual(session.phase, .attention)
        XCTAssertEqual(session.draft, "Make a brief")
        XCTAssertTrue(session.history.isEmpty)
    }

    // decision: APP-550
    func testPanelAttachesBelowThePhysicalCameraCutout() {
        let geometry = NotchGeometry(screen: CGRect(x: 0, y: 0, width: 1512, height: 982),
                                     topInset: 32, leftAreaMaxX: 666, rightAreaMinX: 846,
                                     menuBarHeight: 32)
        XCTAssertEqual(geometry.centerX, 756)
        XCTAssertEqual(geometry.handleWidth, 180)
        XCTAssertEqual(geometry.topEdge, 950)
        XCTAssertEqual(geometry.panelFrame(size: CGSize(width: 528, height: 354)).maxY, 950)
    }

    // decision: APP-518
    func testANotchlessDisplayGetsAVirtualNotchOverTheMenuBar() {
        // Owner 2026-10-27 (a friend's M1): a black pill at the top centre, as tall as the menu bar, not a tab below it
        let geometry = NotchGeometry(screen: CGRect(x: -1920, y: 200, width: 1920, height: 1080),
                                     topInset: 0, leftAreaMaxX: nil, rightAreaMinX: nil,
                                     menuBarHeight: 25)
        XCTAssertFalse(geometry.hasNotch)
        XCTAssertEqual(geometry.centerX, -960)
        XCTAssertEqual(geometry.cutoutFrame, CGRect(x: -1042, y: 1255, width: 164, height: 25))
        let hidden = NotchGeometry(screen: CGRect(x: 0, y: 0, width: 1440, height: 900), topInset: 0,
                                   leftAreaMaxX: nil, rightAreaMinX: nil, menuBarHeight: 0)
        XCTAssertEqual(hidden.cutoutFrame.maxY, 900)                  // a hidden menu bar: still at the top edge
        XCTAssertEqual(hidden.cutoutFrame.height, 24)
    }

    // decision: APP-550
    func testANotchedDisplaysPillIsTheCameraHousing() {
        let geometry = NotchGeometry(screen: CGRect(x: 0, y: 0, width: 1512, height: 982),
                                     topInset: 32, leftAreaMaxX: 666, rightAreaMinX: 846,
                                     menuBarHeight: 32)
        XCTAssertEqual(geometry.cutoutFrame, CGRect(x: 666, y: 950, width: 180, height: 32))
    }

    // decision: APP-564
    func testClosingTheSurfaceDoesNotStopTheTask() {
        var session = NotchSession()
        session.draft = "Make a brief"
        let taskID = session.beginWork()!
        session.collapse()
        XCTAssertEqual(session.phase, .working)
        XCTAssertEqual(session.activeTaskID, taskID)
        XCTAssertTrue(session.finishWork(taskID: taskID))
        XCTAssertFalse(session.isExpanded)
        XCTAssertEqual(session.phase, .result)
        XCTAssertEqual(session.history.count, 1)
    }

    func testStoppedTaskCannotPublishALateResult() {
        var session = NotchSession()
        session.draft = "Make a brief"
        let taskID = session.beginWork()!
        session.stopWork()
        XCTAssertFalse(session.finishWork(taskID: taskID))
        XCTAssertEqual(session.phase, .stopped)
        XCTAssertTrue(session.history.isEmpty)
    }

    func testAnOldCompletionCannotReplaceANewerTask() {
        var session = NotchSession()
        session.draft = "First"
        let oldID = session.beginWork()!
        session.stopWork()
        session.newRequest()
        session.draft = "Second"
        let currentID = session.beginWork()!
        XCTAssertFalse(session.finishWork(taskID: oldID))
        XCTAssertEqual(session.activeTaskID, currentID)
        XCTAssertTrue(session.finishWork(taskID: currentID))
        XCTAssertEqual(session.result?.request, "Second")
    }

    func testEmptyRequestsCannotStartAndAttachmentsAreDeduplicated() {
        var session = NotchSession()
        session.draft = " \n "
        XCTAssertNil(session.beginWork())
        let file = URL(fileURLWithPath: "/tmp/Notes.txt")
        session.attach([file, file])
        XCTAssertEqual(session.attachments.count, 1)
        session.removeAttachment(file)
        XCTAssertTrue(session.attachments.isEmpty)
    }

    func testResultKeepsOriginalContextAfterStartingAnotherRequest() {
        var session = NotchSession()
        session.draft = "An example handoff"
        session.attach([URL(fileURLWithPath: "/tmp/Notes.txt")])
        let id = session.beginWork()!
        XCTAssertTrue(session.finishWork(taskID: id))
        session.newRequest()
        XCTAssertEqual(session.history.first?.request, "An example handoff")
        XCTAssertEqual(session.history.first?.sources.first?.lastPathComponent, "Notes.txt")
    }
}
