import Foundation

/// What the packaged FERB.app runs at launch or from its menu (L27). `ferb setup` does the work and enforces where FERB
/// may be installed from; this only reads the installed LaunchAgent (~/Library/LaunchAgents/com.ferb.daemon.plist)
/// and whether launchd has it running.
public enum AppSetupAction: Equatable {
    case none                               // the agent runs this app's own build
    case install                            // nothing installed yet: `ferb setup`
    case update                             // this app, replaced by another build or not running: `ferb setup --update`
    case confirmReplace(current: String)    // another installation (a checkout, or a FERB.app elsewhere): ask first
}

public enum AppSetupPlan {
    public static func decide(agent: [String: Any]?, agentExecutable: String, build: String,
                              daemonRunning: Bool = true) -> AppSetupAction {
        guard let arguments = agent?["ProgramArguments"] as? [String], let program = arguments.first else {
            return .install
        }
        guard program == agentExecutable else { return .confirmReplace(current: program) }
        let environment = agent?["EnvironmentVariables"] as? [String: String] ?? [:]
        return environment["FERB_APP_BUILD"] == build && daemonRunning ? .none : .update
    }

    /// At launch nothing runs for `.none`; from the menu (Set Up FERB…) it is the full setup again.
    public static func arguments(for action: AppSetupAction, fromMenu: Bool = false) -> [String]? {
        switch action {
        case .none: return fromMenu ? ["setup"] : nil
        case .install, .confirmReplace: return ["setup"]
        case .update: return ["setup", "--update"]
        }
    }

    public static func needsConfirmation(_ action: AppSetupAction) -> Bool {
        if case .confirmReplace = action { return true }
        return false
    }
}

/// Turns a process's output into text as it arrives. A character split across two reads waits for its other
/// bytes; prompts that end without a newline appear at once.
public struct ConsoleDecoder {
    private var pending: [UInt8] = []

    public init() {}

    public mutating func append(_ data: Data) -> String {
        pending += data
        var cut = pending.count
        var index = pending.count - 1
        var continuation = 0
        while index >= 0 && continuation < 3 {
            let byte = pending[index]
            if byte & 0xC0 == 0x80 { index -= 1; continuation += 1; continue }
            let length = byte & 0xE0 == 0xC0 ? 2 : byte & 0xF0 == 0xE0 ? 3 : byte & 0xF8 == 0xF0 ? 4 : 1
            if length > pending.count - index { cut = index }
            break
        }
        let text = String(decoding: pending[..<cut], as: UTF8.self)
        pending = Array(pending[cut...])
        return text
    }
}
