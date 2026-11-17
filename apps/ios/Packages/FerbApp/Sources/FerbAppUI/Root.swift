import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI

/// Everything on screen (Android's Root.kt): the live sky behind every page (the shared city's coarse place, else the
/// time zone's, PHN-420/450), the page the gate allows, and a sheet over it. Nothing of FERB's is drawn before the
/// gate is main (PHN-8). The app switcher and screen captures are covered (Privacy.swift, PHN-565).
public struct FerbRoot: View {
    @ObservedObject var app: AppShell
    @Environment(\.scenePhase) private var phase

    public init(app: AppShell) { self.app = app }

    public var body: some View {
        #if DEBUG && targetEnvironment(simulator)
        if UserDefaults.standard.bool(forKey: "FERBUITestShield") { ShieldCheckPage() } else { root }
        #else
        root
        #endif
    }

    private var root: some View {
        RootView(model: app.model, shell: app)
            .onChange(of: phase, initial: true) { _, p in app.scene(p) }
    }
}

#if DEBUG && targetEnvironment(simulator)
/// A Debug simulator started with `-FERBUITestShield YES` shows only this: a plain half and a shielded half, for
/// checking by `xcrun simctl io booted screenshot` (and the simulator's own Trigger Screenshot) what the screenshot
/// shield does here (kb PHN-597). Never in a Release build or on a phone.
struct ShieldCheckPage: View {
    var body: some View {
        VStack(spacing: 0) {
            Text("PLAIN: always in screenshots").font(.title2.bold()).foregroundStyle(.black)
                .frame(maxWidth: .infinity, maxHeight: .infinity).background(Color.yellow)
                .accessibilityIdentifier("shield-plain")
            Text(ScreenshotShield.available ? "SHIELDED: should be blank in a screenshot" : "NO SHIELD ON THIS iOS")
                .font(.title2.bold()).foregroundStyle(.white)
                .frame(maxWidth: .infinity, maxHeight: .infinity).background(Color.red)
                .screenshotShielded()
                .accessibilityElement()
                .accessibilityLabel(Text(ScreenshotShield.available ? "shielded" : "no shield"))
                .accessibilityIdentifier("shield-shielded")
        }
        .ignoresSafeArea()
    }
}
#endif

struct RootView: View {
    @Bindable var model: AppModel
    let shell: AppShell

    var body: some View {
        GeometryReader { geo in
            let insets = PhoneInsets(top: geo.safeAreaInsets.top, bottom: geo.safeAreaInsets.bottom)
            LiveSky(place: model.skyPlace()) { moment in
                let gate = model.ui.gate
                PhoneScreen(moment: moment, insets: insets,
                            season: gate == .main && model.tab == .chat && model.home,
                            sheet: {
                                if gate == .main || gate == .pair {
                                    SheetsView(model: model, shell: shell, insets: insets)
                                }
                            }, page: {
                                // the gate's pages change with a rise and a fade; the lock and the start replace
                                // everything at once (PHN-8), and the page after the lock comes in (kb PHN-623)
                                // the first run's asks come once, after the first unlock, before pairing or the app
                                // (kb PHN-636)
                                let asks = !shell.firstRun.pending.isEmpty && (gate == .pair || gate == .main)
                                MotionSwitch(Screen(gate: gate, scan: gate == .pair && model.sheet == .scan && !model.ui.pairing, asks: asks),
                                             instant: { $0.gate == .locked || $0.gate == .starting }) { s in
                                    if s.asks { AsksPage(asker: shell.firstRun, insets: insets) } else { page(s.gate, insets: insets, moment: moment) }
                                }
                            })
                    .preferredColorScheme(moment.ink.topIsDark ? .light : .dark)
            }
        }
        // the launch (kb PHN-631..634): FERB's mark from the first frame over everything, settling into the lock
        // page's mark as soon as the gate allows a page
        .launchOverlay(shell.launch, ready: model.ui.gate != .starting)
        .onAppear { StartLog.mark("first frame") }
        .onChange(of: model.ui.gate, initial: true) { _, g in if g != .starting { StartLog.mark("page", "\(g)") } }
        .ferbPrivacy(hideWhenCaptured: model.settings.blockScreenshots, mustHide: model.mustSecure)
        .task(id: model.ui.gate) {
            await shell.firstRun.refresh()
            await askNotificationsOnce()
        }
    }

