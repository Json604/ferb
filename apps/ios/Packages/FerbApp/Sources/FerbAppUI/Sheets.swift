import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import UIKit

/// The sheet over the page (Mobile.swift Sheet; Android's Sheets.kt), or nothing.
struct SheetsView: View {
    @Bindable var model: AppModel
    @Environment(\.openURL) private var openURL
    let shell: AppShell
    let insets: PhoneInsets

    var body: some View {
        // one sheet at a time, up from below and back down; a going one keeps what it last showed (kb PHN-623)
        MotionSwitch((model.sheet == .scan ? nil : model.sheet).map(SheetKey.init), kind: .sheet) { k in sheet(k.sheet) }
    }

    /// The sheet as the switch's key (AppSheet is Equatable only).
    private struct SheetKey: Hashable {
        let sheet: AppSheet
        func hash(into h: inout Hasher) { h.combine(String(describing: sheet)) }
    }

    @ViewBuilder private func sheet(_ shown: AppSheet) -> some View {
        let close = { model.show(nil) }
        switch Optional(shown) {
        case nil, .scan:
            EmptyView()   // the scanner is a page of its own (Root's ScanPage)
        case .relay:
            RelaySheet(copy: LabSample.relay, insets: insets, onClose: close)
        case .connection:
            ConnectionSheetFor(model: model, insets: insets, close: close)
        case .profile:
            Profile(model: model, shell: shell, insets: insets, close: close)
        case .stopAll:
            Plain(title: L.s("stop_all_title"), text: L.s("stop_all_text"), insets: insets, close: close) {
                Pill(title: L.s("stop_all_yes"), kind: .black, symbol: "stop.fill",
                     onClick: model.ui.busy.contains("stop-all") ? nil : { model.stopEverything() }).padding(.top, 18)
                Pill(title: L.s("cancel"), kind: .plain, height: 44, onClick: close).padding(.top, 4)
            }
        case .settings:
            SettingsView(model: model, insets: insets, close: close)
        case .days(let key):
            DaysView(model: model, key: key, insets: insets)
        case .live:
            LiveView(model: model, insets: insets)
        case .insomnia:
            InsomniaSheet(model: model, insets: insets)
        case .camera:
            CameraView(model: model, insets: insets)
        // the owner's coding sessions (kb HAND-651)
        case .session:
            SessionScreen(model: model, insets: insets)
        case .sessionYes:
            SessionYesView(model: model, insets: insets)
        case .sessionPick:
            PickTerminalView(model: model, insets: insets)
        case .sessionHand:
            SessionHandSheet(model: model, insets: insets)
        case .sessionNew:
            NewSessionSheet(model: model, insets: insets)
        case .sessionWatch:
            SessionWatchView(model: model, insets: insets)
        // a running task's short view (kb PHN-725; FERB.app's TaskShortView): its picture, title, line, Watch live, Stop
        case .task(let taskId, _):
            let r = ((model.ui.running?.tasks ?? []) + (model.temporaryRunning?.tasks ?? [])).first { $0.taskId == taskId }
            let shown = model.live.view
            Sheet(bottomInset: insets.bottom, onDismiss: close) {
                SheetTitle(r?.title ?? L.s("question_default_title"), onClose: close)
                TaskShortView(task: WorkTask(id: taskId, title: r?.title ?? "", line: shown?.line ?? r?.line ?? "", waiting: r?.waiting ?? false,
                                             paused: r?.paused ?? false, picture: shown?.frame.flatMap { UIImage(data: $0) }),
                              empty: shown?.ended?.text ?? shown?.empty,
                              onWatch: { model.watchLive(taskId) }, onStop: { model.stop(taskId) },
                              shield: { AnyView($0.screenshotShielded()) })
                    .padding(.top, 8)
            }
        case .sites(let row):
            let sites: [String] = model.rows.lazy.compactMap { if case .answer(let k, let ui) = $0, k == row { return ui.sites } else { return nil } }.first ?? []
            Sheet(bottomInset: insets.bottom, onDismiss: close) {
                SheetTitle(L.s("sites_visited"), onClose: close)
                Card {
                    ForEach(Array(sites.enumerated()), id: \.offset) { i, h in
                        if i > 0 { Divide() }
                        CardRow(title: h, onClick: { if let u = Present.siteLink(h) { openURL(u) } }) {
                            Image(systemName: "globe").font(.system(size: 20))
                        } trailing: { Opens() }
                    }
                }
                .padding(.top, 8)
            }
        case .forget:
            Plain(title: L.s("forget_title"), text: L.s("forget_text"), insets: insets, close: close) {
                Pill(title: L.s("forget_yes"), kind: .black) { model.forget() }.padding(.top, 18)
                Pill(title: L.s("cancel"), kind: .plain, height: 44, onClick: close).padding(.top, 4)
            }
        case .typeCode:
            TypeCode(model: model, insets: insets, close: close)
        case .ownRelay:
            let url = model.ui.link?.ownRelayUrl
            Plain(title: L.s("mac_own_relay_title"), text: url.map { L.s("mac_own_relay_set", hostOf($0)) } ?? L.s("mac_own_relay_none"),
                  insets: insets, close: close)
        case .notifications:
            Plain(title: L.s("notify_title"), text: L.s("notify_text"), insets: insets, close: {
                shell.prefs.notificationsExplained = true
                close()
            }) {
                Pill(title: L.s("notify_allow"), kind: .black) {
                    shell.prefs.notificationsExplained = true
                    Task {
                        if !(await NotificationPoster.ask()) { model.note(Note("notify_denied")) }
                        model.show(nil)
                    }
                }
                .padding(.top, 18)
                Pill(title: L.s("cancel"), kind: .plain, height: 44) { shell.prefs.notificationsExplained = true; close() }.padding(.top, 4)
            }
        case .voice(let kind):
            let text = L.s(kind == .onDevice ? "voice_text_device" : "voice_text_service")
            Plain(title: L.s("voice_title"), text: AppleSpeech.allowed ? text : text + "\n\n" + L.s("voice_ask_next"), insets: insets, close: close) {
                Pill(title: L.s("voice_continue"), kind: .black, symbol: "mic") {
                    model.voiceTold(kind)
                    if AppleSpeech.allowed { model.microphoneAnswered(true) }
                    else { Task { model.microphoneAnswered(await AppleSpeech.ask()) } }
                }
                .padding(.top, 18)
                Pill(title: L.s("cancel"), kind: .plain, height: 44, onClick: close).padding(.top, 4)
            }
        case .location(let id):
            let busy = model.ui.busy.contains("location")
            Plain(title: L.s("exact_title"), text: L.s(busy ? "exact_reading" : "exact_text"), insets: insets, close: { model.declineLocation(id) }) {
                Pill(title: L.s("exact_share"), kind: .black, onClick: busy ? nil : {
                    shell.sensors.ask { granted in model.shareLocation(id, granted: granted) }
                }).padding(.top, 18)
                Pill(title: L.s("exact_no"), kind: .plain, height: 44, onClick: busy ? nil : { model.declineLocation(id) }).padding(.top, 4)
            }
        case .city:
            Plain(title: L.s("city_title"), text: L.s("city_text"), insets: insets, close: close) {
                Pill(title: L.s("city_continue"), kind: .black) {
                    shell.sensors.ask { granted in
                        shell.prefs.shareCity = granted
                        shell.sensors.cityStateChanged()
                        model.show(.profile)
                        model.cityChanged(granted: granted)
                    }
                }
                .padding(.top, 18)
                Pill(title: L.s("cancel"), kind: .plain, height: 44) { model.show(.profile) }.padding(.top, 4)
            }
        case .help:
            Plain(title: L.s("help_title"), text: L.s("help_text"), insets: insets, close: close)
        case .background:
            Plain(title: L.s("background_title"), text: L.s("background_text"), insets: insets, close: { model.show(.profile) })
        }
    }
}

