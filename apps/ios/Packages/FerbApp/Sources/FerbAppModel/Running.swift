import FerbDesign
import FerbMobile
import Foundation
import Observation
import UIKit

// The work row above the message box (kb PHN-725; Android: model/Running.kt), FERB.app's "N active tasks" row
// (ChatBody.swift WorkRowView, TaskActivityBubble.swift): what runs, from the chat's live state, and its round pictures.

/// One running task as the work row and its short view show it.
public struct RunTask: Equatable, Sendable {
    public var entry: String, taskId: String?, title: String, line: String, waiting: Bool, paused: Bool
    public init(entry: String, taskId: String?, title: String, line: String, waiting: Bool, paused: Bool) {
        self.entry = entry; self.taskId = taskId; self.title = title; self.line = line; self.waiting = waiting; self.paused = paused
    }
}

/// The work row: its words, whether anything runs, since when, and the tasks with pictures.
public struct RunningUi: Equatable, Sendable {
    public var words: WorkWords, running: Bool, since: Date?, tasks: [RunTask]
    public init(words: WorkWords, running: Bool, since: Date?, tasks: [RunTask]) {
        self.words = words; self.running = running; self.since = since; self.tasks = tasks
    }
}

public enum RunningModel {
    /// FERB.app's work row from the chat's live state: a turn that became a task runs; one waiting on the owner's answer
    /// to go on is paused, one only waiting for a time is kept going (L31). Nil when nothing runs, waits or is kept.
    ///
    /// The tasks this chat doesn't show (a schedule's run) come from the Mac's live status (`status.running`, pushed by
    /// the link whenever it changes), never from the Tasks tab's last `tasks.list` (`mac`): that list is read again only
    /// while the Tasks tab shows, so a task that was running when it was read stayed "1 active task" after it finished
    /// (kb PHN-792, the owner's report of 2026-11-21). `mac` only says whether such a task is paused or waits for him.
    public static func of(_ view: TranscriptView?, status: PhoneStatus? = nil, mac: [TaskRow]? = nil) -> RunningUi? {
        if view == nil && status == nil { return nil }
        var turns: [String: TurnItem] = [:]
        for item in view?.items ?? [] { if case .turn(let t) = item { turns[t.entry] = t } }
        var tasks: [RunTask] = []
        var active = 0, paused = 0, kept = 0
        var since: Double?
        for l in view?.live ?? [] {
            guard let line = l.current, let t = turns[l.entry] else { continue }
            switch l.state {
            case "paused": paused += 1
            case "sleeping": kept += 1
            default:
                active += 1
                if let at = t.planAt { since = min(since ?? at, at) }
            }
            let title = WorkModel.title(t)
            if l.state != "sleeping" {
                tasks.append(RunTask(entry: l.entry, taskId: t.taskId ?? t.card?.taskId ?? t.checklist?.taskId, title: title.isEmpty ? "A task from earlier" : title,
                                     line: line, waiting: l.state == "needs_you", paused: l.state == "paused"))
            }
        }
        // FERB.app counts every running task, not only this chat's: the Mac's live status adds the ones started elsewhere
        let here = Set(tasks.compactMap(\.taskId))
        let asking = Set((status?.questions ?? []).compactMap(\.taskId))
        for r in status?.running ?? [] where !r.taskId.isEmpty && !here.contains(r.taskId) {
            let listed = mac?.first { $0.taskId == r.taskId }?.status
            let isPaused = listed == "paused"
            if isPaused { paused += 1 } else { active += 1 }
            let title = WorkModel.oneLine(r.name.isEmpty ? r.goal : r.name)
            tasks.append(RunTask(entry: "mac:\(r.taskId)", taskId: r.taskId, title: title.isEmpty ? "A task from earlier" : title,
                                 line: "", waiting: listed == "needs_user" || asking.contains(r.taskId), paused: isPaused))
        }
        guard active + paused + kept > 0 else { return nil }
        return RunningUi(words: .of(active: active, paused: paused, kept: kept), running: active > 0,
                         since: since.map { Date(timeIntervalSinceReferenceDate: $0) }, tasks: tasks)
    }
}

