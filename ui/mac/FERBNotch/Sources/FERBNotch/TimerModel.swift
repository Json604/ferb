import Foundation
import NotchCore

/// The owner's timers as FERB's records give them (APP-762): what is running, cancelling one from its card, and the
/// done alert's Stop and Repeat. Every change goes through the daemon (`reminders.list`, `reminders.cancel`,
/// `timers.repeat`); nothing here starts a timer by itself.
@MainActor
extension NotchModel {
    /// Reads the running timers again (connect, a timer set or fired, a cancel, a repeat).
    func refreshRunningTimers() async {
        guard let reply = try? await rpc.call("reminders.list", params: [:]),
              let rows = reply["pending"] as? [[String: Any]] else { return }
        let timers = rows.compactMap(RunningTimer.init(row:))
        if timers != runningTimers { runningTimers = timers }
    }

    /// The timer's card's Cancel: the daemon cancels it; the card then says Cancelled.
    func cancelTimer(_ id: String) {
        Task {
            guard (try? await rpc.call("reminders.cancel", params: ["id": id])) != nil else { return }
            cancelledTimers.insert(id)
            runningTimers.removeAll { $0.id == id }
            await refreshRunningTimers()
        }
    }

    /// The done alert's Stop: the alert goes, the chat keeps its message (APP-622).
    func stopTimerAlert() {
        acknowledgement.dismiss()
    }

    /// The done alert's Repeat: the same timer again for the same duration (`timers.repeat`), then the alert goes.
    func repeatTimer(_ id: String) {
        acknowledgement.dismiss()
        Task {
            guard let reply = try? await rpc.call("timers.repeat", params: ["id": id]) else { return }
            if let again = RunningTimer(row: reply.merging(["id": reply["timer"] ?? ""]) { $1 }) {
                runningTimers.removeAll { $0.id == again.id }
                runningTimers.append(again)
            }
            await refreshRunningTimers()
        }
    }

    /// The timer the closed notch shows now: the soonest still running.
    func soonestTimer(now: Date = Date()) -> RunningTimer? { RunningTimer.soonest(runningTimers, now: now) }
}
