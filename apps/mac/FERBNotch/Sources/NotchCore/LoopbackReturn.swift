import Foundation
import Network

/// FERB.app's way back from the sign-in in the person's own browser (REL-452, APP-1138; RFC 8252 §7.3, the loopback
/// redirect): FERB.app opens the site's start page in the default browser, as other apps do, and listens here for the
/// one request the site's last redirect sends the browser to, `http://127.0.0.1:<port>/callback?code=…&state=…`.
///
/// The rules, all in code:
/// - It listens on 127.0.0.1 only (never every interface, never the name "localhost"), on a port the system picks
///   (tests may name one), and stops at the first of: this sign-in's callback, Cancel, the timeout (10 minutes).
/// - The one request it takes is `GET /callback` with `Host: 127.0.0.1:<port>` and this sign-in's state (compared in
///   constant time by `AccountCallback.read`). Anything else on the port (another path or method, another Host, a
///   wrong or missing state, an oversized or malformed request) gets a plain 404 and changes nothing, so another
///   program on this Mac can neither end nor steer the sign-in. Even a program that saw the real request couldn't use
///   its code: the site trades a code only with this sign-in's PKCE verifier, which never leaves FERB.app.
/// - The browser's tab waits for FERB.app's answer (`answer`), so it says what really happened: signed in only once
///   FERB.app has kept the sign-in. Then the connection closes; the listener has closed already.
public final class LoopbackReturn: @unchecked Sendable {
    /// How the sign-in ended, as the browser's tab says it.
    public enum Ending: Equatable, Sendable {
        case signedIn, cancelled, failed
        /// FERB.app gave no answer in time (it never should): the tab sends the person back to FERB.
        case unanswered
    }

    public static let headerLimit = 8 * 1024
    public static let connectionLimit = 16

    private let state: String
    private let port: UInt16
    private let timeout: TimeInterval
    private let answerWait: TimeInterval
    private let queue = DispatchQueue(label: "com.ferb.account.loopback")

    // Only touched on `queue`.
    private var listener: NWListener?
    private var bound: UInt16?
    private var opening: CheckedContinuation<URL?, Never>?
    private var connections: [ObjectIdentifier: NWConnection] = [:]
    private var taken: NWConnection?
    private var result: WebSignInResult?
    private var waiter: CheckedContinuation<WebSignInResult, Never>?

    /// `port` 0: the system picks one. `timeout`: how long the browser may take; `answerWait`: how long the tab waits
    /// for `answer` before it is told to go back to FERB.
    public init(state: String, port: UInt16 = 0, timeout: TimeInterval = AccountSite.signInTimeout,
                answerWait: TimeInterval = 60) {
        self.state = state
        self.port = port
        self.timeout = timeout
        self.answerWait = answerWait
    }

    /// Starts listening; the address the site sends the browser back to, or nil when nothing could listen.
    public func open() async -> URL? {
        await withCheckedContinuation { (continuation: CheckedContinuation<URL?, Never>) in
            queue.async { self.start(continuation) }
        }
    }

    /// This sign-in's callback (`.callback`, the full address the browser asked for), or how the wait ended.
    public func wait() async -> WebSignInResult {
        await withCheckedContinuation { (continuation: CheckedContinuation<WebSignInResult, Never>) in
            queue.async {
                if let result = self.result { continuation.resume(returning: result) } else { self.waiter = continuation }
            }
        }
    }

    /// The page for the browser's tab that brought the callback, then everything closes.
    public func answer(_ ending: Ending) {
        queue.async { self.reply(ending) }
    }

    /// Stops waiting (Cancel). A callback already taken keeps its tab until `answer`.
    public func cancel() {
        queue.async { self.end(.cancelled) }
    }

    // MARK: - On the queue

