import AppKit
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
