import Darwin
import Network
import XCTest
@testable import NotchCore

/// FERB.app's loopback return (APP-1137, REL-452; RFC 8252 §7.3): 127.0.0.1 only, one GET /callback with this
/// sign-in's state and Host, everything else a 404 that changes nothing; the tab waits for FERB's answer; Cancel and
/// the timeout close it. Real sockets on this Mac's loopback, nothing else.
final class LoopbackReturnTests: XCTestCase {
    static let state = "state-0123456789abcdef"

    /// One raw HTTP exchange with 127.0.0.1:`port`: the bytes sent, everything answered until the server closes.
    func exchange(_ port: UInt16, _ request: String, timeout: TimeInterval = 5) async -> String? {
        await withCheckedContinuation { (continuation: CheckedContinuation<String?, Never>) in
            let queue = DispatchQueue(label: "loopback-test-client")
            let connection = NWConnection(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!, using: .tcp)
            var answer = Data()
            var resumed = false
            let done: (String?) -> Void = { text in
                guard !resumed else { return }
                resumed = true
                connection.cancel()
                continuation.resume(returning: text)
            }
            func read() {
                connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { data, _, complete, error in
                    if let data { answer.append(data) }
                    if complete || error != nil {
                        done(answer.isEmpty ? nil : String(decoding: answer, as: UTF8.self))
                    } else {
                        read()
                    }
                }
            }
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    connection.send(content: Data(request.utf8), completion: .contentProcessed { _ in read() })
                case .failed, .waiting:
                    done(nil)
                default: break
                }
            }
            connection.start(queue: queue)
            queue.asyncAfter(deadline: .now() + timeout) { done(answer.isEmpty ? nil : String(decoding: answer, as: UTF8.self)) }
        }
    }

    func get(_ port: UInt16, _ target: String, host: String? = nil, method: String = "GET") -> String {
        "\(method) \(target) HTTP/1.1\r\nHost: \(host ?? "127.0.0.1:\(port)")\r\nUser-Agent: test\r\n\r\n"
    }

    func opened(_ loopback: LoopbackReturn) async throws -> UInt16 {
        let maybe = await loopback.open()
        let address = try XCTUnwrap(maybe)
        XCTAssertEqual(address.scheme, "http")
        XCTAssertEqual(address.host, "127.0.0.1")
        XCTAssertEqual(address.path, "/callback")
        return UInt16(try XCTUnwrap(address.port))
    }

    // decision: APP-1137, APP-1138
    func testOnlyThisSignInsCallbackIsTakenAndEverythingElseIsA404ThatChangesNothing() async throws {
        let loopback = LoopbackReturn(state: Self.state)
        let port = try await opened(loopback)
        let good = "/callback?code=one-time&state=\(Self.state)"
        let refused = [
            get(port, "/callback?code=one-time&state=someone-elses-state"),        // another state
            get(port, "/callback?code=one-time"),                                   // no state
            get(port, "/elsewhere?code=one-time&state=\(Self.state)"),              // another path
            get(port, "/callback/?code=one-time&state=\(Self.state)"),
            get(port, "/callbackx?code=one-time&state=\(Self.state)"),
            get(port, "//callback?code=one-time&state=\(Self.state)"),
            get(port, good, method: "POST"),                                        // another method
            get(port, good, host: "localhost:\(port)"),                             // another Host (DNS rebinding)
            get(port, good, host: "evil.example.com"),
            get(port, good, host: "127.0.0.1:\(port == 65535 ? 65534 : port + 1)"),
            "GET \(good) HTTP/1.1\r\nUser-Agent: test\r\n\r\n",                     // no Host
            "GET \(good) HTTP/1.1\r\nHost: 127.0.0.1:\(port)\r\nHost: 127.0.0.1:\(port)\r\n\r\n",
            "GET \(good) HTTP/2\r\nHost: 127.0.0.1:\(port)\r\n\r\n",
            "GET \(good)\r\nHost: 127.0.0.1:\(port)\r\n\r\n",
            "GET /callback?x=" + String(repeating: "a", count: LoopbackReturn.headerLimit) + " HTTP/1.1\r\n\r\n",
        ]
        for request in refused {
            let answer = await exchange(port, request)
            XCTAssertTrue(answer?.hasPrefix("HTTP/1.1 404 Not Found\r\n") == true, String(request.prefix(80)))
        }
        // still waiting: this sign-in's callback is taken
        async let result = loopback.wait()
        async let page = exchange(port, get(port, good))
        let got = await result
        XCTAssertEqual(got, .callback(URL(string: "http://127.0.0.1:\(port)\(good)")!))
        // the tab waits for FERB's answer, then says it
        loopback.answer(.signedIn)
        let pageText = await page
        let shown = try XCTUnwrap(pageText)
        XCTAssertTrue(shown.hasPrefix("HTTP/1.1 200 OK\r\n"))
        XCTAssertTrue(shown.contains("Content-Type: text/html; charset=utf-8"))
        XCTAssertTrue(shown.contains("Cache-Control: no-store"))
        XCTAssertTrue(shown.contains("Referrer-Policy: no-referrer"))
        XCTAssertTrue(shown.contains("Content-Security-Policy: default-src 'none'"))
        XCTAssertTrue(shown.contains(AccountWords.pageSignedIn))
        XCTAssertFalse(shown.contains("<script"))
        // one request: the port is closed now
        let after = await exchange(port, get(port, good), timeout: 2)
        XCTAssertNil(after)
    }

    // decision: APP-1137, APP-1138
    func testATabWhoseSignInDidntFinishIsToldSo() async throws {
        for (ending, words) in [(LoopbackReturn.Ending.failed, AccountWords.pageFailed),
                                (.cancelled, AccountWords.pageCancelled)] {
            let loopback = LoopbackReturn(state: Self.state)
            let port = try await opened(loopback)
            async let page = exchange(port, get(port, "/callback?error=cancelled&state=\(Self.state)"))
            _ = await loopback.wait()
            loopback.answer(ending)
            let shown = await page
            XCTAssertTrue(shown?.contains(words) == true, "\(ending)")
        }
        // FERB never answering: after the wait the tab sends the person back to FERB
        let loopback = LoopbackReturn(state: Self.state, answerWait: 0.2)
        let port = try await opened(loopback)
        let shown = await exchange(port, get(port, "/callback?code=c&state=\(Self.state)"))
        XCTAssertTrue(shown?.contains(AccountWords.pageUnanswered) == true)
    }

    // decision: APP-1137, APP-1138
    func testCancelAndTheTimeoutCloseThePort() async throws {
        let cancelled = LoopbackReturn(state: Self.state)
        let port = try await opened(cancelled)
        cancelled.cancel()
        let result = await cancelled.wait()
        XCTAssertEqual(result, .cancelled)
        let after = await exchange(port, get(port, "/callback?code=c&state=\(Self.state)"), timeout: 2)
        XCTAssertNil(after)

        let slow = LoopbackReturn(state: Self.state, timeout: 0.3)
        let slowPort = try await opened(slow)
        let timedOut = await slow.wait()
        XCTAssertEqual(timedOut, .timedOut)
        let late = await exchange(slowPort, get(slowPort, "/callback?code=c&state=\(Self.state)"), timeout: 2)
        XCTAssertNil(late)
    }

    /// A named port (tests, never FERB.app) is the one listened on; a port already taken gives no address, so the
    /// browser never opens on a return that can't come back.
    // decision: APP-1137, APP-1138
    func testANamedPortIsUsedAndATakenOneGivesNoAddress() async throws {
        let first = LoopbackReturn(state: Self.state)
        let port = try await opened(first)
        let second = LoopbackReturn(state: Self.state, port: port)
        let address = await second.open()
        XCTAssertNil(address)
        first.cancel()
        _ = await first.wait()
        let free = try XCTUnwrap(Self.freePort())
        let named = LoopbackReturn(state: Self.state, port: free)
        let address2 = await named.open()
        XCTAssertEqual(address2, AccountSite.returnAddress(port: free))
        let answer = await exchange(free, get(free, "/nothing"))
        XCTAssertTrue(answer?.hasPrefix("HTTP/1.1 404") == true)
        named.cancel()
    }

    /// A port nothing listens on now: bound and closed at once (a socket that never listened frees it at once).
    static func freePort() -> UInt16? {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        address.sin_port = 0
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let bound = withUnsafeMutablePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.bind(fd, $0, length) == 0 && getsockname(fd, $0, &length) == 0 }
        }
        return bound ? UInt16(bigEndian: address.sin_port) : nil
    }

    /// The whole browser step with its seams: the start page names the loopback address, a callback comes back, the
    /// tab is answered and FERB comes forward.
    // decision: APP-1137, APP-1138
    @MainActor
    func testTheBrowserStepOpensTheStartPageAndBringsFERBForward() async throws {
        var openedURL: URL?
        var forward = 0
        let browser = BrowserSignIn(open: { openedURL = $0; return true }, bringForward: { forward += 1 })
        final class Box { var url: URL? }
        let returned = Box()
        let request = AccountRequest(verifier: String(repeating: "v", count: 43), state: Self.state)
        let running = Task { @MainActor in
            await browser.run(state: Self.state) { address in
                returned.url = address
                return request.startURL(.google, redirect: address)
            }
        }
        // wait until the page was "opened", then play the browser
        var tries = 0
        while openedURL == nil && tries < 200 { try await Task.sleep(nanoseconds: 10_000_000); tries += 1 }
        let back = try XCTUnwrap(returned.url)
        XCTAssertTrue(openedURL?.absoluteString.contains("redirect=\(back.absoluteString)") == true)
        let port = UInt16(try XCTUnwrap(back.port))
        async let page = exchange(port, get(port, "/callback?code=c&state=\(Self.state)"))
        let got = await running.value
        guard case .callback = got else { return XCTFail("\(got)") }
        browser.finish(.signedIn)
        let shown = await page
        XCTAssertTrue(shown?.contains(AccountWords.pageSignedIn) == true)
        XCTAssertEqual(forward, 1)

        // no browser opened: the port closes and the step says so
        let none = BrowserSignIn(open: { _ in false }, bringForward: {})
        let failed = await none.run(state: Self.state) { $0 }
        XCTAssertEqual(failed, .failed)
    }
}
