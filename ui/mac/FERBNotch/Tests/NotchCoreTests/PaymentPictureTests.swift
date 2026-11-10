import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// GATE-579, APP-828: the payment card's one-time picture of the payment page: asked for once by the question's id,
/// kept in memory only while the question is open, never asked for a card without one.
@MainActor
final class PaymentPictureTests: XCTestCase {
    // decision: APP-828, GATE-579
    func testTheCardReadsWhetherItHasThePagesPicture() throws {
        let shown = try XCTUnwrap(DaemonApproval(["approval_id": "ap-1", "summary": "Pay ₹70?", "picture": true]))
        XCTAssertTrue(shown.hasPicture)
        XCTAssertNil(shown.pictureProblem)
        let missing = try XCTUnwrap(DaemonApproval(["approval_id": "ap-2", "summary": "Pay ₹70?",
                                                    "picture_problem": "FERB couldn't capture the payment page."]))
        XCTAssertFalse(missing.hasPicture)
        XCTAssertEqual(missing.pictureProblem, "FERB couldn't capture the payment page.")
        let plain = try XCTUnwrap(DaemonApproval(["approval_id": "ap-3", "summary": "Delete this for good?"]))
        XCTAssertFalse(plain.hasPicture)
        XCTAssertNil(plain.pictureProblem)
    }

    // decision: APP-828
    func testThePictureIsAskedForOnceAndGoesWithItsQuestion() async throws {
        let rpc = PictureRPC(image: Self.png())
        let pictures = PaymentPictures(rpc: rpc)
        let card = try XCTUnwrap(DaemonApproval(["approval_id": "ap-1", "picture": true]))
        let plain = try XCTUnwrap(DaemonApproval(["approval_id": "ap-2"]))
        pictures.load(card)
        pictures.load(card)                       // the notch and the chat window: one fetch between them
        pictures.load(plain)
        for _ in 0..<200 where pictures.images["ap-1"] == nil { try await Task.sleep(for: .milliseconds(10)) }
        XCTAssertNotNil(pictures.images["ap-1"])
        XCTAssertEqual(rpc.calls.map(\.0), ["questions.picture"])
        XCTAssertEqual(rpc.calls.first?.1["approval_id"] as? String, "ap-1")
        pictures.keep(["ap-1"])
        XCTAssertNotNil(pictures.images["ap-1"])
        pictures.keep([])                         // answered: its picture goes
        XCTAssertTrue(pictures.images.isEmpty)
    }

    /// Owner 2026-11-11 (live, payment card ap-dcef18e50af4): "i didnt see ht orprev amzn checkout page … it isnt
    /// showing". The daemon said `picture: true`, but the card's picture view was an empty Group until the picture
    /// came, and an empty Group never appears, so it never asked for it: no picture and no line. The card as the
    /// notch's row and the chat window's transcript draw it, through the model the connection fills, asks once
    /// between them and shows the picture; a picture that can't be fetched shows one line instead, never nothing.
    // decision: APP-887, APP-828, GATE-579
    func testTheCardAsDrawnAsksForThePictureAndShowsItOrSaysItCouldnt() throws {
        let rpc = ScriptedRPC()
        rpc.replies["questions.picture"] = ["image": Self.png()]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        let payment: [String: Any] = ["approval_id": "ap-pay", "capability": "ferb.confirm", "kind": "owner_question",
                                      "reason": "FERB: this pays INR 72.05 on amazon.in…", "summary": "Pay INR 72.05?",
                                      "exact": "INR 72.05 · amazon.in", "options": ["Yes", "No"],
                                      "labels": ["Pay INR 72.05", "Don't pay"], "picture": true,
                                      "title": "Check before FERB goes on", "task_id": "task-1", "corrects": true]
        model.receiveBusEvent(["topic": "approval.requested", "data": payment])
        let approval = try XCTUnwrap(model.pendingApprovals.first)
        XCTAssertTrue(approval.hasPicture)
        // the notch card's row and the chat window's card (both are ApprovalView, inRow or not)
        let row = NSHostingView(rootView: ApprovalView(model: model, approval: approval, inRow: true).frame(width: 360))
        let window = NSHostingView(rootView: ApprovalView(model: model, approval: approval).frame(width: 390))
        for host in [row, window] {
            host.frame = CGRect(x: 0, y: 0, width: 390, height: 500)
            host.layoutSubtreeIfNeeded()
        }
        let deadline = Date().addingTimeInterval(3)
        while model.paymentPictures.images["ap-pay"] == nil, Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        }
        XCTAssertNotNil(model.paymentPictures.images["ap-pay"], "the card never asked for its picture")
        XCTAssertEqual(rpc.calls.filter { $0.0 == "questions.picture" }.count, 1, "one fetch between the two cards")

        // the picture taken already (or the call failed): the card says so in one line
        let gone = ScriptedRPC()
        gone.replies["questions.picture"] = [:]
        let pictures = PaymentPictures(rpc: gone)
        pictures.load(approval)
        let until = Date().addingTimeInterval(3)
        while pictures.missing["ap-pay"] == nil, Date() < until { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        XCTAssertEqual(pictures.missing["ap-pay"], PaymentPictures.notShown)
        XCTAssertNil(pictures.images["ap-pay"])
    }

    private static func png() -> String {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 4, pixelsHigh: 3, bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                   bytesPerRow: 0, bitsPerPixel: 0)!
        return rep.representation(using: .png, properties: [:])!.base64EncodedString()
    }
}

private final class PictureRPC: DaemonRPCTransport {
    private let recorded = RecordedCalls()
    var calls: [(String, [String: Any])] { recorded.all }
    let image: String
    init(image: String) { self.image = image }
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        return method == "questions.picture" ? ["image": image] : [:]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