    /// Which page shows: the gate's, and the scanner as a page of the pairing gate of its own (MotionSwitch's key).
    private struct Screen: Hashable {
        let gate: Gate
        let scan: Bool
        var asks = false
    }

    @ViewBuilder private func page(_ gate: Gate, insets: PhoneInsets, moment: SkyMoment) -> some View {
        switch gate {
        case .starting:
            // after the launch (unlocked before the phone opened): the mark breathes where the launch left it
            if shell.launch.done { StartingMark() } else { Color.clear }
        case .failed:
            MessagePage(title: L.s("failed_title"), text: L.s("failed_text"), action: L.s("try_again"), insets: insets) {
                Task { await shell.phone.start() }
            }
        case .noScreenLock:
            MessagePage(title: L.s("no_lock_title"), text: L.s("no_lock_text"), action: L.s("no_lock_open_settings"), insets: insets) {
                if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
            }
        case .locked:
            UnlockPage(insets: insets, bio: Words.bioName, symbol: Words.bioSymbol) { model.unlockApp() }
        case .pair:
            if model.sheet == .scan && !model.ui.pairing {
                ScanPage(model: model, insets: insets)
            } else {
                PairView(model: model, insets: insets)
            }
        case .codes:
            CodesPage(code: model.ui.link?.pairingCode ?? "", insets: insets) { model.cancelPairing() }
        case .removed:
            MessagePage(title: L.s("removed_title"),
                        text: L.bio(model.ui.pairReason == .keys ? "pair_reason_keys"
                                    : model.ui.pairReason == .unknown ? "pair_reason_unknown" : "pair_reason_removed"),
                        action: L.s("pair_again"), insets: insets) { model.forget() }
        case .main:
            // the connection control in every page's top bar (kb PHN-751)
            MainView(model: model, shell: shell, insets: insets, moment: moment).environment(\.connection, connection(model))
        }
    }

    /// Notifications are asked once, after FERB's own words, when the phone is first paired (iOS asks only once; no
    /// Stay connected switch to hang it on, kb PHN-566).
    private func askNotificationsOnce() async {
        guard model.ui.gate == .main, !shell.prefs.notificationsExplained, model.sheet == nil, shell.firstRun.pending.isEmpty else { return }
        guard await NotificationPoster.notAsked() else { shell.prefs.notificationsExplained = true; return }
        model.show(.notifications)
    }
}

/// The pairing page (Android's Pair): the lab's page, and over it the line saying why it shows or what went wrong.
struct PairView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        ZStack(alignment: .top) {
            PairPage(insets: insets,
                     onScan: { model.clearPairError(); model.show(.scan) },
                     onTypeCode: { model.clearPairError(); model.show(.typeCode) },
                     onGetMacApp: { if let u = URL(string: L.s("mac_app_url")) { UIApplication.shared.open(u) } },
                     onHelp: { model.show(.help) })
            if let line {
                Card(padding: 16) {
                    Text(line).type(15.5, .regular, Ink.black).frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityAddTraits(.updatesFrequently)
                }
                .padding(.horizontal, 16).padding(.top, 70)
            }
        }
    }

    private var line: String? {
        if model.ui.pairing { return L.s("pair_reaching") }
        if let e = model.ui.pairError { return e.text }
        switch model.ui.pairReason {
        case .wiped: return L.s("pair_reason_wiped")
        case .removed: return L.s("pair_reason_removed")
        case .keys: return L.bio("pair_reason_keys")
        case .unknown: return L.s("pair_reason_unknown")
        case nil: return nil
        }
    }
}

