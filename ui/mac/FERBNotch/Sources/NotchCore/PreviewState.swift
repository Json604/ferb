import Foundation

/// One `tasks.preview` answer (notes/preview.md decision 2): what FERB is doing now (`state`, a short `line`), the
/// frame if there is one, and why a frame is old (`problem`). Pure, so the notch's preview loop is testable.
public struct PreviewAnswer: Equatable {
    /// working, working_elsewhere, asking, waiting, paused, done or none.
    public let state: String
    public let line: String
    /// The frame was captured for this request; false for the last frame kept (frozen, busy, a failed capture).
    public let fresh: Bool
    public let hasImage: Bool
    /// The step's task id the answer shows, and its step.
    public let worker: String?
    public let step: String?
    /// How the frame was masked when the app's own reading couldn't (OCR only).
    public let masking: String?
    /// withheld, gone, not_allowed, slow or failed, with the daemon's words.
    public let problemKind: String?
    public let problemText: String?
    /// The worker held the surface, so the last frame came back.
    public let busy: Bool

    public init(_ reply: [String: Any]) {
        func words(_ value: Any?) -> String? { (value as? String).flatMap { $0.isEmpty ? nil : $0 } }
        state = words(reply["state"]) ?? "none"
        line = reply["line"] as? String ?? ""
        fresh = reply["fresh"] as? Bool ?? false
        hasImage = words(reply["image"]) != nil
        worker = words(reply["worker"])
        step = words(reply["step"])
        masking = words(reply["masking"])
        let problem = reply["problem"] as? [String: Any]
        problemKind = words(problem?["kind"])
        problemText = words(problem?["text"])
        busy = reply["busy"] as? Bool ?? false
    }

    /// The line drawn over the frozen frame: whenever FERB isn't working in this window now, or the frame is old. A
    /// problem says why the frame is old.
    public var overlay: String? {
        guard state != "working" || !fresh else { return nil }
        if let problemText, state == "working" { return problemText }
        return line.isEmpty ? nil : line
    }

    /// Shown in place of a frame when the app has none.
    public var emptyText: String {
        if state == "none" { return line.isEmpty ? "No window to show yet" : line }
        return problemText ?? (line.isEmpty ? "Waiting for a visual task…" : line)
    }

    /// The app keeps its last frame unless the daemon has nothing at all to show (no surface yet).
    public var dropsLastFrame: Bool { state == "none" && !hasImage }

    /// When to ask again: a fresh frame asks at once (the rate is the capture's own, notes/preview.md decision 3: no
    /// cap until measured); the worker holding the surface is retried soon; a frozen or empty answer waits a second.
    public var nextRequestDelay: Duration {
        if fresh && hasImage { return .zero }
        if busy { return .milliseconds(200) }
        return .seconds(1)
    }
}

/// The daemon refused `tasks.preview` for this app: it isn't FERB.app (an app-only method), or the daemon predates
/// previews. Asking again every second changes nothing, so the preview hides until the daemon connects again. Read
/// from the daemon's error code (DaemonRPCError.remote carries it with the message, step 8e).
public enum PreviewRefusal: Equatable {
    case notThisApp, daemonTooOld

    /// From the daemon's error code (step 8e): -32005, a FERB.app-only method called by another program; -32601 (no
    /// such method) or -32001 (not allowed for this connection: a daemon from before the app-only code, or before the
    /// method), a daemon older than this app.
    public init?(code: Int) {
        switch code {
        case -32005: self = .notThisApp
        case -32601, -32001: self = .daemonTooOld
        default: return nil
        }
    }

    public var text: String {
        switch self {
        case .notThisApp: return "Live preview shows only in FERB.app"
        case .daemonTooOld: return "Live preview needs a FERB update"
        }
    }
}

/// Whether anyone could see a preview now (notes/preview.md fix list: capture only while someone can see it): not
/// while the displays sleep, the screen is locked or another user's session is in front. A covered window is the view's
/// own check (its window's occlusion).
public struct PreviewGate: Equatable {
    public var displaysAsleep = false
    public var screenLocked = false
    public var sessionInactive = false

    public init() {}

    public var mayCapture: Bool { !displaysAsleep && !screenLocked && !sessionInactive }
}
