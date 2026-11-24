#if FERB_PLAYGROUND
import Foundation
import NotchCore

/// The playground's temporary chat (TemporaryChat): `chat.temporary.get` and `.set`, `chat.send` with `fresh`, and
/// chat.status's `temporary`, shaped as the daemon's (the temporary chat's contract), in the playground's memory only.
/// Its replies are invented; nothing real runs.
@MainActor
final class PlaygroundTemporaryChat {
    private struct Chat {
        let id: String
        let startedAt: String
        var rev = 1
        var entries: [[String: Any]] = []
    }

    private var chat: Chat?
    /// Publishes a bus event to the app (`chat.temporary`).
    var emit: (String, [String: Any]) -> Void = { _, _ in }

    /// The reply of get and set: off, or the chat with its entries.
    private var reply: [String: Any] {
        guard let chat else { return ["on": false] }
        return ["on": true, "id": chat.id, "conversation": "temporary:" + chat.id, "started_at": chat.startedAt,
                "rev": chat.rev, "entries": chat.entries]
    }

    /// chat.status's `temporary`, only while one is on.
    var status: [String: Any]? {
        chat.map { ["id": $0.id, "rev": $0.rev, "started_at": $0.startedAt] }
    }

    func answer(_ method: String, _ params: [String: Any]) throws -> [String: Any]? {
        switch method {
        case "chat.temporary.get":
            return reply
        case "chat.temporary.set":
            guard let on = params["on"] as? Bool else {
                throw DaemonRPCError.remote("chat.temporary.set needs on (true or false)", code: -32602)
            }
            if on, chat == nil {
                chat = Chat(id: String(UUID().uuidString.prefix(8)).lowercased(), startedAt: Self.now())
                published()
            } else if !on, let ended = chat {
                chat = nil
                emit("chat.temporary", ["on": false, "id": ended.id, "rev": ended.rev + 1])
            }
            return reply
        default:
            return nil
        }
    }

    /// A message into the temporary chat: its ack, then an invented answer; each entry change is published.
    /// `client`: the sending app's id, kept on its message entry as the daemon keeps it.
    func send(_ text: String, client: String? = nil, pause: (Double) async -> Void,
              yield: ([String: Any]) -> Void) async throws {
        if chat == nil {
            chat = Chat(id: String(UUID().uuidString.prefix(8)).lowercased(), startedAt: Self.now())
        }
        guard let id = chat?.id else { return }
        let request = "playground-temp-" + UUID().uuidString.prefix(8)
        let message = add(kind: "message", request: request, client: client,
                          body: ["text": text, "attachments": [], "hidden": []])
        yield(["type": "ack", "request_id": request, "fresh": true, "conversation": "temporary:" + id, "at": Self.now()])
        await pause(1.0)
        try Task.checkCancellation()
        guard chat?.id == id else { return }       // ended meanwhile: its turns are forgotten
        let answer = "This is the playground's temporary chat: I started fresh, nothing real ran, and none of this is kept."
        yield(["type": "reply", "request_id": request, "text": answer, "status": "done", "at": Self.now()])
        add(kind: "answer", request: request, answers: message, body: ["text": answer, "status": "done"])
    }

    @discardableResult
    private func add(kind: String, request: String, answers: String? = nil, client: String? = nil,
                     body: [String: Any]) -> String {
        guard var current = chat else { return "" }
        let pos = current.entries.count + 1
        let id = "tmp-\(current.id)-\(pos)"
        let now = Self.now()
        var entry: [String: Any] = ["id": id, "conversation_id": "temporary:" + current.id, "pos": pos, "seq": pos,
                                    "rev": 1, "kind": kind, "state": "done", "open": false, "request_id": request,
                                    "created_at": now, "updated_at": now, "day": String(now.prefix(10)), "body": body,
                                    "deleted": false]
        if let answers { entry["answers"] = answers }
        if let client { entry["client_id"] = client }
        current.entries.append(entry)
        current.rev += 1
        chat = current
        published()
        return id
    }

    private func published() {
        guard let chat else { return }
        emit("chat.temporary", ["on": true, "id": chat.id, "rev": chat.rev])
    }

    private static func now() -> String { ISO8601DateFormatter().string(from: Date()) }
}
#endif
