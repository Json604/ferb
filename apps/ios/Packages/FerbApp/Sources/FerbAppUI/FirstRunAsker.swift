import AVFoundation
import FerbAppModel
import Observation
import Speech
import SwiftUI
import UserNotifications

/// The first run's asks on iPhone (owner kb PHN-635; PHN-636..639; Android's rememberFirstRun): Continue brings iOS's
/// own questions one after another; each answer, yes or no, is kept so it is never asked again; the location's Yes
/// turns on sharing the city (what he is granting). When-in-use only; the exact spot is still asked per task
/// (GATE-613). An ask iOS already decided is never on the page (iOS would not ask again anyway).
@MainActor
@Observable
final class FirstRunAsker {
    private(set) var pending: [FirstRun.Ask] = []
    @ObservationIgnored private let prefs: Prefs
    @ObservationIgnored private let sensors: Sensors
    /// The profile sheet reads the city switch again (set by the shell once its model exists).
    @ObservationIgnored var refreshProfile: () -> Void = {}
    @ObservationIgnored private var notificationsDecided = true
    @ObservationIgnored private var asking = false

    init(prefs: Prefs, sensors: Sensors) {
        self.prefs = prefs
        self.sensors = sensors
    }

    private var run: FirstRun {
        FirstRun(decided: { [unowned self] in decided($0) }, asked: { [prefs] in prefs.firstRunAsked },
                 record: { [prefs] in prefs.firstRunAsked = $0 })
    }

    private func decided(_ a: FirstRun.Ask) -> Bool {
        switch a {
        case .notifications: notificationsDecided
        case .location: sensors.authorization != .notDetermined
        case .camera: CameraAccess.now != .notAsked
        case .microphone:
            AVAudioApplication.shared.recordPermission != .undetermined && SFSpeechRecognizer.authorizationStatus() != .notDetermined
        }
    }

    /// Reads what iOS has decided (notifications' answer is asynchronous), then what the page asks for.
    func refresh() async {
        notificationsDecided = !(await NotificationPoster.notAsked())
        if !asking { pending = Self.skippedForUITests ? [] : run.pending() }
    }

    /// The UI tests about other pages start past the first run (`-FERBUITestFirstRunDone YES`; Debug, simulator only).
    private static var skippedForUITests: Bool {
        #if DEBUG && targetEnvironment(simulator)
        UserDefaults.standard.bool(forKey: "FERBUITestFirstRunDone")
        #else
        false
        #endif
    }

    /// Continue: iOS's questions, one after another.
    func start() {
        guard !asking else { return }
        asking = true
        Task {
            for a in run.pending() {
                let granted = await ask(a)
                run.answered(a)
                if a == .location && granted {
                    prefs.shareCity = true
                    sensors.cityStateChanged()
                    refreshProfile()
                }
            }
            asking = false
            pending = []
        }
    }

    /// Not now: passed over; each feature explains itself when it is used.
    func notNow() {
        run.notNow()
        pending = []
    }

    private func ask(_ a: FirstRun.Ask) async -> Bool {
        switch a {
        case .notifications: return await NotificationPoster.ask()
        case .location: return await withCheckedContinuation { c in sensors.ask { c.resume(returning: $0) } }
        case .camera: return await CameraAccess.ask()
        case .microphone: return await AppleSpeech.ask()
        }
    }
}
