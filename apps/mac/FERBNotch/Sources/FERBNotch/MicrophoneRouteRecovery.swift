import Foundation

/// Bluetooth input/output can emit several format changes while switching modes.
/// Rebuild once the route settles, and never after the user releases or cancels.
@MainActor
final class MicrophoneRouteRecovery {
    private var pending: Task<Void, Never>?

    func schedule(_ rebuild: @escaping @MainActor () -> Void) {
        cancel()
        pending = Task {
            do { try await Task.sleep(for: .milliseconds(250)) } catch { return }
            guard !Task.isCancelled else { return }
            rebuild()
        }
    }

    func cancel() {
        pending?.cancel()
        pending = nil
    }
}