/// A sheet with a title, a close button, its words and what else it holds (the relay and profile sheets' frame).
struct Plain<More: View>: View {
    let title: String
    let text: String
    let insets: PhoneInsets
    let close: () -> Void
    @ViewBuilder var more: More

    var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: close) {
            SheetTitle(title, onClose: close)
            Text(text).type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true).padding(.top, 6)
            more
        }
    }
}

extension Plain where More == EmptyView {
    init(title: String, text: String, insets: PhoneInsets, close: @escaping () -> Void) {
        self.init(title: title, text: text, insets: insets, close: close) { EmptyView() }
    }
}

/// A grey field for typed words (the type-code sheet, a question's answer).
struct Field: View {
    @Binding var text: String
    let hint: String
    var capitals = false
    var number = false

    var body: some View {
        TextField(hint, text: $text, axis: .vertical)
            .font(.system(size: 16)).foregroundStyle(Ink.black).lineLimit(1...4)
            .textInputAutocapitalization(capitals ? .characters : .sentences)
            .autocorrectionDisabled(capitals || number)
            .keyboardType(number ? .numberPad : .default)
            .padding(.horizontal, 16).padding(.vertical, 12).frame(minHeight: 54)
            .background(Ink.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }
}

struct TypeCode: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let close: () -> Void
    @State private var code = ""

