import Foundation
import NotchCore

/// The owner's timers as FERB's records give them (APP-762, DMN-548): what is running, what happened to each timer a
/// card shows (running, cancelled, fired, repeated), cancelling one from its card, and the done alert's Stop and Repeat.
/// Every change goes through the daemon (`reminders.list`, `timers.states`, `reminders.cancel`, `timers.repeat`) and
/// comes back as `timer.changed`, whichever path made it (a card, chat, Settings); nothing here starts a timer by
/// itself, and no view keeps a timer's state of its own.
@MainActor
extension NotchModel {
    /// Connect: the running timers and the state of every timer the app has shown come from FERB's rows, not memory
    /// (a cancel or a firing while the app was away is told here).
    func refreshTimers() async {
        await refreshRunningTimers()
        let known = Set(timerRecords.keys).union(askedTimerStates).union(runningTimers.map(\.id))
        await loadTimerStates(Array(known))
    }

    /// Reads the running timers again (connect, a timer set, a repeat).
    func refreshRunningTimers() async {
        guard let reply = try? await rpc.call("reminders.list", params: [:]),
              let rows = reply["pending"] as? [[String: Any]] else { return }
        let timers = rows.compactMap(RunningTimer.init(row:))
        if timers != runningTimers { runningTimers = timers }
    }

    /// What FERB's rows say of these timers (`timers.states`). An older daemon without it leaves the records as they are.
    func loadTimerStates(_ ids: [String]) async {
        let ids = Array(Set(ids.filter { !$0.isEmpty })).sorted()
        guard !ids.isEmpty else { return }
        askedTimerStates.formUnion(ids)
        guard let reply = try? await rpc.call("timers.states", params: ["ids": ids]),
              let rows = reply["timers"] as? [String: Any] else { return }
        for (id, value) in rows {
            if let row = value as? [String: Any], let record = TimerRecord(row: row), timerRecords[id] != record {
                timerRecords[id] = record
            }
        }
    }

    /// A timer's card or alert showed: its state is read once from FERB's rows (connect reads every one again).
    func needTimerState(_ id: String?) {
        guard let id, !id.isEmpty, timerRecords[id] == nil, !askedTimerStates.contains(id) else { return }
        askedTimerStates.insert(id)
        Task { await loadTimerStates([id]) }
    }

    /// `timer.changed` (DMN-548): a timer was set, cancelled (its card, chat, Settings) or fired. Its cards and the
    /// closed notch follow at once; a repeat marks the timer it repeats as repeated.
    func receiveTimerChange(_ data: [String: Any]) {
        guard let id = data["timer_id"] as? String, !id.isEmpty, ["timer", "reminder"].contains(data["kind"] as? String),
              let state = TimerRecord.State(rawValue: data["state"] as? String ?? "") else { return }
        let record = TimerRecord(state: state, repeatedBy: timerRecords[id]?.repeatedBy)
        if timerRecords[id] != record { timerRecords[id] = record }
        if let original = data["repeat_of"] as? String, !original.isEmpty {
            timerRecords[original] = TimerRecord(state: .fired, repeatedBy: id)
        }
        if state == .running, let timer = RunningTimer(row: data.merging(["id": id]) { $1 }) {
            if !runningTimers.contains(timer) {
                runningTimers.removeAll { $0.id == id }
                runningTimers.append(timer)
            }
        } else if runningTimers.contains(where: { $0.id == id }) {
            runningTimers.removeAll { $0.id == id }
        }
    }

    /// A timer fired (its `timer.fired`, or its arrival): it isn't running, and its alert may offer Stop and Repeat.
    func timerFired(_ id: String?) {
        guard let id, !id.isEmpty else { return }
        runningTimers.removeAll { $0.id == id }
        if timerRecords[id]?.state != .fired { timerRecords[id] = TimerRecord(state: .fired) }
    }

    /// The timer's card's Cancel: the daemon cancels it; the card then says Cancelled (and `timer.changed` tells the
    /// rest).
    func cancelTimer(_ id: String) {
        Task {
            guard (try? await rpc.call("reminders.cancel", params: ["id": id])) != nil else { return }
            receiveTimerChange(["timer_id": id, "kind": "timer", "state": TimerRecord.State.cancelled.rawValue])
        }
    }

    /// Whether a fired timer's alert still offers Stop and Repeat: FERB's row says it fired and it wasn't repeated, and
    /// the owner hasn't stopped it here. The arrival and the timer's card in chat read the same answer.
    func timerAlertOpen(_ id: String?) -> Bool {
        guard let id, let record = timerRecords[id] else { return false }
        return record.state == .fired && record.repeatedBy == nil && !stoppedTimers.contains(id)
    }

    /// Dismisses the arrival showing on the notch only when it is this timer's or reminder's (`timer_id`).
    func dismissArrival(of id: String?) {
        guard let shown = acknowledgement.reminderCard, shown.timerID == id else { return }
        acknowledgement.dismiss()
    }

    /// The done alert's Stop (and a reminder's Done): that timer's arrival goes, the chat keeps its message (APP-622).
    func stopTimerAlert(_ id: String?) {
        if let id { stoppedTimers.insert(id) }
        dismissArrival(of: id)
    }

    /// The done alert's Repeat: the same timer again for the same duration (`timers.repeat`, at most once: a second
    /// Repeat gets the timer the first started), then that timer's arrival goes.
    func repeatTimer(_ id: String) {
        stoppedTimers.insert(id)
        dismissArrival(of: id)
        Task {
            guard let reply = try? await rpc.call("timers.repeat", params: ["id": id]),
                  let again = reply["timer"] as? String, !again.isEmpty else { return }
            timerRecords[id] = TimerRecord(state: .fired, repeatedBy: again)
            let state = reply["state"] as? String ?? TimerRecord.State.running.rawValue   // an older daemon: running
            receiveTimerChange(reply.merging(["timer_id": again, "kind": "timer", "state": state]) { $1 })
            await refreshRunningTimers()
        }
    }

    /// The timer the closed notch shows now: the soonest still running.
    func soonestTimer(now: Date = Date()) -> RunningTimer? { RunningTimer.soonest(runningTimers, now: now) }
}
