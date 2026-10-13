import Foundation
import XCTest
@testable import NotchCore

final class RPCLineBufferTests: XCTestCase {
    func testFragmentedLargeTraceScansOnlyNewBytesAndPreservesFullUTF8() throws {
        // A full task trace can contain multi-megabyte output and split UTF-8 scalars.
        let output = String(repeating: "trace — उत्तर 🎵\n", count: 150_000)
        let frame = try JSONSerialization.data(withJSONObject: ["result": ["trace": output]])
        var buffer = RPCLineBuffer()
        var cursor = 0
        var examinedBytes = 0
        while cursor < frame.count {
            let end = min(cursor + 8191, frame.count)
            buffer.append(frame[cursor..<end])
            XCTAssertEqual(buffer.searchOffset, cursor)
            examinedBytes += end - buffer.searchOffset
            XCTAssertNil(buffer.nextLine())
            XCTAssertEqual(buffer.searchOffset, end)
            // Polling without another recv must not rescan the accumulated trace.
            XCTAssertNil(buffer.nextLine())
            XCTAssertEqual(buffer.searchOffset, end)
            cursor = end
        }
        XCTAssertEqual(examinedBytes, frame.count)
        buffer.append(Data([10]))
        let received = try XCTUnwrap(buffer.nextLine())
        XCTAssertEqual(received, frame)
        let decoded = try XCTUnwrap(JSONSerialization.jsonObject(with: received) as? [String: [String: String]])
        XCTAssertEqual(decoded["result"]?["trace"], output)
        XCTAssertNil(buffer.nextLine())
        XCTAssertEqual(buffer.searchOffset, 0)
    }

    func testCoalescedFramesPartialRemainderAndRepeatedCompaction() {
        var buffer = RPCLineBuffer()
        buffer.append(Data("first\nsecond\npar".utf8))
        XCTAssertEqual(buffer.nextLine(), Data("first".utf8))
        XCTAssertEqual(buffer.nextLine(), Data("second".utf8))
        XCTAssertNil(buffer.nextLine())
        XCTAssertEqual(buffer.searchOffset, 3)
        buffer.append(Data("tial\n\nlast".utf8))
        XCTAssertEqual(buffer.nextLine(), Data("partial".utf8))
        XCTAssertEqual(buffer.nextLine(), Data())
        XCTAssertNil(buffer.nextLine())
        XCTAssertEqual(buffer.searchOffset, 4)
        buffer.append(Data([10]))
        XCTAssertEqual(buffer.nextLine(), Data("last".utf8))
        XCTAssertNil(buffer.nextLine())
        buffer.append(Data("after-empty\n".utf8))
        XCTAssertEqual(buffer.nextLine(), Data("after-empty".utf8))
    }
}
