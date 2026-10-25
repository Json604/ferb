import AppKit
import SwiftUI
import NotchCore

/// Send to developer (L35): the report file FERB wrote about a failed task, as `reports.build` returns it. The address
/// is the daemon's (`[feedback] address`); the app never holds one of its own.
struct FailureReport: Equatable {
    static let note = "The attached file is everything FERB recorded about this task, with secrets removed. "
        + "Open it to see exactly what you're sending."

    let file: URL
    let subject: String
    let summary: String
    let address: String

    init?(_ reply: [String: Any]) {
        guard let path = reply["path"] as? String, path.hasPrefix("/"),
              let address = reply["address"] as? String, !address.isEmpty,
              let subject = reply["subject"] as? String else { return nil }
        file = URL(fileURLWithPath: path)
        self.subject = subject
        self.summary = reply["summary"] as? String ?? ""
        self.address = address
    }

    /// The email's body: FERB's short summary, then what the attached file is.
    var body: String { summary.isEmpty ? Self.note : summary + "\n\n" + Self.note }
}

/// Where a report stands under its answer.
enum ReportState: Equatable {
    case preparing
    /// A new email is open in the owner's Mail app; the owner presses Send.
    case composed
    /// No Mail account could take it: the file is shown in Finder, with the address to send it to.
    case revealed(address: String)
    /// FERB's reason, shown under the answer.
    case failed(String)

    var line: String? {
        switch self {
        case .preparing: return nil
        case .composed: return "A new email with the report is open in Mail."
        case .revealed(let address): return "Send this file to " + address
        case .failed(let reason): return reason
        }
    }
}

/// The owner's own Mail app. FERB only opens a new email; it never sends one.
protocol ReportMailing: AnyObject {
    /// A new email to `recipients` with `items` (the body text and the file); false when no compose service can
    /// take them. `failed` runs if the service fails after it started.
    func compose(recipients: [String], subject: String, items: [Any], failed: @escaping @MainActor () -> Void) -> Bool
    func reveal(_ file: URL)
}

final class SystemReportMailer: NSObject, ReportMailing, NSSharingServiceDelegate {
    private var sharing: [ObjectIdentifier: (service: NSSharingService, failed: @MainActor () -> Void)] = [:]

    func compose(recipients: [String], subject: String, items: [Any], failed: @escaping @MainActor () -> Void) -> Bool {
        guard let service = NSSharingService(named: .composeEmail) else { return false }
        service.recipients = recipients
        service.subject = subject
        guard service.canPerform(withItems: items) else { return false }
        service.delegate = self
        sharing[ObjectIdentifier(service)] = (service, failed)
        service.perform(withItems: items)
        return true
    }

    func reveal(_ file: URL) { NSWorkspace.shared.activateFileViewerSelecting([file]) }

    func sharingService(_ sharingService: NSSharingService, didShareItems items: [Any]) {
        sharing.removeValue(forKey: ObjectIdentifier(sharingService))
    }

    /// AppKit calls its sharing delegate on the main thread. The owner closing the email is no failure: nothing more
    /// opens then.
    func sharingService(_ sharingService: NSSharingService, didFailToShareItems items: [Any], error: Error) {
        guard let failed = sharing.removeValue(forKey: ObjectIdentifier(sharingService))?.failed else { return }
        let cancelled = (error as NSError).domain == NSCocoaErrorDomain && (error as NSError).code == NSUserCancelledError
        if !cancelled { MainActor.assumeIsolated { failed() } }
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

    /// The owner's click on Send to developer: FERB writes the report, then a new email opens in their Mail app with
    /// the file attached (their own foreground action, rule 17). Without Mail, the file is shown in Finder.
    func sendReport(for turn: ConversationTurn) {
        guard turn.offersReport, let id = turn.reportID, reports[turn.id] != .preparing else { return }
        let turnID = turn.id
        reports[turnID] = .preparing
        var params: [String: Any] = ["task_id": id]
        if let version = Self.appVersion { params["app_version"] = version }
        Task {
            do {
                let reply = try await rpc.call("reports.build", params: params, wait: Self.reportWait)
                guard let report = FailureReport(reply) else { throw DaemonRPCError.invalidResponse }
                let opened = reportMailer.compose(recipients: [report.address], subject: report.subject,
                                                  items: [report.body, report.file]) { [weak self] in
                    self?.revealReport(report, for: turnID)
                }
                if opened { reports[turnID] = .composed } else { revealReport(report, for: turnID) }
            } catch {
                reports[turnID] = .failed(error.localizedDescription)
            }
            onBackgroundChange?(true)
        }
    }

    func revealReport(_ report: FailureReport, for turnID: UUID) {
        reportMailer.reveal(report.file)
        reports[turnID] = .revealed(address: report.address)
        onBackgroundChange?(true)
    }
}

/// Send to developer under a failed, interrupted or planning-failure answer, and where its report stands.
struct ReportRow: View {
    let turn: ConversationTurn
    @ObservedObject var model: NotchModel

    var body: some View {
        let state = model.reports[turn.id]
        VStack(alignment: .leading, spacing: 4) {
            Button(state == .preparing ? "Preparing report…" : "Send to developer") { model.sendReport(for: turn) }
                .buttonStyle(FerbButtonStyle(quiet: true))
                .disabled(state == .preparing)
                .help("Opens a new email in Mail with everything FERB recorded about this task, secrets removed")
            if let line = state?.line {
                HStack(spacing: 6) {
                    Text(line).textSelection(.enabled)
                    if case .revealed(let address)? = state {
                        Button("Copy address") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(address, forType: .string)
                        }
                        .buttonStyle(FerbButtonStyle(quiet: true))
                    }
                }
                .font(.system(size: 10)).foregroundStyle(Palette.secondary)
            }
        }
    }
}
