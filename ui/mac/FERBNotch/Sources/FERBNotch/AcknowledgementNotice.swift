import Foundation

/// Provisional pre-worker text lives only in memory. Its lifetime starts at final text,
/// not the first token. Turn ownership prevents an old abort/timer replacing a new preview.
@MainActor
final class AcknowledgementNotice {
    private(set) var text: String?
    private(set) var turn: UUID?
    var changed: ((String?) -> Void)?
    private let duration: Duration
    private var expiry: Task<Void, Never>?
    private var completedTurn: UUID?
    private var suppressed = Set<UUID>()
    init(duration: Duration = .seconds(3)) { self.duration = duration }

    func append(_ delta: String, turn: UUID) {
        guard !delta.isEmpty, !suppressed.contains(turn), completedTurn != turn else { return }
        expiry?.cancel()
        expiry = nil
        if self.turn != turn {
            if let old = self.turn { suppressed.insert(old) }
            self.turn = turn
            completedTurn = nil
            text = ""
        }
        text = (text ?? "") + delta
        changed?(text)
    }

    func finish(_ text: String, turn: UUID) {
        guard !suppressed.contains(turn), !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        if let old = self.turn, old != turn { suppressed.insert(old) }
        if self.turn != turn { completedTurn = nil }
        self.turn = turn
        self.text = text
        changed?(text)
        complete(turn: turn)
    }

    func complete(turn: UUID) {
        guard self.turn == turn, !suppressed.contains(turn), completedTurn != turn else { return }
        completedTurn = turn
        expiry?.cancel()
        expiry = Task { [weak self, duration] in
            do { try await Task.sleep(for: duration) } catch { return }
            guard self?.turn == turn else { return }
            self?.dismiss()
        }
    }

    func suppress(turn: UUID) {
        suppressed.insert(turn)
        if self.turn == turn { dismiss() }
    }
    func abort(turn: UUID) { suppress(turn: turn) }
    func show(_ text: String) { finish(text, turn: UUID()) }
    func dismiss() {
        expiry?.cancel()
        expiry = nil
        if let turn { suppressed.insert(turn) }
        turn = nil
        guard text != nil else { return }
        text = nil
        changed?(nil)
    }
    deinit { expiry?.cancel() }
}