    var body: some View {
        Plain(title: L.s("type_code_title"), text: L.s("type_code_text"), insets: insets, close: close) {
            TextField(L.s("type_code_hint"), text: $code)
                .font(.system(size: 16)).foregroundStyle(Ink.black)
                .textInputAutocapitalization(.characters).autocorrectionDisabled().keyboardType(.asciiCapable)
                .padding(.horizontal, 16).frame(minHeight: 54)
                .background(Ink.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .padding(.top, 16)
            Pill(title: L.s("type_code_pair"), kind: .black, onClick: code.trimmingCharacters(in: .whitespaces).isEmpty ? nil : { model.pair(code) })
                .padding(.top, 14)
        }
    }
}

/// MobileScreens.swift ProfileSheet with this phone's switches working (PHN-8, PHN-420, PHN-422, PHN-811, PHN-566).
struct Profile: View {
    @Bindable var model: AppModel
    let shell: AppShell
    let insets: PhoneInsets
    let close: () -> Void

    var body: some View {
        let s = model.settings
        // the Mac's own date for this phone's pairing, else the day this phone first saw it paired
        let pairedAt: Date? = model.ui.mac?.pairedAt.flatMap { ISO8601DateFormatter().date(from: $0) }
            ?? (s.pairedSince > 0 ? Date(timeIntervalSince1970: TimeInterval(s.pairedSince) / 1000) : nil)
        let since = pairedAt?.formatted(date: .long, time: .omitted) ?? "—"
        ProfileSheet(macName: model.ui.mac?.name ?? L.s("profile_mac_name"), pairedSince: since, insets: insets,
                     lockOn: true,   // PHN-8: FERB always opens with Face ID, Touch ID or the passcode; there is no off
                     onClose: close, onForget: { model.show(.forget) },
                     more: AnyView(VStack(spacing: 0) {
                         Divide()
                         CardRow(title: L.s("profile_screenshots"),
                                 // says honestly whether this iOS keeps the pictures of the Mac out of screenshots (PHN-811)
                                 detail: L.s(ScreenshotShield.available ? "profile_screenshots_detail_shield" : "profile_screenshots_detail_no_shield")) { RowIcon(symbol: "eye") } trailing: {
                             PhoneSwitch(on: s.blockScreenshots, onChange: { model.blockScreenshots($0) }, label: L.s("profile_screenshots"))
                         }
                         Divide()
                         CardRow(title: L.s("profile_lock_screen"), detail: L.s("profile_lock_screen_detail")) { RowIcon(symbol: "bell") } trailing: {
                             PhoneSwitch(on: s.hideChecks, onChange: { model.hideChecks($0) }, label: L.s("profile_lock_screen"))
                         }
                         Divide()
                         CardRow(title: L.s("profile_city"), detail: L.s("profile_city_detail")) { RowIcon(symbol: "location") } trailing: {
                             PhoneSwitch(on: s.shareCity, onChange: { on in
                                 if on { model.show(.city) } else {
                                     shell.prefs.shareCity = false
                                     shell.sensors.cityStateChanged()
                                     model.refreshProfile()
                                 }
                             }, label: L.s("profile_city"))
                         }
                         if model.ui.newKeys {
                             Divide()
                             CardRow(title: L.bio("keys_row"), detail: L.bio("keys_row_detail"),
                                     onClick: model.ui.busy.contains("keys") ? nil : { model.registerKeys() }) { RowIcon(symbol: Words.bioSymbol) } trailing: { Opens() }
                         }
                     }),
                     lockTitle: L.s("profile_lock").replacingOccurrences(of: "%@", with: Words.bioTitle),
                     instead: (title: L.s("background_title"), detail: L.s("background_detail"), onClick: { model.show(.background) }),
                     // PHN-656: no switch that does nothing: "Always on", and a tap says why
                     lockAlways: L.s("profile_lock_always"), onLockInfo: { model.note(Note("profile_lock_why")) },
                     // PHN-801: the connection's row (the top bar's badge); its sheet keeps Try again, the relay and the Mac tab
                     connection: AnyView(ConnectionRow(mark: linkMark(model.ui.link, lost: model.ui.linkLost),
                                                       state: linkStateWords(linkMark(model.ui.link, lost: model.ui.linkLost)),
                                                       path: linkPathWords(linkMark(model.ui.link, lost: model.ui.linkLost)),
                                                       onClick: { model.show(.connection) })))
    }
}

/// Watch live (PHN-454, PHN-810): the task's window on the Mac as FERB.app's preview shows it, full screen, with
/// FERB's line and Stop. The frame lives in memory only; screenshots and captures as the owner's switch says (PHN-811).
struct LiveView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        if let v = model.live.view {
            let image = v.frame.flatMap { UIImage(data: $0) }.map { Image(uiImage: $0) }
            let placeholder = v.ended?.text ?? v.empty ?? L.s("live_waiting")
            let line = image != nil ? (v.ended?.text ?? v.line) : (v.ended == nil ? v.line : nil)
            ZStack(alignment: .bottom) {
                LivePage(title: L.s("live_title"), frame: image, placeholder: placeholder, line: line,
                         closeTitle: L.s("live_close"), stopTitle: L.s("live_stop"), insets: insets,
                         onClose: { model.closeLive() }, onStop: { model.stop(v.taskId) },
                         // the picture of the Mac out of screenshots while the owner's switch is on (PHN-810, PHN-811)
                         shieldFrame: { AnyView($0.screenshotShielded()) })
                if v.ended != nil {
                    Pill(title: L.s("live_again"), kind: .grey, symbol: "eye", height: 46) { model.watchAgain() }
                        .padding(.horizontal, 16).padding(.bottom, insets.bottom + 62)
                }
            }
            // opened over the page, it rises and fades in, and fades out (kb PHN-623)
            .presenceFade()
        }
    }
}

