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

/// APP-528's "another app is using the microphone", as a structural fact: the app responsible for each process running
/// microphone input. A process counts through the app it belongs to (itself, the app bundle holding it, or the
/// responsible or parent process that is one); a system daemon no app owns (Siri's always-on listener, corespeechd)
/// is not an app and never counts. FERB's own processes never count (APP-528: FERB's own dictation).
public enum MicrophoneUse {
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
