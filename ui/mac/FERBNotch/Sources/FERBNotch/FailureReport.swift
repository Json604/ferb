import AppKit
import SwiftUI
import NotchCore

/// Send to developer (L35): the report file FERB wrote about a failed task (or the diagnostic file), as
/// `reports.build` / `reports.diagnostic` return it. The owner views it, then Send has the daemon post that same file to
/// the developer's report relay (owner decision 2026-11-01); the app never holds the relay's address.
struct FailureReport: Equatable {
    let file: URL
    let subject: String
    let summary: String

    init?(_ reply: [String: Any]) {
        guard let path = reply["path"] as? String, path.hasPrefix("/"),
              let subject = reply["subject"] as? String else { return nil }
        file = URL(fileURLWithPath: path)
        self.subject = subject
        self.summary = reply["summary"] as? String ?? ""
    }
}

/// Where a report stands under its button.
enum ReportState: Equatable {
    case preparing
    /// Written, with secrets removed: the owner can view it, then press Send.
    case ready(FailureReport)
    case sending(FailureReport)
    case sent
    /// Not sent (not set up, offline, the relay refused): the file stays saved and Send again tries once more.
    case unsent(FailureReport, reason: String)
    /// FERB couldn't write the report: its reason; the button asks again.
    case failed(String)

    var report: FailureReport? {
        switch self {
        case .ready(let report), .sending(let report), .unsent(let report, _): return report
        case .preparing, .sent, .failed: return nil
        }
    }

    var isUnsent: Bool {
        if case .unsent = self { return true }
        return false
    }

    var line: String? {
        switch self {
        case .preparing: return nil
        case .ready: return "Ready to send, with secrets removed. View it to see exactly what goes."
        case .sending: return "Sending…"
        case .sent: return "Sent"
        case .unsent(_, let reason): return "Couldn't send: " + reason + " The report is saved; you can send it again."
        case .failed(let reason): return reason
        }
    }
}

/// Send and Send again: the daemon posts the file the owner viewed (`reports.send`); it never retries by itself.
@MainActor
enum ReportSender {
    /// Seconds the app waits for the relay's answer: a report of up to 25 MB goes up on the owner's connection (the
    /// daemon gives up on its own after 540 s and says so).
    static let sendWait = 600

    static func send(_ report: FailureReport, appVersion: String?, rpc: DaemonRPCTransport) async -> ReportState {
        var params: [String: Any] = ["path": report.file.path, "title": report.subject]
        if let appVersion { params["app_version"] = appVersion }
        do {
            let reply = try await rpc.call("reports.send", params: params, wait: sendWait)
            if reply["sent"] as? Bool == true { return .sent }
            return .unsent(report, reason: reply["error"] as? String ?? "FERB gave no reason.")
        } catch {
            return .unsent(report, reason: error.localizedDescription)
        }
    }
}

@MainActor
extension NotchModel {
    /// How long the notch waits for a report (seconds): a long task's whole record is rendered and redacted first.
    static let reportWait = 120

    /// The notch's version, sent with the report.
    static var appVersion: String? {
        guard let short = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String else { return nil }
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String
        return build.map { short + " (" + $0 + ")" } ?? short
    }

    /// The owner's click on Send to developer: FERB writes the report (secrets removed) and the row offers View and
    /// Send. Nothing leaves the Mac until the owner presses Send.
    func sendReport(for turn: ConversationTurn) {
        guard turn.offersReport else { return }
        switch reports[turn.id] {
        case nil, .failed?: break
        default: return                       // being prepared, or already prepared or sent
        }
        let turnID = turn.id
        reports[turnID] = .preparing
        // Both ids the turn holds: the daemon applies the same rule to whichever recorded the failure (L35).
        var params: [String: Any] = [:]
        if let task = turn.backendTaskID { params["task_id"] = task }
        if let request = turn.requestID { params["request_id"] = request }
        if let version = Self.appVersion { params["app_version"] = version }
        Task {
            do {
                let reply = try await rpc.call("reports.build", params: params, wait: Self.reportWait)
                guard let report = FailureReport(reply) else { throw DaemonRPCError.invalidResponse }
                reports[turnID] = .ready(report)
            } catch {
                reports[turnID] = .failed(error.localizedDescription)
            }
            onBackgroundChange?(true)
        }
    }

    /// Send, or Send again after it couldn't: one try, its answer shown under the button.
    func sendBuiltReport(for turnID: UUID) {
        guard let report = reports[turnID]?.report, reports[turnID] != .sending(report) else { return }
        reports[turnID] = .sending(report)
        Task {
            reports[turnID] = await ReportSender.send(report, appVersion: Self.appVersion, rpc: rpc)
            onBackgroundChange?(true)
        }
    }
}

/// Send to developer under a failed, interrupted or planning-failure answer, and where its report stands.
struct ReportRow: View {
    let turn: ConversationTurn
    @ObservedObject var model: NotchModel

    var body: some View {
        let state = model.reports[turn.id]
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                switch state {
                case .ready(let report), .unsent(let report, _), .sending(let report):
                    Button("View report") { model.openReportFile(report.file) }
                        .buttonStyle(FerbButtonStyle(quiet: true))
                    Button(state?.isUnsent == true ? "Send again" : "Send") { model.sendBuiltReport(for: turn.id) }
                        .buttonStyle(FerbButtonStyle(quiet: true))
                        .disabled(state == .sending(report))
                        .help("Sends this report to FERB's developer")
                case .sent:
                    EmptyView()
                case .preparing, .failed, nil:
                    Button(state == .preparing ? "Preparing report…" : "Send to developer") {
                        model.sendReport(for: turn)
                    }
                    .buttonStyle(FerbButtonStyle(quiet: true))
                    .disabled(state == .preparing)
                    .help("Prepares everything FERB recorded about this task, secrets removed, for you to view and send")
                }
            }
            if let line = state?.line {
                Text(line).textSelection(.enabled)
                    .font(.system(size: 10)).foregroundStyle(Palette.secondary)
            }
        }
    }
}
