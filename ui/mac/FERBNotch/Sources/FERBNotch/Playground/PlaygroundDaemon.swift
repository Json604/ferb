#if FERB_PLAYGROUND
import AppKit
import NotchCore

/// The fake daemon behind the playground: the same `DaemonRPCTransport` seam FERB.app's `DaemonRPC` fills, answering
/// what the intro, the setup model and the notch ask, with replies shaped as the real daemon's (ferb/daemon/areas/
/// intro.py, ferb/device/permissions.py) and waits scaled by the playground's speed. No socket, no file but its state.
final class PlaygroundDaemon: DaemonRPCTransport, @unchecked Sendable {
    let state: PlaygroundState

    @MainActor lazy var workspace = PlaygroundWorkspace(state: state)

    init(state: PlaygroundState) { self.state = state }

    /// Simulated tasks the owner stopped (`task.stop`): their stream ends stopped at its next step.
    @MainActor private var stopped: Set<String> = []

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        try await answer(method, params)
    }

    func call(_ method: String, params: [String: Any], wait seconds: Int) async throws -> [String: Any] {
        try await answer(method, params)
    }

    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { continuation in
            let listenerID = UUID()
            let run = Task { @MainActor [weak self] in
                guard let self else { return continuation.finish() }
                guard !Task.isCancelled else { continuation.finish(); return }
                if method == "logs.subscribe" {
                    self.workspace.listeners[listenerID] = continuation
                    return
                }
                do {
                    try await self.events(method, params) { continuation.yield($0) }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { [weak self] _ in
                run.cancel()
                Task { @MainActor in self?.workspace.listeners.removeValue(forKey: listenerID) }
            }
        }
    }

    // MARK: Calls

    /// Calls the playground answers silently (the notch's own housekeeping, every few seconds).
    private static let quiet: Set<String> = ["chat.status", "ui.click_through", "about.guide", "memory.learning",
                                             "results.ack", "attention.ack", "runtimes.status", "setup.permissions",
                                             "setup.persona", "setup.browsers", "ping", "chat.attach"]

    @MainActor
    private func answer(_ method: String, _ params: [String: Any]) async throws -> [String: Any] {
        if !Self.quiet.contains(method) { state.note("daemon ← \(method) \(Self.describe(params))") }
        if let reply = try workspace.answer(method, params) { return reply }
        var s = state.saved
        switch method {
        case "ping", "auth.hello":
            return ["pid": 0, "role": "app", "stopped": false]
        case "chat.attach":
            return ["session_id": "playground", "capabilities": ["input_paths"]]
        case "chat.status":
            return ["paused": s.paused, "stopped": false, "running": [[String: Any]](), "results": [[String: Any]](),
                    "notices": [[String: Any]](), "approvals": [[String: Any]]()]
        case "sense.pause", "sense.resume":
            state.saved.paused = method == "sense.pause"
            return ["ok": true]

        // Sign in
        case "runtimes.status":
            return runtimes()
        case "runtimes.use":
            state.saved.runtime = params["runtime"] as? String
            return ["ok": true]
        case "runtimes.check":
            await state.pause(1.2)
            return s.signInFails ? ["ok": false, "detail": "the playground's sign-in is set to fail"] : ["ok": true]
        case "runtimes.sign_in":
            return await claudeSignIn(params)
        case "runtimes.models":            // invented ids: the playground reaches no provider
            let runtime = params["runtime"] as? String ?? "codex"
            return ["runtime": runtime, "model": "sample-model-1", "models": ["sample-model-1", "sample-model-2"],
                    "roles": [String: Any]()]
        case "runtimes.model":
            await state.pause(0.6)
            return ["ok": true, "model": params["model"] as? String ?? "", "detail": "answered"]

        // Permissions
        case "setup.permissions":
            return permissions()
        case "setup.permission_request":
            guard let id = params["permission"] as? String else { return ["ok": false] }
            if SetupModel.answeredInPopup.contains(id) {
                _ = await state.ask(id)                  // macOS's own popup (Paste from Other Apps)
            } else {
                await state.pause(0.4)                   // macOS adds FERB to the pane's list; the pane opens next
            }
            return ["ok": true]
        case "automation.request":
            return ["ok": true]
        case "setup.finish":
            s.setupDone = state.allGranted && s.runtime != nil
            state.saved.setupDone = s.setupDone
            return ["ok": true, "setup_done": s.setupDone]

        // Getting to know you (the Mac scan Try mentions)
        case "setup.persona":
            return persona(start: params.isEmpty)

        // Browsers and FERB's own account
        case "setup.browsers":
            return browsers()
        case "setup.browsers_pair_all":
            await state.pause(2.5)
            if s.pairingFails {
                return ["profiles": 2, "results": [["name": "Personal", "label": "Google Chrome",
                                                    "email": Self.personalEmail, "paired": true]],
                        "skipped": [["name": "Work", "label": "Google Chrome", "email": Self.workEmail, "reason": "off",
                                     "step": "turn FERB on in Google Chrome's extensions"]]]
            }
            state.saved.browsersPaired = true
            return ["profiles": 2, "skipped": [[String: Any]](),
                    "results": [["name": "Personal", "email": Self.personalEmail, "paired": true],
                                ["name": "Work", "email": Self.workEmail, "paired": true]]]
        case "task.stop":
            stopped.formUnion(params["task_ids"] as? [String] ?? [])
            return ["stopped": params["task_ids"] ?? []]
        case "setup.browser_pair":
            await state.pause(1.5)
            return ["paired": !s.pairingFails, "why": "FERB isn't turned on in that profile"]
        case "setup.browser_open", "settings.set":
            return ["ok": true]
        case "identity.find":
            return identity(address: params["address"] as? String)
        case "identity.choose":
            if params["clear"] as? Bool == true {
                state.saved.identityAddress = ""
                state.saved.identityPaired = false
            } else {
                state.saved.identityAddress = params["address"] as? String ?? ""
            }
            return ["ok": true]
        case "identity.pair":
            await state.pause(2.0)
            state.saved.identityPaired = !s.pairingFails
            return s.pairingFails ? ["paired": false, "hint": "turn FERB's extension on in that profile"]
                : ["paired": true]

        // Try
        case "intro.suggest":
            switch s.suggest {
            case .normal: await state.pause(2.5)
            case .slow: await state.pause(12)
            case .fails:
                await state.pause(2.0)
                throw DaemonRPCError.remote("The playground's suggestions are set to fail.")
            }
            return ["suggestions": Self.suggestions.map { ["request": $0.request, "app": $0.app] },
                    "fits": s.scanSeesSomething]
        default:
            throw DaemonRPCError.remote("This operation is not simulated in the playground: " + method)
        }
    }

    // MARK: Streams

    @MainActor
    private func events(_ method: String, _ params: [String: Any], yield: ([String: Any]) -> Void) async throws {
        switch method {
        case "intro.preview":
            let request = params["request"] as? String ?? ""
            state.note("daemon ← intro.preview “\(request.prefix(40))…”")
            switch state.saved.preview {
            case .normal: await state.pause(3.5)          // the planner's turn
            case .slow: await state.pause(14)
            case .fails:
                await state.pause(2.5)
                throw DaemonRPCError.remote("FERB couldn't show that plan: the planner didn't answer (the playground's " +
                                            "preview is set to fail).")
            }
            try Task.checkCancellation()
            let plan = Self.plan(for: request)
            let id = "preview-" + UUID().uuidString.prefix(8)
            yield(["type": "plan", "preview": true, "request_id": id, "text": plan.line, "plan_line": plan.line,
                   "goal": request, "identity": "owner", "total": plan.steps.count])
            for (index, label) in plan.steps.enumerated() {
                await state.pause(0.08)
                yield(["type": "part", "preview": true, "request_id": id, "part": "\(index + 1)", "step": "\(index + 1)",
                       "kind": "do", "label": label, "state": "preview",
                       "depends_on": index == 0 ? [] : ["\(index)"]])
            }
            yield(["type": "done", "preview": true, "request_id": id])
        case "chat.send":
            if workspace.enabled {
                try await workspace.chat(params["text"] as? String ?? "", yield: yield)
                return
            }
            // The tour's Try task, its stop practice, or anything typed in the notch: a simulated task whose
            // steps tick by; nothing real runs in the playground. task.stop ends it stopped.
            let request = params["text"] as? String ?? ""
            let id = "playground-" + UUID().uuidString.prefix(8)
            let task = "task-" + id
            state.note("daemon ← chat.send (a simulated task; nothing real runs)")
            yield(["type": "ack", "request_id": id, "at": Self.now(), "text": "On it."])
            await state.pause(1.0)
            let plan = Self.plan(for: request)
            yield(["type": "plan", "request_id": id, "task_id": task, "at": Self.now(), "text": plan.line,
                   "plan_line": plan.line, "goal": request, "identity": "owner", "total": plan.steps.count])
            for (index, label) in plan.steps.enumerated() {
                let row: [String: Any] = ["type": "part", "request_id": id, "task_id": task, "part": "\(index + 1)",
                                          "step": "\(index + 1)", "kind": "do", "label": label, "at": Self.now()]
                yield(row.merging(["state": "working"]) { $1 })
                await state.pause(2.0)
                try Task.checkCancellation()
                if stopped.contains(task) {
                    let text = "Stopped. This is the intro playground: nothing real ran."
                    yield(["type": "outcome", "request_id": id, "task_id": task, "status": "stopped",
                           "answer": text, "at": Self.now()])
                    return
                }
                yield(row.merging(["state": "done"]) { $1 })
            }
            let text = "This is the intro playground: I went through the steps, but nothing real ran."
            yield(["type": "outcome", "request_id": id, "task_id": task, "status": "done", "answer": text,
                   "at": Self.now()])
        case "setup.browsers_pairing":
            let fails = state.saved.pairingFails
            let todo: [[String: Any]] = [["browser": "chrome", "folder": "Default", "name": "Personal",
                                          "email": Self.personalEmail, "label": "Google Chrome"],
                                         ["browser": "chrome", "folder": "Profile 1", "name": "Work",
                                          "email": Self.workEmail, "label": "Google Chrome"],
                                         ["browser": "chrome", "folder": "Profile 3", "name": "Family", "email": "",
                                          "label": "Google Chrome"]]
            let skipped: [[String: Any]] = fails ? [["browser": "brave", "folder": "Default", "name": "Personal",
                                                     "email": Self.personalEmail,
                                                     "label": "Brave", "reason": "removed", "can_add": true,
                                                     "step": "FERB's extension was removed there, so Brave won't add it again: add FERB from its Chrome Web Store page in Brave"]] : []
            yield(["type": "plan", "profiles": todo.count + skipped.count, "todo": todo, "skipped": skipped])
            var results: [[String: Any]] = []
            for (index, row) in todo.enumerated() {
                await state.pause(1.5)
                let paired = !(fails && index == 1)
                let result = row.merging(["paired": paired, "reason": paired ? "" : "off",
                                          "why": paired ? "" : "turn FERB on in Google Chrome's extensions"]) { $1 }
                results.append(result)
                yield(result.merging(["type": "result"]) { $1 })
            }
            if !fails { state.saved.browsersPaired = true }
            yield(["type": "done", "profiles": todo.count + skipped.count, "skipped": skipped, "results": results,
                   "paired": results.filter { $0["paired"] as? Bool == true }.count])
        default:
            // logs.subscribe, notifications.subscribe: open and quiet until the app leaves
            while !Task.isCancelled { try await Task.sleep(for: .seconds(3600)) }
        }
    }

    // MARK: Replies

    static let suggestions: [IntroSuggestion] = [
        IntroSuggestion(request: "Find three things on my Mac I could tidy up and tell me why, without changing anything.", app: "Finder"),
        IntroSuggestion(request: "Compare three well-reviewed electric kettles on the web and tell me which is the best value.",
                        app: "")]

    static func plan(for request: String) -> (line: String, steps: [String]) {
        if request.localizedCaseInsensitiveContains("kettle") {
            return ("I'll look up well-reviewed electric kettles, compare three on price and reviews, and tell you " +
                    "which is the best value.",
                    ["Search the web for well-reviewed electric kettles", "Read reviews and prices for the top three",
                     "Compare them and pick the best value"])
        }
        if request.localizedCaseInsensitiveContains("tidy") {
            return ("I'll look through Downloads and your Desktop for things you could tidy up and show you how I'd " +
                    "do it, without moving anything.",
                    ["Look through Downloads and the Desktop", "Find large, old or duplicate files",
                     "Suggest three things to tidy, with how I'd do each"])
        }
        return ("I'll work out the steps for that and show you the plan.",
                ["Understand what you asked", "Find what it needs on your Mac", "Do it and check the result"])
    }

    @MainActor
    private func runtimes() -> [String: Any] {
        let s = state.saved
        let current = s.runtime
        var claude: [String: Any] = ["name": "claude", "usable": current == "claude", "signed_in": current == "claude",
                                     "model": "claude"]
        if let sign = s.claudeSignIn {
            claude["sign_in"] = ["state": sign, "url": "https://claude.ai/playground-sign-in",
                                 "why": sign == "failed" ? "The playground's sign-in is set to fail." : ""]
        }
        var rows: [[String: Any]] = [["name": "codex", "usable": current == "codex", "signed_in": current == "codex",
                                      "model": "gpt", "fix": current == "codex" ? "" : "Sign in with ChatGPT"], claude]
        for provider in APIProvider.allCases {
            rows.append(["name": provider.rawValue, "usable": current == provider.rawValue, "model": provider.rawValue])
        }
        let runtime = current ?? "codex"
        return ["roles": ["planner": runtime, "worker": runtime], "runtimes": rows]
    }

    @MainActor
    private func claudeSignIn(_ params: [String: Any]) async -> [String: Any] {
        if params["cancel"] as? Bool == true {
            state.saved.claudeSignIn = nil
            return ["state": "idle"]
        }
        if params["code"] != nil {
            state.saved.claudeSignIn = state.saved.signInFails ? "failed" : "signed_in"
            return ["state": state.saved.claudeSignIn ?? "idle"]
        }
        state.saved.claudeSignIn = "waiting"
        Task { @MainActor [state] in
            await state.pause(4)
            guard state.saved.claudeSignIn == "waiting" else { return }
            state.saved.claudeSignIn = state.saved.signInFails ? "failed" : "signed_in"
            state.note("Claude sign-in in the browser: \(state.saved.claudeSignIn ?? "")")
        }
        return ["state": "waiting", "url": "https://claude.ai/playground-sign-in"]
    }

    @MainActor
    private func permissions() -> [String: Any] {
        let pane = "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?"
        let anchors = ["accessibility": "Privacy_Accessibility", "screen_recording": "Privacy_ScreenCapture",
                       "full_disk_access": "Privacy_AllFiles", "input_monitoring": "Privacy_ListenEvent",
                       "clipboard": "Privacy_Pasteboard"]
        let requestable: Set<String> = ["accessibility", "screen_recording", "input_monitoring", "clipboard"]
        let rows: [[String: Any]] = PlaygroundState.permissions
            .filter { !PlaygroundState.appPermissions.contains($0) }
            .map { id in
                let status = state.grant(id)
                return ["id": id, "status": status.rawValue, "process": "daemon", "required": true,
                        "pane": pane + (anchors[id] ?? ""),
                        "requestable": requestable.contains(id) && (id != "clipboard" || status == .notDetermined),
                        "restart_to_check": PlaygroundState.restartToCheck.contains(id), "blocked": ""]
            }
        return ["rows": rows, "automation": [[String: Any]](), "automation_pane": pane + "Privacy_Automation",
                // The orb's drag card carries the playground app's own icon (FERB's agent app isn't installed here).
                "grant_target": Bundle.main.bundlePath,
                "app_panes": ["notifications": "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=" +
                                PlaygroundMode.bundleID,
                              "microphone": pane + "Privacy_Microphone",
                              "speech_recognition": pane + "Privacy_SpeechRecognition"],
                "setup_done": state.saved.setupDone]
    }

    @MainActor
    private func persona(start: Bool) -> [String: Any] {
        if start && state.saved.persona == "not_started" {
            state.saved.persona = "scanning"
            Task { @MainActor [state] in
                await state.pause(6)
                state.saved.persona = state.saved.scanSeesSomething ? "seen" : "finished"
            }
        }
        if state.saved.persona == "seen" {
            return ["state": "seen", "summary": "You spend most of your day in a code editor, a browser and a design " +
                    "tool, and your Downloads folder is getting full."]
        }
        return ["state": state.saved.persona]
    }

    // Invented accounts (HAND-424): an ordinary one, a long one that shows the middle shortening, a profile that isn't
    // signed in (Family) and FERB's own.
    static let personalEmail = "sam.rivera@example.com"
    static let workEmail = "samantha.rivera-longname.operations@studio-production.example.com"
    static let ferbEmail = "ferb.assistant@example.com"

    @MainActor
    private func browsers() -> [String: Any] {
        let paired = state.saved.browsersPaired
        func profile(_ folder: String, _ name: String, _ email: String, _ before: String) -> [String: Any] {
            ["folder": folder, "name": name, "email": email, "state": paired ? "paired" : before, "ferb_identity": false,
             "connected": paired]
        }
        var profiles = [profile("Default", "Personal", Self.personalEmail, "on"),
                        profile("Profile 1", "Work", Self.workEmail, "installed"),
                        profile("Profile 3", "Family", "", "on")]
        if state.saved.identityPaired {
            profiles.append(["folder": "Profile 2", "name": "FERB", "email": Self.ferbEmail, "state": "paired",
                             "ferb_identity": true, "connected": true])
        }
        return ["store_url": "https://chromewebstore.google.com/", "extension_dir": "",
                "browsers": [["browser": "chrome", "label": "Google Chrome", "requested": true, "manual": false,
                              "unconfirmed": "", "profiles": profiles]]]
    }

    @MainActor
    private func identity(address: String?) -> [String: Any] {
        let s = state.saved
        var reply: [String: Any] = ["current": ["address": s.identityAddress, "browser": s.identityAddress.isEmpty ? "" : "chrome",
                                                "profile": s.identityAddress.isEmpty ? "" : "Profile 2",
                                                "profile_name": s.identityAddress.isEmpty ? "" : "FERB",
                                                "profile_email": s.identityAddress.isEmpty ? "" : Self.ferbEmail,
                                                "paired": s.identityPaired, "owner_profile": ""]]
        if let address, !address.isEmpty {
            reply["candidates"] = address.contains("@")
                ? [["browser": "chrome", "browser_name": "Google Chrome", "folder": "Profile 2", "name": "FERB",
                    "email": address.lowercased(), "only": true, "other_accounts": 0]]
                : [[String: Any]]()
        }
        return reply
    }

    private static func describe(_ params: [String: Any]) -> String {
        // Never logs what was typed into a secure field (there is none here, but the rule is FERB's: no secrets in logs)
        let keys = params.keys.sorted().filter { !["code", "key", "text", "request"].contains($0) }
        return keys.isEmpty ? "" : "{" + keys.map { "\($0): \(params[$0] ?? "")" }.joined(separator: ", ") + "}"
    }

    private static func now() -> String { ISO8601DateFormatter().string(from: Date()) }
}
#endif