    private func start(_ continuation: CheckedContinuation<URL?, Never>) {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: NWEndpoint.Port(rawValue: port) ?? .any)
        parameters.acceptLocalOnly = true
        parameters.allowLocalEndpointReuse = false
        guard result == nil, let listener = try? NWListener(using: parameters) else {
            continuation.resume(returning: nil)
            return
        }
        self.listener = listener
        opening = continuation
        listener.stateUpdateHandler = { [weak self] state in self?.listenerChanged(state) }
        listener.newConnectionHandler = { [weak self] connection in self?.arrived(connection) }
        listener.start(queue: queue)
        queue.asyncAfter(deadline: .now() + timeout) { [weak self] in self?.end(.timedOut) }
    }

    private func listenerChanged(_ state: NWListener.State) {
        switch state {
        case .ready:
            guard let port = listener?.port?.rawValue, port != 0 else { return opened(nil) }
            bound = port
            opened(AccountSite.returnAddress(port: port))
        case .failed, .cancelled:
            opened(nil)
            if result == nil, taken == nil { end(.failed) }
        default:
            break
        }
    }

    private func opened(_ address: URL?) {
        opening?.resume(returning: address)
        opening = nil
    }

    private func arrived(_ connection: NWConnection) {
        guard result == nil, taken == nil, connections.count < Self.connectionLimit else {
            connection.cancel()
            return
        }
        connections[ObjectIdentifier(connection)] = connection
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self, let connection else { return }
            switch state {
            case .failed, .cancelled: self.connections[ObjectIdentifier(connection)] = nil
            default: break
            }
        }
        connection.start(queue: queue)
        read(connection, Data())
    }

    private func read(_ connection: NWConnection, _ head: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: Self.headerLimit) { [weak self] data, _, done, error in
            guard let self else { return }
            var head = head
            if let data { head.append(data) }
            if let end = head.range(of: Data("\r\n\r\n".utf8)) {
                self.handle(connection, head.subdata(in: head.startIndex..<end.lowerBound))
            } else if head.count > Self.headerLimit || done || error != nil {
                self.refuse(connection)
            } else {
                self.read(connection, head)
            }
        }
    }

    private func handle(_ connection: NWConnection, _ head: Data) {
        guard result == nil, taken == nil, let bound, let url = Self.callback(head, port: bound),
              AccountCallback.read(url, state: state, returnAddress: AccountSite.returnAddress(port: bound)) != nil else {
            return refuse(connection)
        }
        taken = connection
        connections[ObjectIdentifier(connection)] = nil
        closeListener()
        finish(.callback(url))
        queue.asyncAfter(deadline: .now() + answerWait) { [weak self] in self?.reply(.unanswered) }
    }

    /// The address of a well-formed `GET /callback?… HTTP/1.x` request for `127.0.0.1:<port>`, else nil.
    static func callback(_ head: Data, port: UInt16) -> URL? {
        guard head.count <= headerLimit, let text = String(data: head, encoding: .utf8) else { return nil }
        let lines = text.components(separatedBy: "\r\n")
        let request = lines[0].split(separator: " ", omittingEmptySubsequences: false)
        guard request.count == 3, request[0] == "GET", request[2] == "HTTP/1.1" || request[2] == "HTTP/1.0" else {
            return nil
        }
        let target = String(request[1])
        guard target == AccountSite.callbackPath || target.hasPrefix(AccountSite.callbackPath + "?") else { return nil }
        let hosts = lines.dropFirst().compactMap { line -> String? in
            guard let colon = line.firstIndex(of: ":"),
                  line[..<colon].trimmingCharacters(in: .whitespaces).lowercased() == "host" else { return nil }
            return line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        }
        guard hosts == ["\(AccountSite.loopbackHost):\(port)"] else { return nil }
        return URL(string: "http://\(AccountSite.loopbackHost):\(port)\(target)")
    }

    private func refuse(_ connection: NWConnection) {
        connections[ObjectIdentifier(connection)] = nil
        send(connection, status: "404 Not Found", type: "text/plain; charset=utf-8", body: "Not found\n")
    }

    private func reply(_ ending: Ending) {
        guard let connection = taken else { return }
        taken = nil
        send(connection, status: "200 OK", type: "text/html; charset=utf-8", body: Self.page(ending))
        end(.cancelled)
    }

    private func send(_ connection: NWConnection, status: String, type: String, body: String) {
        let bytes = Data(body.utf8)
        let head = "HTTP/1.1 \(status)\r\nContent-Type: \(type)\r\nContent-Length: \(bytes.count)\r\n" +
            "Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n" +
            "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; " +
            "form-action 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n"
        connection.send(content: Data(head.utf8) + bytes, completion: .contentProcessed { _ in connection.cancel() })
    }

    /// The wait is over with `outcome` (the first outcome counts); the listener and every connection but a taken one
    /// close.
    private func end(_ outcome: WebSignInResult) {
        finish(outcome)
        closeListener()
        for connection in connections.values { connection.cancel() }
        connections.removeAll()
    }

    private func finish(_ outcome: WebSignInResult) {
        guard result == nil else { return }
        result = outcome
        waiter?.resume(returning: outcome)
        waiter = nil
    }

    private func closeListener() {
        opened(nil)
        listener?.stateUpdateHandler = nil
        listener?.newConnectionHandler = nil
        listener?.cancel()
        listener = nil
    }

    // MARK: - The page

    /// A small page in the look of FERB's site (its colors, type and logo), with nothing fetched: the listener is
    /// closed by the time the browser could ask for anything.
    static func page(_ ending: Ending) -> String {
        let text: String
        switch ending {
        case .signedIn: text = AccountWords.pageSignedIn
        case .cancelled: text = AccountWords.pageCancelled
        case .failed: text = AccountWords.pageFailed
        case .unanswered: text = AccountWords.pageUnanswered
        }
        return """
        <!doctype html>
        <html lang="en">
        <head>
        <meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <meta name="robots" content="noindex">
        <title>sign in · FERB</title>
        <link rel="icon" href="data:,">
        <style>
        *{margin:0;padding:0;box-sizing:border-box}
        body{background:#fafafa;color:#333;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;font-size:1rem;line-height:1.7;-webkit-font-smoothing:antialiased}
        main{max-width:520px;margin:0 auto;padding:4rem 1.5rem}
        svg{display:block;width:48px;height:48px;margin-bottom:1.5rem}
        h1{font-size:1.75rem;font-weight:600;line-height:1.2;letter-spacing:-0.02em;color:#111;margin-bottom:1.25rem}
        </style>
        </head>
        <body>
        <main>
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="16 16 96 96" role="img" aria-label="FERB"><rect x="16" y="16" width="96" height="96" rx="22" fill="#111"/><path d="M49.969 58.772L49.969 90L60.477 90L60.477 58.772L67.729 58.772L67.729 51.742L60.477 51.742L60.477 49.448Q60.477 47.08 61.402 46.081Q62.327 45.082 64.473 45.082Q66.471 45.082 68.321 45.304L68.321 37.46Q66.989 37.386 65.583 37.275Q64.177 37.164 62.771 37.164Q56.333 37.164 53.151 40.42Q49.969 43.676 49.969 48.782L49.969 51.742L43.679 51.742L43.679 58.772Z" fill="#fff"/><circle cx="82" cy="85" r="8" fill="#F5A524"/></svg>
        <h1>sign in</h1>
        <p>\(text)</p>
        </main>
        </body>
        </html>

        """
    }
}
