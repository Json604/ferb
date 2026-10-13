import Foundation
import Darwin

/// Owner-authenticated local transport. Request bodies (including secrets) are never logged or persisted.
public protocol DaemonRPCTransport {
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any]
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error>
}

public final class DaemonRPC: DaemonRPCTransport, @unchecked Sendable {
    public let socketPath: String
    public init(socketPath: String = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".ferb/run/ferb.sock").path) {
        self.socketPath = socketPath
    }

    public func call(_ method: String, params: [String: Any] = [:]) async throws -> [String: Any] {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async { [socketPath] in
                do {
                    let connection = try UnixRPCConnection(path: socketPath)
                    defer { connection.close() }
                    continuation.resume(returning: try connection.call(method, params: params))
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    public func stream(_ method: String, params: [String: Any] = [:]) -> AsyncThrowingStream<[String: Any], Error> {
        let cancellation = StreamCancellation()
        return AsyncThrowingStream { continuation in
            continuation.onTermination = { _ in cancellation.cancel() }
            DispatchQueue.global(qos: .utility).async { [socketPath] in
                do {
                    let connection = try UnixRPCConnection(path: socketPath)
                    defer { connection.close() }
                    cancellation.attach(connection)
                    try connection.stream(method, params: params) { continuation.yield($0) }
                    continuation.finish()
                } catch { continuation.finish(throwing: error) }
            }
        }
    }
}

private final class StreamCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var connection: UnixRPCConnection?
    private var cancelled = false
    func attach(_ connection: UnixRPCConnection) {
        lock.lock(); defer { lock.unlock() }
        self.connection = connection
        if cancelled { connection.interrupt() }
    }
    func cancel() {
        lock.lock(); defer { lock.unlock() }
        cancelled = true
        connection?.interrupt()
    }
}

public enum DaemonRPCError: LocalizedError {
    case unavailable, disconnected, invalidResponse, pathTooLong, remote(String)
    public var errorDescription: String? {
        switch self {
        case .unavailable: return "FERB is not running. Run ferb setup --update, then reconnect."
        case .disconnected: return "Connection to FERB was interrupted. Tasks already started keep running."
        case .invalidResponse: return "FERB returned an invalid response."
        case .pathTooLong: return "The FERB socket path is too long."
        case .remote(let message): return message
        }
    }
}

private final class UnixRPCConnection: @unchecked Sendable {
    private var descriptor: Int32
    private var nextID = 0
    private var buffered = RPCLineBuffer()
    private let lock = NSLock()

    init(path: String) throws {
        descriptor = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { throw DaemonRPCError.unavailable }
        do {
            var noSignal: Int32 = 1
            setsockopt(descriptor, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
            var timeout = timeval(tv_sec: 10, tv_usec: 0)
            setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
            setsockopt(descriptor, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
            var address = sockaddr_un()
            address.sun_family = sa_family_t(AF_UNIX)
            let bytes = Array(path.utf8CString).map { UInt8(bitPattern: $0) }
            guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw DaemonRPCError.pathTooLong }
            withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
            address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
            let result = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else { throw DaemonRPCError.unavailable }
            _ = try call("auth.hello", params: ["role": "cli"])
        } catch { Darwin.close(descriptor); descriptor = -1; throw error }
    }

    func close() {
        lock.lock(); defer { lock.unlock() }
        if descriptor >= 0 { Darwin.close(descriptor); descriptor = -1 }
    }
    func interrupt() {
        lock.lock(); defer { lock.unlock() }
        if descriptor >= 0 { Darwin.shutdown(descriptor, SHUT_RDWR) }
    }
    func call(_ method: String, params: [String: Any]) throws -> [String: Any] {
        let id = try send(method, params: params)
        while true {
            let response = try read()
            guard response["id"] as? Int == id else { continue }
            return try result(response)
        }
    }
    func stream(_ method: String, params: [String: Any], receive: ([String: Any]) -> Void) throws {
        var timeout = timeval(tv_sec: 0, tv_usec: 0)
        setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        let id = try send(method, params: params)
        while true {
            let response = try read()
            if response["method"] as? String == method + ".event", let event = response["params"] as? [String: Any] {
                receive(event)
            } else if response["id"] as? Int == id {
                _ = try result(response)
                return
            }
        }
    }
    private func send(_ method: String, params: [String: Any]) throws -> Int {
        nextID += 1
        let bytes = try JSONSerialization.data(withJSONObject: ["jsonrpc": "2.0", "id": nextID, "method": method, "params": params]) + Data([10])
        try bytes.withUnsafeBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let count = Darwin.send(descriptor, buffer.baseAddress!.advanced(by: offset), buffer.count - offset, 0)
                if count < 0 && errno == EINTR { continue }
                guard count > 0 else { throw DaemonRPCError.disconnected }
                offset += count
            }
        }
        return nextID
    }
    private func read() throws -> [String: Any] {
        while true {
            if let line = buffered.nextLine() {
                let value = try JSONSerialization.jsonObject(with: line)
                guard let object = value as? [String: Any] else { throw DaemonRPCError.invalidResponse }
                return object
            }
            var bytes = [UInt8](repeating: 0, count: 8192)
            let count = Darwin.recv(descriptor, &bytes, bytes.count, 0)
            if count < 0 && errno == EINTR { continue }
            guard count > 0 else { throw DaemonRPCError.disconnected }
            buffered.append(Data(bytes.prefix(count)))
        }
    }
    private func result(_ response: [String: Any]) throws -> [String: Any] {
        if let error = response["error"] as? [String: Any] {
            throw DaemonRPCError.remote(error["message"] as? String ?? "FERB could not complete this request.")
        }
        guard let result = response["result"] as? [String: Any] else { throw DaemonRPCError.invalidResponse }
        return result
    }
}

/// Newline framing without rescanning an incomplete response after each socket read.
/// Offsets are relative to startIndex because Data may retain nonzero slice indices.
struct RPCLineBuffer {
    private var bytes = Data()
    private(set) var searchOffset = 0

    mutating func append(_ fragment: Data) {
        bytes.append(fragment)
    }

    mutating func nextLine() -> Data? {
        let searchStart = bytes.index(bytes.startIndex, offsetBy: searchOffset)
        guard let newline = bytes[searchStart...].firstIndex(of: 10) else {
            searchOffset = bytes.count
            return nil
        }
        let line = Data(bytes[..<newline])
        bytes.removeSubrange(..<bytes.index(after: newline))
        searchOffset = 0
        return line
    }
}
