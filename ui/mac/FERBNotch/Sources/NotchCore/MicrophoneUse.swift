import Foundation

/// What the system says about one process, read when a sound is about to play: whether it is an app the owner runs
/// (a running application that isn't background-only), the outermost app bundle its executable lives in, the process
/// macOS holds responsible for it and its parent.
public struct ProcessFacts: Equatable, Sendable {
    /// The app's name when the process itself is an app (regular or menu-bar), else nil.
    public var appName: String?
    /// The name of the outermost `.app` bundle holding the process's executable (a browser's helper lives inside the
    /// browser), else nil.
    public var bundleApp: String?
    /// The process macOS holds responsible for this one (an XPC service's app), when known.
    public var responsible: pid_t?
    /// The parent process, when known.
    public var parent: pid_t?

    public init(appName: String? = nil, bundleApp: String? = nil, responsible: pid_t? = nil, parent: pid_t? = nil) {
        self.appName = appName
        self.bundleApp = bundleApp
        self.responsible = responsible
        self.parent = parent
    }
}

/// What one of a process's input devices is, from Core Audio's own facts (its transport type, an aggregate's
/// sub-devices).
public enum InputSource: Equatable, Sendable {
    /// A device that hears the room: built-in, USB, Bluetooth, Continuity, or an aggregate holding one of those.
    case microphone
    /// A virtual driver (a loopback, an audio relay, a meeting app's virtual device) or an aggregate of only those:
    /// what it carries is other apps' sound, not the room.
    case virtual
    /// Core Audio can't say (a device another process keeps private).
    case unknown
}

/// One process running audio input: its pid and the input devices Core Audio names for it (none for a process tap,
/// which records other apps' output rather than a device).
public struct InputUse: Equatable, Sendable {
    public var pid: pid_t
    public var sources: [InputSource]

    public init(pid: pid_t, sources: [InputSource]) {
        self.pid = pid
        self.sources = sources
    }
}

/// APP-528's "another app is using the microphone", as a structural fact: the app responsible for each process running
/// microphone input. A process counts through the app it belongs to (itself, the app bundle holding it, or the
/// responsible or parent process that is one); a system daemon no app owns (Siri's always-on listener, corespeechd)
/// is not an app and never counts. FERB's own processes never count (APP-528: FERB's own dictation).
public enum MicrophoneUse {
    /// The processes among `inputs` whose input is a microphone (APP-908): one of its devices is a microphone; or
    /// Core Audio names none or can't say what one is, and a microphone runs input now without FERB's own dictation
    /// to explain it. A process whose input runs only on virtual devices or on a tap of other apps' sound (an audio
    /// relay, a loopback, a recorder of the Mac's own output) isn't using the microphone: on 2026-11-14 an audio
    /// relay's always-running tap silenced every sound of the owner's first setup.
    public static func microphoneInputs(_ inputs: [InputUse], microphoneRunning: Bool,
                                        isOwn: (pid_t) -> Bool) -> [pid_t] {
        let ownDictation = inputs.contains { isOwn($0.pid) }
        return inputs.filter { use in
            if use.sources.contains(.microphone) { return true }
            if !use.sources.isEmpty && use.sources.allSatisfy({ $0 == .virtual }) { return false }
            return microphoneRunning && !ownDictation
        }.map(\.pid)
    }

    /// The first app other than FERB that holds the microphone through one of `inputs`, by name; nil when none does.
    public static func callingApp(inputs: [pid_t], isOwn: (pid_t) -> Bool,
                                  facts: (pid_t) -> ProcessFacts?) -> String? {
        for pid in inputs where !isOwn(pid) {
            if let app = owningApp(of: pid, isOwn: isOwn, facts: facts) { return app }
        }
        return nil
    }

    /// The app `pid` belongs to: itself when it is an app, else the app bundle holding it, else the first app up its
    /// responsible and parent chain. Nil when the chain reaches launchd (or a process it can't read) without an app,
    /// or reaches one of FERB's own processes.
    public static func owningApp(of pid: pid_t, isOwn: (pid_t) -> Bool, facts: (pid_t) -> ProcessFacts?) -> String? {
        var current = pid
        var seen: Set<pid_t> = []
        while current > 1, seen.insert(current).inserted {
            if current != pid, isOwn(current) { return nil }
            guard let info = facts(current) else { return nil }
            if let app = info.appName ?? info.bundleApp { return app }
            if let responsible = info.responsible, responsible != current, !seen.contains(responsible) {
                current = responsible
            } else if let parent = info.parent {
                current = parent
            } else {
                return nil
            }
        }
        return nil
    }
}
