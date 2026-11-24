import CoreLocation
import Network
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import UIKit
import UserNotifications

/// The app's process (the twin of Android's FerbApplication, kb PHN-500's iPhone half): the phone (FerbPhone), the
/// lock over the process (Session), the app model, the signals, notifications, the phone's facts and location. A
/// relaunch starts locked (PHN-8). iOS has no Stay connected (kb PHN-566): leaving the screen keeps the link up only
/// while the background task iOS grants lasts; before it ends, transfers are checkpointed and the link pauses.
@MainActor
public final class AppShell: ObservableObject {
    public let phone: FerbPhone
    public let session: Session
    public let prefs: Prefs
    public let local: LocalFiles
    public let model: AppModel
    /// FERB's account on this phone (kb PHN-827).
    public let account: AccountModel
    let signals: PhoneSignals
    let notifier: Notifier
    let poster = NotificationPoster()
    let sensors: Sensors
    private let lockTeller: LockTeller
    private var background: UIBackgroundTaskIdentifier = .invalid
    private var observers: [NSObjectProtocol] = []
    /// The launch (kb PHN-631..634): FERB's mark from the first frame, settling into the lock page's mark; the prompt
    /// by itself waits for it to settle (PHN-633).
    public let launch = LaunchState(reduced: UIAccessibility.isReduceMotionEnabled)
    /// The first run's asks (kb PHN-636): once, after the first unlock.
    let firstRun: FirstRunAsker
    private var active = false

