@testable import FERBNotch
import Foundation

/// The owner's Mail app, faked: what a report or diagnostic would open, or, without Mail, show in Finder.
final class FakeMailer: ReportMailing {
    struct Mail {
        let recipients: [String]
        let subject: String
        let items: [Any]
        let failed: @MainActor () -> Void
    }
    let composes: Bool
    var composed: [Mail] = []
    var revealed: [URL] = []
    init(composes: Bool) { self.composes = composes }
    func compose(recipients: [String], subject: String, items: [Any], failed: @escaping @MainActor () -> Void) -> Bool {
        guard composes else { return false }
        composed.append(Mail(recipients: recipients, subject: subject, items: items, failed: failed))
        return true
    }
    func reveal(_ file: URL) { revealed.append(file) }
}
