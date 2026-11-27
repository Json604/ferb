#if FERB_PLAYGROUND
import Foundation
import NotchCore

/// The playground's chat entries (ENG-8, ENG-14), shaped as the engine's: `chat.attach` says it keeps them, every
/// message sent and what FERB says to it is an entry, `chat.entries.watch` brings each change to the app, and a message
/// sent with `reply_to` keeps the quote on its entry as the engine does ({entry_id, kind, who, excerpt}). Invented, in
/// the playground's memory only: it lets the app's reply arrows and quotes run as they do against FERB.
@MainActor
final class PlaygroundEntries {
    static let epoch = "playground-epoch"
    private var entries: [[String: Any]] = []
    private var seq = 0
    /// Each request's message entry, and the client that sent it.
    private var messages: [String: String] = [:]
    private var clients: [String: String] = [:]
    private var watchers: [UUID: ([String: Any]) -> Void] = [:]

    /// A watch: the changes after the app's cursor, `caught_up`, then each change as it is made, until the app leaves.
    func watch(_ params: [String: Any], yield: @escaping ([String: Any]) -> Void) async {
        let after = (params["epoch"] as? String) == Self.epoch ? (params["after_seq"] as? NSNumber)?.intValue ?? 0 : seq
        for entry in entries where (entry["seq"] as? Int ?? 0) > after {
            yield(["type": "change", "entry": entry, "catchup": true])
        }
        yield(["type": "caught_up", "epoch": Self.epoch, "head": seq])
        let id = UUID()
        watchers[id] = yield
        while !Task.isCancelled { try? await Task.sleep(for: .seconds(3600)) }
        watchers.removeValue(forKey: id)
    }

    /// One frame of a message's stream, as the engine keeps it: the ack makes the message's entry (and says its id),
    /// the plan and what FERB says make theirs. Returns the frame the app gets.
    func frame(_ frame: [String: Any], params: [String: Any]) -> [String: Any] {
        guard let request = frame["request_id"] as? String, let type = frame["type"] as? String else { return frame }
        switch type {
        case "ack":
            let client = params["client_id"] as? String
            clients[request] = client
            var body: [String: Any] = ["text": params["text"] as? String ?? "", "attachments": params["input_paths"] ?? []]
            if let quoted = (params["reply_to"] as? String).flatMap(quote) { body["reply_to"] = quoted }
            let id = add(kind: "message", request: request, body: body)
            messages[request] = id
            var acked = frame
            acked["entry_id"] = id
            acked["seq"] = seq
            acked["epoch"] = Self.epoch
            return acked
        case "plan":
            var body: [String: Any] = ["text": frame["text"] ?? ""]
            if let line = frame["plan_line"] { body["plan_line"] = line }
            add(kind: "plan", request: request, body: body)
        case "reply", "clarify":
            answer(request: request, text: frame["text"] as? String ?? "", status: frame["status"] as? String ?? "done")
        case "outcome":
            answer(request: request, text: frame["answer"] as? String ?? "", status: frame["status"] as? String ?? "done")
        default: break
        }
        return frame
    }

    /// FERB's answer to `request` (a reply, or a task's end told on the bus).
    func answer(request: String, text: String, status: String) {
        guard messages[request] != nil, !text.isEmpty else { return }
        add(kind: "answer", request: request, body: ["text": text, "status": status])
    }

    /// The engine's `reply_to` of a quoted entry: who said it and its first 160 characters; nil for an unknown entry
    /// (the message is still sent, without the quote).
    private func quote(_ id: String) -> [String: Any]? {
        guard let entry = entries.last(where: { $0["id"] as? String == id }), let kind = entry["kind"] as? String,
              let body = entry["body"] as? [String: Any] else { return nil }
        let text = (body["text"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? body["plan_line"] as? String ?? ""
        return ["entry_id": id, "kind": kind, "who": kind == "message" ? "owner" : "ferb", "excerpt": String(text.prefix(160))]
    }

    @discardableResult
    private func add(kind: String, request: String, body: [String: Any]) -> String {
        seq += 1
        let id = "sample-entry-\(seq)"
        let now = PlaygroundSamples.time()
        var entry: [String: Any] = ["id": id, "conversation_id": "playground", "pos": seq, "seq": seq, "rev": 1,
                                    "kind": kind, "state": "done", "open": false, "request_id": request,
                                    "created_at": now, "updated_at": now, "day": String(now.prefix(10)), "body": body,
                                    "deleted": false]
        if kind != "message", let message = messages[request] { entry["answers"] = message }
        if let client = clients[request] { entry["client_id"] = client }
        entries.append(entry)
        for watcher in watchers.values { watcher(["type": "change", "entry": entry]) }
        return id
    }
}
#endif