    public init() {
        StartLog.mark("app")
        Self.freshForUITests()
        prefs = Prefs()
        session = Session()
        phone = FerbPhone.create()
        local = LocalFiles.forPhone()
        let api = LibraryPhone(phone)
        account = Self.account(prefs: prefs)
        let sensors = Sensors(prefs: prefs)
        self.sensors = sensors
        firstRun = FirstRunAsker(prefs: prefs, sensors: sensors)
        model = AppModel(api: api, account: account, session: session, prefs: prefs, local: local, saved: Self.loadSaved(),
                         locator: sensors, coarse: { sensors.places.coarse }, locationAnswered: {},
                         voice: AppleSpeech(), micAllowed: { AppleSpeech.allowed })
        model.prompts = Self.prompts(phone)
        firstRun.refreshProfile = { [model] in model.refreshProfile() }
        Task { [firstRun] in await firstRun.refresh() }  // ready before the first unlock ends
        let poster = self.poster
        notifier = Notifier(api: api, prefs: prefs, session: session,
                            lockedNow: { !UIApplication.shared.isProtectedDataAvailable },
                            deliver: { poster.apply($0) })
        signals = PhoneSignals(phone: phone)
        lockTeller = LockTeller(session: session, api: api, local: local)
        sensors.start(api: api)
        signals.start()
        notifier.start()
        poster.answer = { [weak self] id, choice, typed in await self?.notifier.answer(id, choice: choice, typed: typed) }
        poster.open = { [weak self] tap in self?.open(tap) }
        poster.insomnia = { [weak self] notice, lock in _ = await self?.notifier.insomniaAnswer(notice, lock: lock) }
        Task {
            let began = Date()
            let s = await phone.start()
            StartLog.mark("phone open", "\(s.isOpen ? "open" : "not open"), \(Int(Date().timeIntervalSince(began) * 1000)) ms: library, keys, link and store")
        }
        launch.onPromptOk = { [weak self] in
            guard let self else { return }
            StartLog.mark("launch settled")
            if active { model.setWindowReady(true) }
        }
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.notifier.lockScreen(locked: true) }
        })
        observers.append(center.addObserver(forName: UIApplication.protectedDataDidBecomeAvailableNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.notifier.lockScreen(locked: false) }
        })
    }

    /// The scene's phase (FerbRoot): the lock's clock, the link's activity, the background time.
    func scene(_ phase: ScenePhase) {
        switch phase {
        case .active:
            endBackground()
            session.foreground(true)
            signals.scene(active: true)
            model.transfers.resumeAfterBackground()
            active = true
            model.setWindowReady(launch.promptOk)  // the unlock prompt by itself may show now (PHN-615), the mark settled (PHN-633)
        case .background:
            active = false
            model.setWindowReady(false)
            session.foreground(false)
            signals.scene(active: false)
            Self.save(model.saved)
            beginBackground()
        default:
            // .inactive: the app switcher, a system sheet or Face ID's own sheet over FERB; the privacy cover shows
            // (PHN-811) and no prompt starts by itself (PHN-615)
            active = false
            model.setWindowReady(false)
        }
    }

    /// The time iOS gives after the app leaves the screen: the link stays up and notifications are posted until it
    /// ends; at its end transfers pause where they are and the link pauses (nothing more arrives until FERB opens).
    private func beginBackground() {
        guard background == .invalid else { return }
        background = UIApplication.shared.beginBackgroundTask(withName: "FERB") { [weak self] in
            MainActor.assumeIsolated { self?.backgroundEnding() }
        }
    }

    private func backgroundEnding() {
        let id = background
        Task { [weak self] in
            guard let self else { return }
            await model.transfers.checkpoint()
            // once suspended FERB can't take a check's words off the lock screen later: plan them wordless now
            notifier.lockScreen(locked: true)
            signals.backgroundEnded()
            if id != .invalid { UIApplication.shared.endBackgroundTask(id) }
            if background == id { background = .invalid }
        }
    }

    private func endBackground() {
        if background != .invalid {
            UIApplication.shared.endBackgroundTask(background)
            background = .invalid
        }
    }

    private func open(_ tap: NotificationTap) {
        switch tap {
        case .app: break
        case .question(let id): model.openQuestion(id)
        case .task: model.openChat()
        // FERB's own question (a clarify) is answered by a message: the box takes the cursor (FERB.app's needs-you)
        case .chat: model.openChat(focus: true)
        // insomnia's Show me (APP-983): the camera, after the app's unlock
        case .camera: model.openCamera()
        }
    }

    // ---- the account ------------------------------------------------------------------------------------------------

    /// The real account: the Keychain (its own service, apart from the vault's), URLSession, the system's web sheet and
    /// its own Sign in with Apple.
    /// A Debug simulator run by the UI tests (`-FERBUITestUnlock YES`) never reaches the site: an account in memory
    /// (signed in with `-FERBUITestSignedIn YES`) and a stand-in site. Never in a Release build or on a phone.
    private static func account(prefs: Prefs) -> AccountModel {
        let b = Bundle.main
        let version = b.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        #if DEBUG && targetEnvironment(simulator)
        if UserDefaults.standard.bool(forKey: "FERBUITestUnlock") {
            let store = MemoryStore()
            if prefs.accountDeviceId == nil { prefs.accountDeviceId = "ui-test-device-0001" }
            if UserDefaults.standard.bool(forKey: "FERBUITestSignedIn") {
                try? store.set("account", AccountModel.savedItem(
                    AccountInfo(id: "00000000-0000-4000-8000-000000000001", email: "someone@example.com", name: nil, providers: ["google"]),
                    accessToken: "fa1_invented", accessExpiresAt: Date().addingTimeInterval(3600), refreshToken: "fr1_invented"))
            }
            return AccountModel(store: store, http: UITestSite(), web: UITestWeb(), prefs: prefs, version: version)
        }
        #endif
        return AccountModel(store: KeychainStore(service: "com.ferb.phone.account"), http: URLSessionHttp(), web: SystemWebSignIn(),
                            apple: SystemAppleSignIn(), prefs: prefs, version: version)
    }

    // ---- the prompts ------------------------------------------------------------------------------------------------

    /// The real prompts; a Debug build on the simulator started by the UI tests with `-FERBUITestUnlock YES` passes
    /// the prompt alone (a UI test can't answer Face ID's sheet). Never in a Release build or on a phone.
    private static func prompts(_ phone: FerbPhone) -> Prompts {
        #if DEBUG && targetEnvironment(simulator)
        if UserDefaults.standard.bool(forKey: "FERBUITestUnlock") { return UITestPrompts(phone) }
        #endif
        return PhonePrompts(phone)
    }

    /// A Debug simulator run by the UI tests with `-FERBUITestFresh YES` starts as a new install.
    private static func freshForUITests() {
        #if DEBUG && targetEnvironment(simulator)
        guard UserDefaults.standard.bool(forKey: "FERBUITestFresh") else { return }
        let fm = FileManager.default
        for dir in [FileManager.SearchPathDirectory.applicationSupportDirectory, .cachesDirectory] {
            if let u = fm.urls(for: dir, in: .userDomainMask).first { try? fm.removeItem(at: u.appendingPathComponent(dir == .cachesDirectory ? "files" : "ferb")) }
        }
        if let id = Bundle.main.bundleIdentifier {
            let args = UserDefaults.standard.volatileDomain(forName: UserDefaults.argumentDomain)
            UserDefaults.standard.removePersistentDomain(forName: id)
            UserDefaults.standard.setVolatileDomain(args, forName: UserDefaults.argumentDomain)
        }
        #endif
    }

    // ---- what the shell keeps between launches (never a private value) -----------------------------------------------

    private static let savedKey = "ferb.saved"

    static func loadSaved() -> SavedState {
        guard let data = UserDefaults.standard.data(forKey: savedKey), let s = try? JSONDecoder().decode(SavedState.self, from: data) else {
            return SavedState()
        }
        return s
    }

    static func save(_ s: SavedState) {
        if let data = try? JSONEncoder().encode(s) { UserDefaults.standard.set(data, forKey: savedKey) }
    }
}

