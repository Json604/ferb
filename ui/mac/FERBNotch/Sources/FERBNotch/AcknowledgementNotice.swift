import Foundation
import NotchCore

/// Compact planning previews expire six seconds after their final text. Reminder cards remain until the owner
/// reaches for FERB. Turn ownership prevents an old abort/timer replacing newer content.
@MainActor
final class AcknowledgementNotice {
    private(set) var text: String?
    private(set) var turn: UUID?
    private(set) var reminderCard: ResultCard?
    var changed: ((String?) -> Void)?
    private let duration: Duration
    private var expiry: Task<Void, Never>?
    private var completedTurn: UUID?
    private var suppressed = Set<UUID>()
    private let sleep: @MainActor (Duration) async throws -> Void
    init(duration: Duration = .seconds(6),
         sleep: @escaping @MainActor (Duration) async throws -> Void = { try await Task.sleep(for: $0) }) {
        self.duration = duration
        self.sleep = sleep
    }

    func append(_ delta: String, turn: UUID) {
        guard reminderCard == nil, !delta.isEmpty, !suppressed.contains(turn), completedTurn != turn else { return }
        expiry?.cancel()
        expiry = nil
        if self.turn != turn {
            if let old = self.turn { suppressed.insert(old) }
            self.turn = turn
            completedTurn = nil
            text = ""
        }
        reminderCard = nil
        text = (text ?? "") + delta
        changed?(text)
    }

    func finish(_ text: String, turn: UUID, reminder: ResultCard? = nil) {
        // A planning preview cannot displace a reminder the owner has not reached for yet.
        guard reminderCard == nil || reminder != nil else { return }
        guard !suppressed.contains(turn), !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        if let old = self.turn, old != turn { suppressed.insert(old) }
        if self.turn != turn { completedTurn = nil }
        self.turn = turn
        self.text = text
        self.reminderCard = reminder
        changed?(text)
        complete(turn: turn)
    }

    func complete(turn: UUID) {
        guard self.turn == turn, !suppressed.contains(turn), completedTurn != turn else { return }
        completedTurn = turn
        expiry?.cancel()
        expiry = nil
        guard reminderCard == nil else { return }
        expiry = Task { [weak self, duration, sleep] in
            do { try await sleep(duration) } catch { return }
            guard self?.turn == turn else { return }
            self?.dismiss()
        }
    }

    func suppress(turn: UUID) {
        suppressed.insert(turn)
        if self.turn == turn { dismiss() }
    }
    func abort(turn: UUID) { suppress(turn: turn) }
    func show(_ text: String, reminder: ResultCard? = nil) { finish(text, turn: UUID(), reminder: reminder) }
    /// Moving Spaces, yielding and display changes dismiss only the transient planning preview.
    func dismissTransient() { if reminderCard == nil { dismiss() } }
    func dismiss() {
        expiry?.cancel()
        expiry = nil
        if let turn { suppressed.insert(turn) }
        turn = nil
        guard text != nil else { return }
        text = nil
        reminderCard = nil
        changed?(nil)
    }
    deinit { expiry?.cancel() }
}
