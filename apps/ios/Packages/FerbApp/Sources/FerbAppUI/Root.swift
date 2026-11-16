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
        RootView(model: app.model, shell: app)
            .onChange(of: phase, initial: true) { _, p in app.scene(p) }
    }
}

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
                                page(gate, insets: insets, moment: moment)
                            })
                    .preferredColorScheme(moment.ink.topIsDark ? .light : .dark)
            }
        }
        .ferbPrivacy(hideWhenCaptured: model.settings.blockScreenshots, mustHide: model.mustSecure)
        .task(id: model.ui.gate) { await askNotificationsOnce() }
    }

    @ViewBuilder private func page(_ gate: Gate, insets: PhoneInsets, moment: SkyMoment) -> some View {
        switch gate {
        case .starting:
            Color.clear
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
                        text: L.bio(model.ui.pairReason == .keys ? "pair_reason_keys" : "pair_reason_removed"),
                        action: L.s("pair_again"), insets: insets) { model.forget() }
        case .main:
            MainView(model: model, shell: shell, insets: insets, moment: moment)
        }
    }

    /// Notifications are asked once, after FERB's own words, when the phone is first paired (iOS asks only once; no
    /// Stay connected switch to hang it on, kb PHN-566).
    private func askNotificationsOnce() async {
        guard model.ui.gate == .main, !shell.prefs.notificationsExplained, model.sheet == nil else { return }
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
        case nil: return nil
        }
    }
}

/// The scanner as a page of its own (Android's Scanner): the camera in the QR frame, asked for in context after the
/// app's own words; without a camera (the simulator) or with it refused, the typed code.
struct ScanPage: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    @State private var access = CameraAccess.now

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                CloseButton { model.show(nil) }
            }
            .padding(.horizontal, 16).padding(.top, 8)
            Spacer()
            switch access {
            case .allowed:
                QrFrame(hint: L.s("scan_title")) {
                    QRScanner { code in model.pair(code) }
                }
            default:
                Card(padding: 20) {
                    VStack(alignment: .leading, spacing: 14) {
                        Text(L.s("scan_title")).type(24, .semibold)
                        Text(L.s(access == .none ? "scan_no_camera" : access == .denied ? "scan_denied" : "scan_camera_why"))
                            .type(15.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                        if access == .notAsked {
                            Pill(title: L.s("scan_allow"), kind: .black, symbol: "camera") {
                                Task { _ = await CameraAccess.ask(); access = CameraAccess.now }
                            }
                        }
                        Pill(title: L.s("type_code_title"), kind: access == .notAsked ? .grey : .black, symbol: "keyboard") {
                            model.show(.typeCode)
                        }
                    }
                }
                .padding(.horizontal, 16)
            }
            Spacer()
        }
        .padding(.bottom, insets.bottom + 8)
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