/// The work row's round live pictures (APP-403, APP-900) under the preview's rules (PHN-454, PHN-444): only while the
/// row is on screen and the app unlocked and in front (the model says when), never on the shared relay (the library
/// refuses it), at most one picture every `everyMs` per task. Each frame is decoded small, in memory only; screenshots
/// and captures as the owner's switch says (kb PHN-810).
@MainActor
@Observable
public final class Thumbnails {
    public private(set) var pictures: [String: UIImage] = [:]
    @ObservationIgnored private let api: PhoneApi
    @ObservationIgnored private let everyMs: UInt64
    @ObservationIgnored private var running: [String: Live?] = [:]
    @ObservationIgnored private var generation = 0
    public static let everyMs: UInt64 = 2000

    public init(api: PhoneApi, everyMs: UInt64 = Thumbnails.everyMs) { self.api = api; self.everyMs = everyMs }

    /// The most pictures asked for at once (the work row's two and the card rows on screen): a bound on what they cost.
    public static let most = 4

    /// A picture's key: the task's (the step that acted last), or one step's (`worker`, a card row's step task id).
    public static func key(_ taskId: String, _ worker: String? = nil) -> String { worker.map { taskId + "\u{1F}" + $0 } ?? taskId }

    /// Watches exactly `keys` (at most `most`); an empty list stops everything and drops every picture.
    public func show(_ keys: [String]) {
        let want = Set(keys.prefix(Self.most))
        for (id, l) in running where !want.contains(id) { l?.stop(); running[id] = nil }
        pictures = pictures.filter { want.contains($0.key) }
        if want.isEmpty { generation += 1; return }
        let gen = generation
        for id in want where running[id] == nil {
            running[id] = .some(nil)
            let sink = Sink(self, id: id, generation: gen)
            Task {
                let parts = id.split(separator: "\u{1F}", maxSplits: 1).map(String.init)
                let asked = parts.count == 2
                    ? await api.watchStepThumbnail(taskId: parts[0], worker: parts[1], everyMs: everyMs, sink: sink)
                    : await api.watchThumbnail(taskId: id, everyMs: everyMs, sink: sink)
                switch asked {
                case .ok(let l): if gen == generation, running[id] != nil { running[id] = l } else { l.stop() }
                case .failed: running[id] = nil
                }
            }
        }
    }

    fileprivate func frame(_ id: String, _ gen: Int, _ f: PreviewFrame) {
        guard gen == generation, running[id] != nil else { return }
        if let jpeg = f.jpeg { if let p = Self.small(jpeg) { pictures[id] = p } } else if f.gone { pictures[id] = nil }
    }

    fileprivate func ended(_ id: String) { running[id] = nil }

    /// Decoded to about 128 points at most: the round picture is 30.
    public static func small(_ jpeg: Data) -> UIImage? {
        guard let image = UIImage(data: jpeg) else { return nil }
        let side: CGFloat = 128
        let scale = min(1, side / max(image.size.width, image.size.height))
        if scale >= 1 { return image }
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        return UIGraphicsImageRenderer(size: size).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
    }

    private final class Sink: PreviewSink, @unchecked Sendable {
        private weak var owner: Thumbnails?
        private let id: String, generation: Int
        init(_ owner: Thumbnails, id: String, generation: Int) { self.owner = owner; self.id = id; self.generation = generation }
        func frame(frame: PreviewFrame) { let (o, i, g) = (owner, id, generation); Task { @MainActor in o?.frame(i, g, frame) } }
        func ended(code: String?, detail: String?) { let (o, i) = (owner, id); Task { @MainActor in o?.ended(i) } }
    }
}