/// The lab's "FERB's settings" row as a plain sheet (Android's Settings.kt): the switch and the day counts change
/// from here, one Face ID per change; the two lists are shown as they are (PHN-404, PHN-133).
struct SettingsView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let close: () -> Void

    var body: some View {
        let rows = model.ui.settings
        let plain = (rows ?? []).filter { $0.spec.kind != .days }
        let days = (rows ?? []).filter { $0.spec.kind == .days }
        SettingsSheet(title: L.s("settings_title"), rows: plain.map(row), groupTitle: days.isEmpty ? nil : L.s("settings_how_long"),
                      groupRows: days.map(row), note: L.bio("settings_note"), loading: rows == nil ? L.s("settings_loading") : nil,
                      insets: insets, onClose: close)
    }

    private func row(_ r: MacSettingRow) -> SettingRow {
        let busy = model.ui.busy.contains("setting:\(r.spec.key)")
        switch r.spec.kind {
        case .toggle:
            return .toggle(title: r.spec.title, symbol: "eye", on: r.on, onChange: busy ? nil : { model.setSwitch(r.spec.key, on: $0) })
        case .list:
            return .list(title: r.spec.title, symbol: "lock", detail: r.items.isEmpty ? L.s("set_none") : r.items.joined(separator: ", "))
        case .days:
            return .value(title: r.spec.title, value: r.days.map { L.s("set_days_value", $0) }, onClick: busy ? nil : { model.show(.days(key: r.spec.key)) })
        }
    }
}

/// One day count: typed, then saved after one Face ID.
struct DaysView: View {
    @Bindable var model: AppModel
    let key: String
    let insets: PhoneInsets
    @State private var text: String?

    var body: some View {
        if let row = model.ui.settings?.first(where: { $0.spec.key == key }) {
            let back = { model.show(.settings) }
            let value = Binding(get: { text ?? row.days.map(String.init) ?? "" },
                                set: { text = String($0.filter(\.isNumber).prefix(5)) })
            Plain(title: row.spec.title, text: L.bio("set_days_text"), insets: insets, close: back) {
                Field(text: value, hint: L.s("set_days_hint"), number: true).padding(.top, 16)
                Pill(title: L.s("set_days_save"), kind: .black,
                     onClick: model.ui.busy.contains("setting:\(key)") || value.wrappedValue.isEmpty ? nil : { model.setDays(key, text: value.wrappedValue) })
                    .padding(.top, 14)
                Pill(title: L.s("cancel"), kind: .plain, height: 44, onClick: back).padding(.top, 4)
            }
        }
    }
}
