#if FERB_PLAYGROUND
import Foundation
import NotchCore

/// Sessions in the playground: the daemon's `sessions.*` answered with two invented coding sessions (Claude Code
/// working in Terminal, Codex in Visual Studio Code waiting on a permission question). Nothing reaches a terminal, an
/// agent or a settings file.
@MainActor
final class PlaygroundSessions {
    var emit: (String, [String: Any]) -> Void = { _, _ in }
    var sessions: [[String: Any]] = [
        ["id": "claude:sample-site", "agent": "claude_code", "cwd": "/Users/demo/code/site",
         "title": "Tidy the studio page styles", "status": "working", "waiting": "none",
         "host": ["bundle_id": "com.apple.Terminal", "name": "Terminal", "pid": 0, "via": "direct", "tty": "/dev/ttys004"],
         "send_paths": ["terminal_script"], "last_at": PlaygroundSamples.time(-40), "last_text": "Running the tests"],
        ["id": "codex:sample-notes", "agent": "codex", "cwd": "/Users/demo/code/notes-app", "title": "",
         "status": "waiting_for_you", "waiting": "permission",
         "host": ["bundle_id": "com.microsoft.VSCode", "name": "Visual Studio Code", "pid": 0, "via": "direct", "tty": "/dev/ttys007"],
         "send_paths": ["agent_api"], "last_at": PlaygroundSamples.time(-180), "last_text": "Wants to run npm install"]
    ]
    var items: [String: [[String: Any]]] = [
        "claude:sample-site": [
            ["seq": 1, "at": PlaygroundSamples.time(-600), "kind": "user", "text": "The studio page's spacing looks off on narrow screens. Can you tidy the styles?"],
            ["seq": 2, "at": PlaygroundSamples.time(-590), "kind": "assistant", "text": "I'll look at the page's stylesheet and the layout rules for small widths first."],
            ["seq": 3, "at": PlaygroundSamples.time(-580), "kind": "tool", "tool": "Read", "text": "src/styles/studio.css"],
            ["seq": 4, "at": PlaygroundSamples.time(-560), "kind": "tool", "tool": "Edit", "text": "src/styles/studio.css"],
            ["seq": 5, "at": PlaygroundSamples.time(-300), "kind": "assistant", "text": "The gutters now use one spacing scale, and the header no longer overflows at 360 px. Running the tests next."],
            ["seq": 6, "at": PlaygroundSamples.time(-40), "kind": "tool", "tool": "Bash", "text": "npm test"]],
        "codex:sample-notes": [
            ["seq": 1, "at": PlaygroundSamples.time(-900), "kind": "user", "text": "Add unit tests for the note search."],
            ["seq": 2, "at": PlaygroundSamples.time(-880), "kind": "assistant", "text": "The project has no test runner yet. I'll add vitest as a dev dependency and write the tests next to the search module."],
            ["seq": 3, "at": PlaygroundSamples.time(-200), "kind": "interrupted", "text": ""],
            ["seq": 4, "at": PlaygroundSamples.time(-190), "kind": "user", "text": "Go ahead, but keep it to dev dependencies."]]
    ]
    var cards: [String: [[String: Any]]] = [
        "codex:sample-notes": [["id": "sample-card", "session": "codex:sample-notes", "kind": "permission", "tool": "shell",
                                "detail": "npm install --save-dev vitest", "asked_at": PlaygroundSamples.time(-180),
                                "choices": [["id": "approved", "label": "Yes"],
                                            ["id": "approved_for_session", "label": "Yes, and don't ask again this session"],
                                            ["id": "denied", "label": "No"]]]]
    ]
    var started = 0