/// The scanner as a page of its own (Android's Scanner and ScannerPage, one look, kb PHN-619): the camera in the QR
/// frame (its picture clipped to the frame's well, kb PHN-618), or why the camera is needed, and under it the lab's
/// white card with the title, Allow the camera while it isn't asked, Type code and Cancel. Without a camera (the
/// simulator) or with it refused, the typed code.
struct ScanPage: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    @State private var access = CameraAccess.now

    var body: some View {
        ScanPageLayout(access: access, insets: insets,
                       onAllow: { Task { _ = await CameraAccess.ask(); access = CameraAccess.now } },
                       onTypeCode: { model.show(.typeCode) },
                       onCancel: { model.show(nil) }) {
            QRScanner { code in model.pair(code) }
        }
    }
}

/// ScanPage as it is drawn (stateless, for the snapshots).
struct ScanPageLayout<Preview: View>: View {
    let access: CameraAccess
    let insets: PhoneInsets
    let onAllow: () -> Void
    let onTypeCode: () -> Void
    let onCancel: () -> Void
    @ViewBuilder let preview: () -> Preview
    @Environment(\.skyInk) private var ink

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
            if access == .allowed {
                QrFrame(hint: L.s("scan_hint"), live: true, preview: preview)
                    .padding(.horizontal, 40)
            } else {
                Text(L.s(access == .none ? "scan_no_camera" : access == .denied ? "scan_denied" : "scan_camera_why"))
                    .type(16, .medium, ink.top).multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 32)
            }
            Spacer()
            Card(padding: 20) {
                VStack(alignment: .leading, spacing: 12) {
                    Text(L.s("scan_title")).type(24, .semibold)
                    if access == .notAsked {
                        Pill(title: L.s("scan_allow"), kind: .black, symbol: "qrcode.viewfinder", onClick: onAllow)
                    }
                    HStack { PairAction(symbol: "keyboard", title: L.s("scan_type_code"), onClick: onTypeCode) }
                    Pill(title: L.s("cancel"), kind: .plain, height: 44, onClick: onCancel)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}

/// The phone's own kind in the app's words (Android says "fingerprint").
enum Words {
    static var bio: Biometry { Biometry.current }
    /// "Face ID", "Touch ID", "your passcode".
    static var bioName: String { L.bioWord(bio) }
    /// As a row's title: "Face ID", "Touch ID", "Passcode".
    static var bioTitle: String {
        let w = bioName
        return w.hasPrefix("your ") ? String(w.dropFirst(5)).capitalized : w
    }
    static var bioSymbol: String {
        switch bio {
        case .faceID: "faceid"
        case .touchID: "touchid"
        case .opticID: "opticid"
        case .none: "lock"
        }
    }
}

/// The phone still opening after the launch ended: the launch's mark, breathing over the sky (no ink0 ground).
private struct StartingMark: View {
    @State private var state = LaunchState(reduced: UIAccessibility.isReduceMotionEnabled)
    var body: some View { LaunchOverlay(state: state, target: nil, ground: false) }
}

/// The first run's page with the app's words (kb PHN-636; Android's Asks in Root.kt).
private struct AsksPage: View {
    let asker: FirstRunAsker
    let insets: PhoneInsets

    var body: some View {
        PermissionsPage(title: L.s("first_run_title"), intro: L.s("first_run_intro"), rows: asker.pending.map(Self.row),
                        continueTitle: L.s("first_run_continue"), notNowTitle: L.s("first_run_not_now"), insets: insets,
                        onContinue: { asker.start() }, onNotNow: { asker.notNow() })
    }

    static func row(_ a: FirstRun.Ask) -> AskRow {
        switch a {
        case .notifications: AskRow(symbol: "bell", title: L.s("first_run_notifications"), why: L.s("first_run_notifications_why"))
        case .location: AskRow(symbol: "location", title: L.s("first_run_location"), why: L.s("first_run_location_why"))
        case .camera: AskRow(symbol: "camera", title: L.s("first_run_camera"), why: L.s("first_run_camera_why"))
        case .microphone: AskRow(symbol: "mic", title: L.s("first_run_microphone"), why: L.s("first_run_microphone_why"))
        }
    }
}
