import Foundation

/// FERB's own click-through windows (the pointer overlay on each display, the notch's glow), by window number, told to
/// the daemon whenever they change and again after each reconnection (`ui.click_through`, D31 fix round 4: H1, N1).
/// The daemon's click check skips exactly these windows of this app while it runs, so FERB's ordinary clicks under its
/// own overlay aren't refused. Only windows that never take a click belong here, never the notch's own panel.
///
/// Sends go out one at a time, each with the windows as they are when it goes, so the last thing the daemon hears is
/// the current set. A send that fails leaves the daemon's copy unknown: the next change or reconnection sends again.
@MainActor
public final class ClickThroughReport {
    private let rpc: DaemonRPCTransport
    private var sources: [String: Set<Int>] = [:]
    /// What the daemon holds for this app, as far as known (nil: unknown, after a failed send). It starts with nothing.
    private var told: [Int]? = []
    private var last: Task<Void, Never>?

    public init(rpc: DaemonRPCTransport) {
        self.rpc = rpc
    }

    /// Every source's windows together, in order.
    public var windows: [Int] {
        sources.values.reduce(into: Set<Int>()) { $0.formUnion($1) }.sorted()
    }

    /// One source's windows now ("cursor", "tracer"). A number of 0 or less (no window-server window yet) is left out.
    public func set(_ source: String, windows: [Int]) {
        let now = Set(windows.filter { $0 > 0 })
        guard sources[source] != now else { return }
        sources[source] = now
        schedule()
    }

    /// The daemon connection came back: it may have restarted and forgotten, so the windows are told again (with none
    /// and nothing told before, there is nothing to tell).
    public func reconnected() {
        guard !windows.isEmpty || told == nil else { return }
        told = nil
        schedule()
    }

    /// Sends again when the daemon's copy is unknown: a send failed (the daemon was restarting, or didn't list this app
    /// yet, just launched). The notch's reconciliation pass calls it every few seconds.
    public func retry() {
        guard told == nil else { return }
        schedule()
    }

    /// Waits for the sends already scheduled (tests).
    public func settled() async {
        await last?.value
    }

    private func schedule() {
        let previous = last
        last = Task { [weak self] in
            await previous?.value
            await self?.send()
        }
    }

    private func send() async {
        let now = windows
        guard now != told else { return }
        do {
            _ = try await rpc.call("ui.click_through", params: ["windows": now])
            told = now
        } catch {
            told = nil
        }
    }
}