    func answer(_ method: String, _ params: [String: Any], enabled: Bool) throws -> [String: Any]? {
        let id = params["session"] as? String ?? ""
        let index = sessions.firstIndex { $0["id"] as? String == id }
        switch method {
        case "sessions.list":
            return ["enabled": enabled, "sessions": enabled ? sessions.map(Self.yours) : []]
        case "sessions.timeline":
            guard let index else { throw DaemonRPCError.remote("That session has ended.", code: SessionRefusal.sessionGone) }
            let all = items[id] ?? []
            let limit = (params["limit"] as? NSNumber)?.intValue ?? 50
            let before = (params["before"] as? NSNumber)?.intValue ?? 0, after = (params["after"] as? NSNumber)?.intValue ?? 0
            let seq = { (row: [String: Any]) in (row["seq"] as? Int) ?? 0 }
            let chosen: [[String: Any]]
            var more = false
            if before > 0 {
                let earlier = all.filter { seq($0) < before }
                chosen = Array(earlier.suffix(limit))
                more = earlier.count > limit
            } else if after > 0 {
                chosen = Array(all.filter { seq($0) > after }.prefix(limit))
            } else {
                chosen = Array(all.suffix(limit))
                more = all.count > limit
            }
            return ["session": Self.yours(sessions[index]), "items": chosen, "more": more,
                    "cards": before > 0 ? [] : (cards[id] ?? [])]
        case "sessions.send", "sessions.control":
            guard let index else { throw DaemonRPCError.remote("That session has ended.", code: SessionRefusal.sessionGone) }
            let text = (params["text"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if method == "sessions.control" { sessions[index]["control"] = "mac" }
            else if sessions[index]["control"] as? String != "mac" {
                let row = sessions[index]
                let host = (row["host"] as? [String: Any])?["name"] as? String ?? "its app"
                return ["needs_yes": ["question": "Send this to \(AgentSessionRow.agentName(row["agent"] as? String ?? "")) in \(host)?",
                                      "detail": text,
                                      "covers": "Yes lets you send to this session from here until you stop controlling it."]]
            }
            guard !text.isEmpty else {
                guard method == "sessions.control" else {
                    throw DaemonRPCError.remote("There's nothing to send.", code: SessionRefusal.sendFailed)
                }
                emit("sessions.changed", ["count": sessions.count])
                return ["control": "mac"]
            }
            if !(cards[id] ?? []).isEmpty {
                let agent = AgentSessionRow.agentName(sessions[index]["agent"] as? String ?? "")
                throw DaemonRPCError.remote("\(agent) is asking something. Answer that first.", code: SessionRefusal.cardOpen)
            }
            append(id, kind: "user", text: text)
            sessions[index]["status"] = "working"
            sessions[index]["last_text"] = text
            emit("sessions.changed", ["count": sessions.count])
            var reply: [String: Any] = ["sent": true, "path": (sessions[index]["send_paths"] as? [String])?.first ?? "",
                                        "delivered": true, "seconds": 0.4]
            if method == "sessions.control" { reply["control"] = "mac" }
            return reply
        case "sessions.release":
            if let index { sessions[index]["control"] = nil }
            emit("sessions.changed", ["count": sessions.count])
            return ["released": index == nil ? 0 : 1]
        case "sessions.answer":
            guard let index else { throw DaemonRPCError.remote("That session has ended.", code: SessionRefusal.sessionGone) }
            let card = (cards[id] ?? []).first { $0["id"] as? String == params["card"] as? String }
            guard let card else { throw DaemonRPCError.remote("That question was already answered.", code: SessionRefusal.cardGone) }
            cards[id] = []
            let yes = (params["choice"] as? String ?? "").hasPrefix("approved")
            sessions[index]["waiting"] = "none"
            sessions[index]["status"] = "working"
            if yes { append(id, kind: "tool", text: card["detail"] as? String ?? "", tool: card["tool"] as? String) }
            else { append(id, kind: "assistant", text: "Understood, I won't run that. Tell me how you'd like to go on.") }
            emit("sessions.card_gone", ["session": id, "card": card["id"] ?? ""])
            return ["answered": true]
        case "sessions.interrupt":
            guard let index else { throw DaemonRPCError.remote("That session has ended.", code: SessionRefusal.sessionGone) }
            guard sessions[index]["control"] as? String == "mac" else {
                throw DaemonRPCError.remote("Say Yes to sending to this session first.", code: SessionRefusal.notTheSession)
            }
            append(id, kind: "interrupted", text: "")
            sessions[index]["status"] = "idle"
            emit("sessions.changed", ["count": sessions.count])
            return ["interrupted": true]
        case "sessions.prepare":
            var reply: [String: Any] = ["folders": ["/Users/demo/code/site", "/Users/demo/code/notes-app"],
                                        "agents": ["claude_code", "codex"], "host": "Terminal"]
            if let folder = params["folder"] as? String, !folder.isEmpty {
                let agent = params["agent"] as? String ?? "claude_code"
                let program = agent == "codex" ? "codex" : "claude"
                let first = params["text"] as? String ?? ""
                reply["needs_yes"] = ["question": "Start \(AgentSessionRow.agentName(agent)) in a new Terminal window?",
                                      "detail": "cd '\(folder)' && \(program)" + (first.isEmpty ? "" : " '\(first)'"),
                                      "covers": "FERB types this command into a new Terminal window, in the background."]
            }
            return reply
        case "sessions.start":
            started += 1
            let agent = params["agent"] as? String ?? "claude_code"
            let folder = params["folder"] as? String ?? "/Users/demo/code/site"
            let new = "\(agent == "codex" ? "codex" : "claude"):sample-new-\(started)"
            sessions.insert(["id": new, "agent": agent, "cwd": folder, "title": "", "status": "idle", "waiting": "none",
                             "host": ["bundle_id": "com.apple.Terminal", "name": "Terminal", "pid": 0, "via": "direct", "tty": ""],
                             "send_paths": ["terminal_script"], "last_at": PlaygroundSamples.time(), "last_text": ""], at: 0)
            if let first = params["text"] as? String, !first.isEmpty { items[new] = [["seq": 1, "at": PlaygroundSamples.time(), "kind": "user", "text": first]] }
            emit("sessions.changed", ["count": sessions.count])
            return ["started": true, "command": "cd '\(folder)' && \(agent == "codex" ? "codex" : "claude")"]
        case "sessions.delegate":
            guard let index else { throw DaemonRPCError.remote("That session has ended.", code: SessionRefusal.sessionGone) }
            sessions[index]["delegate"] = "sample-delegate"
            sessions[index]["goal"] = params["goal"] as? String ?? ""
            emit("sessions.changed", ["count": sessions.count])
            return ["handed_over": true, "message": "FERB is on it. You'll see it in the chat."]
        case "sessions.remove_hooks":
            return ["removed": true]
        default:
            return nil
        }
    }

    /// The line a session's Watch shows over its sample frame.
    func watchLine(_ id: String) -> String {
        let host = (sessions.first { $0["id"] as? String == id }?["host"] as? [String: Any])?["name"] as? String
        return host.map { "Sample window of \($0)" } ?? "No window to show yet"
    }

    private func append(_ id: String, kind: String, text: String, tool: String? = nil) {
        let next = ((items[id] ?? []).compactMap { $0["seq"] as? Int }.max() ?? 0) + 1
        var row: [String: Any] = ["seq": next, "at": PlaygroundSamples.time(), "kind": kind, "text": text]
        if let tool { row["tool"] = tool }
        items[id, default: []].append(row)
    }

    private static func yours(_ row: [String: Any]) -> [String: Any] {
        row.merging(["yours": row["control"] as? String == "mac"]) { $1 }
    }
}
#endif