#if DEBUG && targetEnvironment(simulator)
/// The UI tests' stand-in for FERB's site (Debug, simulator only): an invented pairing assertion; nothing leaves the
/// simulator.
final class UITestSite: HttpClient {
    func send(_ request: URLRequest) async throws -> (status: Int, body: Data) {
        switch request.url?.path {
        case "/api/app/assertion": return (200, Data(#"{"assertion":"fpa1.invented.ui-test","expires_in":300}"#.utf8))
        case "/api/app/signout": return (200, Data(#"{"ok":true}"#.utf8))
        default: return (503, Data(#"{"error":"unavailable"}"#.utf8))
        }
    }
}

/// The UI tests' stand-in for the web sign-in sheet: closed at once.
@MainActor
final class UITestWeb: WebSignIn {
    func open(_ url: URL, callbackScheme: String) async -> WebSignInResult { .cancelled }
}

/// The UI tests' stand-in for the system prompt (Debug, simulator only): the owner "passed" it at once, or, for the
/// first `-FERBUITestCancelPrompts N` prompts, cancelled it. Signing still goes through the phone's keys (software
/// ones on the simulator).
@MainActor
final class UITestPrompts: Prompts {
    private let phone: FerbPhone
    private var cancels = UserDefaults.standard.integer(forKey: "FERBUITestCancelPrompts")
    init(_ phone: FerbPhone) { self.phone = phone }
    func signer(_ kind: KeyKind, why: Why) -> Signer? {
        if cancels > 0 { cancels -= 1; return Cancelled() }
        return phone.signer(kind, reason: why.reason)
    }
    func local() async -> Bool {
        if cancels > 0 { cancels -= 1; return false }
        return true
    }

    private final class Cancelled: Signer, @unchecked Sendable {
        func sign(payload: String) -> String? { nil }
    }
}
#endif

/// The app delegate: notifications' answers and taps arrive here, also when iOS starts FERB for one.
@MainActor
public final class FerbAppDelegate: NSObject, UIApplicationDelegate {
    public lazy var app = AppShell()

    public func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = app.poster
        app.poster.registerCategories()
        return true
    }
}

// ---- the phone's own facts and location (T-1214, SENSE-420, GATE-613) ---------------------------------------------

/// iOS's side of the phone's facts (DeviceReporter) and location (PlaceKeeper, Locator): battery, Low Power Mode,
/// the time zone and language, the network kind, the city while the owner shares it (a reduced-accuracy fix, the
/// city named by Apple's geocoder; never coordinates to the Mac), and one exact fix when the owner says yes to the
/// Mac's request. Every reading comes from a callback; nothing polls.
@MainActor
final class Sensors: NSObject, Locator, CLLocationManagerDelegate {
    let reporter: DeviceReporter
    let places: PlaceKeeper
    private let prefs: Prefs
    private let manager = CLLocationManager()
    private var exactWaiters: [CheckedContinuation<CLLocation?, Never>] = []
    private var observers: [NSObjectProtocol] = []
    private let path = NWPathMonitor()

    init(prefs: Prefs) {
        self.prefs = prefs
        let d = UIDevice.current
        reporter = DeviceReporter(DeviceFacts(name: d.name, model: d.model, os: "\(d.systemName) \(d.systemVersion)"))
        let geocoder = CLGeocoder()
        places = PlaceKeeper { lat, lon in
            let marks = try? await geocoder.reverseGeocodeLocation(CLLocation(latitude: lat, longitude: lon))
            guard let m = marks?.first, let city = m.locality ?? m.subAdministrativeArea else { return nil }
            return DevicePlace(city: city, region: m.administrativeArea ?? "", country: m.country ?? "", countryCode: m.isoCountryCode ?? "")
        }
        super.init()
        manager.delegate = self
        manager.desiredAccuracy = kCLLocationAccuracyReduced
    }

    func start(api: PhoneApi) {
        let d = UIDevice.current
        d.isBatteryMonitoringEnabled = true
        let center = NotificationCenter.default
        let read: @MainActor () -> Void = { [weak self] in self?.readNow() }
        for name in [UIDevice.batteryLevelDidChangeNotification, UIDevice.batteryStateDidChangeNotification,
                     Notification.Name.NSProcessInfoPowerStateDidChange, Notification.Name.NSSystemTimeZoneDidChange,
                     NSLocale.currentLocaleDidChangeNotification] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { _ in MainActor.assumeIsolated { read() } })
        }
        path.pathUpdateHandler = { [weak self] p in
            let kind = p.status != .satisfied ? "offline" : p.usesInterfaceType(.wifi) ? "wifi"
                : p.usesInterfaceType(.cellular) ? "mobile" : p.usesInterfaceType(.wiredEthernet) ? "ethernet" : "other"
            Task { @MainActor in self?.reporter.network(kind) }
        }
        path.start(queue: DispatchQueue(label: "com.ferb.phone.facts"))
        readNow()
        cityStateChanged()
        reporter.start(api)
    }

    private func readNow() {
        let d = UIDevice.current
        if d.batteryLevel >= 0 { reporter.battery(percent: Int((d.batteryLevel * 100).rounded()), charging: d.batteryState == .charging || d.batteryState == .full) }
        reporter.powerSaver(ProcessInfo.processInfo.isLowPowerModeEnabled)
        let zone = TimeZone.current
        reporter.zone(zone.identifier, offsetMinutes: zone.secondsFromGMT() / 60)
        reporter.locale(Locale.current.identifier(.bcp47), languages: Locale.preferredLanguages)
    }

    /// The owner's switch or iOS's permission changed: start or stop the coarse fixes.
    func cityStateChanged() {
        let allowed = [.authorizedWhenInUse, .authorizedAlways].contains(manager.authorizationStatus)
        if prefs.shareCity && allowed {
            reporter.location(DeviceFacts.locationOn, place: places.place)
            manager.startMonitoringSignificantLocationChanges()
            manager.requestLocation()
        } else {
            manager.stopMonitoringSignificantLocationChanges()
            places.forget()
            reporter.location(prefs.shareCity ? DeviceFacts.locationDenied : DeviceFacts.locationOff, place: nil)
        }
    }

    /// iOS's own question (after FERB's words). Calls `done` with whether it allowed a location.
    func ask(_ done: @escaping (Bool) -> Void) {
        if manager.authorizationStatus != .notDetermined { done(allowed()); return }
        askDone = done
        manager.requestWhenInUseAuthorization()
    }

    private var askDone: ((Bool) -> Void)?

    // Locator
    func allowed() -> Bool { [.authorizedWhenInUse, .authorizedAlways].contains(manager.authorizationStatus) }

    /// iOS's answer about location so far (notDetermined: never asked).
    var authorization: CLAuthorizationStatus { manager.authorizationStatus }

    func exact(timeout: Double) async -> LocationFix? {
        manager.desiredAccuracy = kCLLocationAccuracyBest
        defer { manager.desiredAccuracy = kCLLocationAccuracyReduced }
        let location: CLLocation? = await withCheckedContinuation { cont in
            exactWaiters.append(cont)
            manager.requestLocation()
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: UInt64(timeout * 1e9))
                self?.finishExact(nil)
            }
        }
        guard let l = location else { return nil }
        return LocationFix(latitude: l.coordinate.latitude, longitude: l.coordinate.longitude, accuracyM: l.horizontalAccuracy)
    }

    private func finishExact(_ l: CLLocation?) {
        let waiters = exactWaiters
        exactWaiters = []
        for w in waiters { w.resume(returning: l) }
    }

    nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        MainActor.assumeIsolated {
            if let done = askDone, manager.authorizationStatus != .notDetermined { askDone = nil; done(allowed()) }
            cityStateChanged()
        }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        MainActor.assumeIsolated {
            guard let l = locations.last else { return }
            if !exactWaiters.isEmpty, l.horizontalAccuracy <= 100 { finishExact(l) }
            guard prefs.shareCity else { return }
            Task {
                let place = await places.fix(latitude: l.coordinate.latitude, longitude: l.coordinate.longitude)
                reporter.location(DeviceFacts.locationOn, place: place)
            }
        }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        MainActor.assumeIsolated { if !exactWaiters.isEmpty { finishExact(nil) } }
    }
}
