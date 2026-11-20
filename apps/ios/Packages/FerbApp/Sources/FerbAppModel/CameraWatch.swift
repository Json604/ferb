import FerbCore
import FerbMobile
import Foundation
import Observation

/// What the camera view ("Your Mac's camera") shows: the last frame (JPEG bytes in memory; the screen decodes them),
/// the Mac's state and words, or why it ended. (Android's CameraView.)
public struct CameraScreen: Equatable, Sendable {
    public var frame: Data? = nil
    /// starting, live, off, denied, failed (the Mac's).
    public var state: String = "starting"
    /// The Mac's words for the state; nil: the app's "Starting your Mac's camera…" while there is no frame.
    public var text: String? = nil
    public var ended: Note? = nil
    /// The app locked under it: a tap unlocks and looks again.
    public var needsUnlock: Bool = false

    public init(frame: Data? = nil, state: String = "starting", text: String? = nil, ended: Note? = nil, needsUnlock: Bool = false) {
        self.frame = frame
        self.state = state
        self.text = text
        self.ended = ended
        self.needsUnlock = needsUnlock
    }
}

/// Show me (APP-983; Android: model/CameraWatch.kt): the Mac's camera while the camera view is on screen, and only
/// then. Each frame is kept in memory and never written anywhere (no file, no cache, no notification); the view is
/// kept out of screenshots and hidden while the screen is captured ([AppModel.mustSecure]). It stops when the view
/// goes, the app goes to the background or locks; the Mac turns the camera off a few seconds after the last ask. On
/// the public relay the library refuses it before anything is asked, and the view says the library's words.
@MainActor
@Observable
public final class CameraWatch {
    public private(set) var view: CameraScreen?
    @ObservationIgnored private let api: PhoneApi
    @ObservationIgnored private var live: Live?
    @ObservationIgnored private var generation = 0

    public init(api: PhoneApi) {
        self.api = api
    }

    public var showing: Bool { view != nil }

    @discardableResult
    public func start() -> Task<Void, Never> {
        stop(keepView: true)
        generation += 1
        let gen = generation
        view = CameraScreen()
        let sink = Sink(self, generation: gen)
        return Task {
            switch await api.watchCamera(sink: sink) {
            case .ok(let l):
                if gen == generation { live = l } else { l.stop() }
            case .failed(let e):
                if gen == generation { view?.ended = AppModel.errorNote(e) }
            }
        }
    }

    /// Ends the pulling; the view goes too unless `keepView`. The last frame is dropped with it.
    public func stop(keepView: Bool = false) {
        generation += 1
        live?.stop()
        live = nil
        if keepView { view?.frame = nil } else { view = nil }
    }

    func frame(_ gen: Int, _ f: CameraFrame) {
        guard gen == generation, var v = view else { return }
        // the Mac turned it off, was denied or failed: the last frame goes with it
        if let jpeg = f.jpeg { v.frame = jpeg } else if f.state != "live" && f.state != "starting" { v.frame = nil }
        v.state = f.state
        v.text = f.text
        view = v
    }

    func ended(_ gen: Int, code: String?, detail: String?) {
        guard gen == generation, let code, view != nil else { return }
        view?.frame = nil
        switch code {
        // the library's own words for the public relay (the link's rule)
        case "needs_better_path": view?.ended = detail.map { Note("err_mac_said", $0) } ?? Note("not_on_shared_relay")
        case "needs_unlock": view?.ended = Note("camera_needs_unlock")
        case "closed": view?.ended = Note("camera_ended")
        default: view?.ended = Note("err_mac_said", detail ?? "")
        }
        view?.needsUnlock = code == "needs_unlock"
    }

    /// The camera's answers, on its own thread: posted to the main actor.
    private final class Sink: CameraSink, @unchecked Sendable {
        private weak var owner: CameraWatch?
        private let generation: Int
        init(_ owner: CameraWatch, generation: Int) {
            self.owner = owner
            self.generation = generation
        }
        func frame(frame: CameraFrame) {
            let (o, g) = (owner, generation)
            Task { @MainActor in o?.frame(g, frame) }
        }
        func ended(code: String?, detail: String?) {
            let (o, g) = (owner, generation)
            Task { @MainActor in o?.ended(g, code: code, detail: detail) }
        }
    }
}
