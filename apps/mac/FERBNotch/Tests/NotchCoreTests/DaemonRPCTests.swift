import XCTest
import Foundation
import Darwin
@testable import NotchCore

final class DaemonRPCTests: XCTestCase {
    // decision: APP-570
    func testStreamingDeliversFragmentedEventThenEndsOnMatchingResponse() async throws {
        let server = try RPCFixture(notifications: [
            ["method": "unrelated.event", "params": ["type": "wrong"]],
            ["method": "chat.send.event", "params": ["type": "plan", "task_id": "backend-123"]]
        ]) { request in ["id": request["id"]!, "result": ["ended": true]] }
        defer { server.close() }
        var received: [[String: Any]] = []
        for try await event in DaemonRPC(socketPath: server.path).stream("chat.send", params: ["text": "Example fixture request"]) {
            received.append(event)
        }
        XCTAssertEqual(received.count, 1)
        XCTAssertEqual(received.first?["task_id"] as? String, "backend-123")
    }

    // decision: APP-593
    func testRemoteApprovalErrorsDoNotIncludeRequestBodies() async throws {
        let server = try RPCFixture { request in
            if request["method"] as? String == "auth.hello" {
                return ["id": request["id"]!, "result": ["ok": true]]
            }
            return ["id": request["id"]!, "error": ["code": -32000, "message": "Approval is no longer pending"]]
        }
        defer { server.close() }
        do {
            _ = try await DaemonRPC(socketPath: server.path).call("chat.answer", params: ["approval_id": "a", "text": "private-test-value"])
            XCTFail("Expected failure")
        } catch {
            XCTAssertFalse(error.localizedDescription.contains("private-test-value"))
        }
    }

    // decision: APP-913
    // A stopped preview loop's call ends with it (journal 66): its connection closes at once, so the daemon stops the
    // capture nobody waits for and the thread waiting for the reply is freed, not held for the 10 s reply wait.
    func testACancelledCallClosesItsConnectionAtOnce() async throws {
        let release = DispatchSemaphore(value: 0)
        let server = try RPCFixture { request in
            if request["method"] as? String != "auth.hello" { _ = release.wait(timeout: .now() + 8) }
            return ["id": request["id"]!, "result": ["ok": true]]
        }
        defer { release.signal(); server.close() }
        let rpc = DaemonRPC(socketPath: server.path)
        let call = Task { try await rpc.call("tasks.preview", params: ["task_id": "t-example"]) }
        try await Task.sleep(for: .milliseconds(300))
        let cancelled = Date()
        call.cancel()
        do {
            _ = try await call.value
            XCTFail("A cancelled call has no answer")
        } catch {}
        XCTAssertLessThan(Date().timeIntervalSince(cancelled), 2, "the call ended with its caller, not at the reply wait")
    }
}

private final class RPCFixture {
    let path = "/tmp/ferb-ui-\(UUID().uuidString).sock"
    private let socketFD: Int32
    init(notifications: [[String: Any]] = [], reply: @escaping ([String: Any]) -> [String: Any]) throws {
        socketFD = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8CString)
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in buffer.copyBytes(from: bytes.map { UInt8(bitPattern: $0) }) }
        let bound = withUnsafePointer(to: &address) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.bind(socketFD, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0, listen(socketFD, 1) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        let listener = socketFD
        DispatchQueue.global().async {
            let peer = Darwin.accept(listener, nil, nil)
            guard peer >= 0 else { return }
            defer { Darwin.close(peer) }
            var noSignal: Int32 = 1        // a reply to a caller that left fails, never kills the test process
            setsockopt(peer, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
            var buffer = Data()
            var byte: UInt8 = 0
            while Darwin.recv(peer, &byte, 1, 0) == 1 {
                if byte != 10 { buffer.append(byte); continue }
                guard let request = try? JSONSerialization.jsonObject(with: buffer) as? [String: Any] else { return }
                buffer.removeAll()
                let events = request["method"] as? String == "auth.hello" ? [] : notifications
                for message in events + [reply(request)] {
                    let response = try! JSONSerialization.data(withJSONObject: message) + Data([10])
                    for value in response { var value = value; _ = Darwin.send(peer, &value, 1, 0) }
                }
            }
        }
    }
    func close() { Darwin.close(socketFD); unlink(path) }
}
