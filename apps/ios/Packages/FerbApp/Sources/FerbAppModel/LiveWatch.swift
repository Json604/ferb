import FerbCore
import FerbMobile
import Foundation
import Observation

/// What the Watch live screen shows: the last frame (JPEG bytes in memory; the screen decodes them), FERB.app's state
/// and line over it, or why it ended. (Android's LiveView; named apart from the library's LiveView.)
public struct LiveScreen: Equatable, Sendable {
    public let taskId: String
    public var frame: Data? = nil
    public var state: String = ""
    public var line: String? = nil
    public var empty: String? = nil
    public var ended: Note? = nil
    /// The Mac's window closed: a tap unlocks and watches again.
    public var needsUnlock: Bool = false

    public init(taskId: String, frame: Data? = nil, state: String = "", line: String? = nil, empty: String? = nil, ended: Note? = nil,
                needsUnlock: Bool = false) {
        self.taskId = taskId
        self.frame = frame
        self.state = state
        self.line = line
        self.empty = empty
        self.ended = ended
        self.needsUnlock = needsUnlock
    }
}

/// Watch live (PHN-454, PHN-111; kb PHN-810; Android: model/LiveWatch.kt): a task's live preview at the link's own
/// pace, each frame kept in memory and never written anywhere (no file, no cache, no notification); the screen that
/// shows it follows the owner's screenshot switch like every page (owner 2026-11-23, kb PHN-810, PHN-811). It stops
/// when the screen goes, the app goes to the background, or the app locks; on the public relay it is never asked for
/// (the relay's rule is said instead).
@MainActor
@Observable
public final class LiveWatch {
    public private(set) var view: LiveScreen?
    @ObservationIgnored private let api: PhoneApi
    @ObservationIgnored private var live: Live?
    @ObservationIgnored private var generation = 0

    public init(api: PhoneApi) {
        self.api = api
    }

    public var showing: Bool { view != nil }

    @discardableResult
    public func start(_ taskId: String, worker: String? = nil) -> Task<Void, Never> {
        let api = self.api
        // a step's own picture (a card row's short view, kb PHN-726) comes at the row's pace
        return begin(taskId) { sink in
            if let worker { await api.watchStepThumbnail(taskId: taskId, worker: worker, everyMs: Thumbnails.everyMs, sink: sink) }
            else { await api.watchLive(taskId: taskId, sink: sink) }
        }
    }

    /// A coding session's window on the Mac (kb HAND-651): the same preview, rules and screen as a task's; `taskId` holds
    /// the session's id.
    @discardableResult
    public func startSession(_ session: String) -> Task<Void, Never> {
        let api = self.api
        return begin(session) { sink in await api.watchSession(session, sink: sink) }
    }

    /// A session's window that isn't asked for (the public relay's rule, HAND-710): its place on screen says why.
    public func refused(_ session: String, _ why: Note) {
        stop(keepView: true)
        view = LiveScreen(taskId: session, ended: why)
    }

    private func begin(_ id: String, _ ask: @escaping @MainActor (PreviewSink) async -> Outcome<Live>) -> Task<Void, Never> {
        stop(keepView: true)
        generation += 1
        let gen = generation
        view = LiveScreen(taskId: id)
        let sink = Sink(self, generation: gen)
        return Task {
            let asked = await ask(sink)
            switch asked {
            case .ok(let l):
                if gen == generation { live = l } else { l.stop() }
            case .failed(let e):
                if gen == generation { view?.ended = AppModel.errorNote(e) }
            }
        }
    }

    /// Ends the stream; the screen goes too unless `keepView`. The last frame is dropped with it.
    public func stop(keepView: Bool = false) {
        generation += 1
        live?.stop()
        live = nil
        if !keepView { view = nil }
    }

    fileprivate func frame(_ gen: Int, _ f: PreviewFrame) {
        guard gen == generation, var v = view else { return }
        if let jpeg = f.jpeg { v.frame = jpeg } else if f.gone { v.frame = nil }
        v.state = f.state
        v.line = f.line
        v.empty = f.emptyText
        view = v
    }

    fileprivate func ended(_ gen: Int, code: String?, detail: String?) {
        guard gen == generation, let code, view != nil else { return }
        switch code {
        case "needs_better_path": view?.ended = Note("not_on_shared_relay")
        case "needs_unlock": view?.ended = Note("live_needs_unlock")
        case "closed": view?.ended = Note("live_ended")
        default: view?.ended = Note("err_mac_said", detail ?? "")
        }
        view?.needsUnlock = code == "needs_unlock"
    }

    /// The preview's answers, on its own thread: posted to the main actor.
    private final class Sink: PreviewSink, @unchecked Sendable {
        private weak var owner: LiveWatch?
        private let generation: Int
        init(_ owner: LiveWatch, generation: Int) {
            self.owner = owner
            self.generation = generation
        }
        func frame(frame: PreviewFrame) {
            let (o, g) = (owner, generation)
            Task { @MainActor in o?.frame(g, frame) }
        }
        func ended(code: String?, detail: String?) {
            let (o, g) = (owner, generation)
            Task { @MainActor in o?.ended(g, code: code, detail: detail) }
        }
    }
}
