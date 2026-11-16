import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import UIKit

/// The sheet over the page (Mobile.swift Sheet; Android's Sheets.kt), or nothing.
struct SheetsView: View {
    @Bindable var model: AppModel
    let shell: AppShell
    let insets: PhoneInsets

    var body: some View {
        let close = { model.show(nil) }
        switch model.sheet {
        case nil, .scan:
            EmptyView()   // the scanner is a page of its own (Root's ScanPage)
        case .asking(let id):
            if let q = model.ui.questions.first(where: { $0.approvalId == id }) {
                let busy = model.ui.busy.contains("answer:\(q.approvalId)")
                if q.onlyOnMac || q.held {
                    let parts = [q.reason.isEmpty ? nil : q.reason, q.needs?.problem, q.onlyOnMac ? L.s("question_on_mac") : nil].compactMap { $0 }
                    Plain(title: q.title ?? L.s("question_default_title"),
                          text: parts.isEmpty ? L.s("n_held_text") : parts.joined(separator: "\n\n"), insets: insets, close: close)
                } else if q.hidden {
                    PrivateAnswer(model: model, q: q, insets: insets, busy: busy, close: close)
                } else {
                    Asking(model: model, q: q, insets: insets, busy: busy, close: close)
                }
            }
        case .relay:
            RelaySheet(copy: LabSample.relay, insets: insets, onClose: close)
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
        case .notYet(let f):
            Plain(title: L.s(f.wordsKey), text: L.s(f.textKey), insets: insets, close: close)
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

/// MobileScreens.swift QuestionSheet: the question, why, the exact thing, and its answers. Two answers are the lab's
/// No and Yes; more are a pill each; none is the owner's own words. "Yes asks for Face ID" only where the Mac's status
/// says the Yes takes the strong key (PHN-470, PHN-520).
struct Asking: View {
    @Bindable var model: AppModel
    let q: Question
    let insets: PhoneInsets
    let busy: Bool
    let close: () -> Void
    @State private var text = ""

    var body: some View {
        let title = q.title ?? L.s("question_default_title")
        let choices = q.choices
        if choices.count == 2 {
            let no = choices[0], yes = choices[1]
            let note = q.needsProof && q.needs?.fingerprint == true ? L.bio("question_yes_fingerprint") : nil
            QuestionSheet(symbol: "questionmark.circle", title: title, reason: q.reason, yes: yes.label, no: no.label, note: note,
                          insets: insets, onNo: busy ? nil : { model.answer(q.approvalId, no.input) },
                          onYes: busy ? nil : { model.answer(q.approvalId, yes.input) }, onDismiss: close) {
                if let e = q.exact { ExactCommand(e) }
            }
        } else {
            Sheet(bottomInset: insets.bottom, onDismiss: close) {
                HStack(alignment: .top, spacing: 14) {
                    SheetIcon(symbol: "questionmark.circle")
                    VStack(alignment: .leading, spacing: 4) {
                        Text(title).type(22, .semibold)
                        Text(q.reason).type(15.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 0)
                }
                if let e = q.exact { ExactCommand(e).padding(.top, 14) }
                if !choices.isEmpty {
                    VStack(spacing: 10) {
                        ForEach(Array(choices.enumerated()), id: \.offset) { i, c in
                            Pill(title: c.label, kind: i == 0 ? .black : .grey, onClick: busy ? nil : { model.answer(q.approvalId, c.input) })
                        }
                    }
                    .padding(.top, 18)
                } else {
                    Field(text: $text, hint: L.s("question_answer_hint")).padding(.top, 18)
                    HStack(spacing: 10) {
                        Pill(title: L.s("question_skip"), kind: .grey, fill: false, onClick: busy ? nil : { model.answer(q.approvalId, .words(text: "")) })
                        Pill(title: L.s("question_send"), kind: .black, onClick: busy || text.trimmingCharacters(in: .whitespaces).isEmpty ? nil : {
                            model.answer(q.approvalId, .words(text: text.trimmingCharacters(in: .whitespacesAndNewlines)))
                            text = ""
                        })
                    }
                    .padding(.top, 12)
                }
            }
        }
    }
}

/// MobileScreens.swift SecretSheet (PHN-109, PHN-110, PHN-502): the value goes straight to the Mac on the sealed
/// path, signed after the prompt, and is kept nowhere: not in saved state, not in a log, and wiped from the field when
/// sent or when the sheet goes; while it shows the app is hidden from screen captures (mustSecure). A code takes the
/// lab's own keypad, so the system keyboard never sees it; a password takes a SecureField (no autocorrect, no
/// suggestions, kept out of keyboard learning by iOS).
struct PrivateAnswer: View {
    @Bindable var model: AppModel
    let q: Question
    let insets: PhoneInsets
    let busy: Bool
    let close: () -> Void
    @State private var digits: [UInt16] = []
    @State private var secret = ""
    private static let maxCode = 10

    var body: some View {
        let title = q.title ?? L.s("private_title_default")
        Group {
            if q.isCode {
                SecretSheet(title: title, detail: q.reason, filled: digits.count, length: max(6, min(Self.maxCode, digits.count + 1)),
                            insets: insets, onKey: { k in
                                if k == "⌫" { if !digits.isEmpty { digits[digits.count - 1] = 0; digits.removeLast() } }
                                else if digits.count < Self.maxCode, let u = k.utf16.first { digits.append(u) }
                            }, onDismiss: close,
                            actions: AnyView(actions(canSend: !digits.isEmpty) { model.answerHidden(q.approvalId, units: &digits) }))
            } else {
                Sheet(bottomInset: insets.bottom, onDismiss: close) {
                    HStack(spacing: 14) {
                        SheetIcon(symbol: "key")
                        VStack(alignment: .leading, spacing: 2) {
                            Text(title).type(19, .semibold)
                            if !q.reason.isEmpty { Text(q.reason).type(15, .regular, Ink.detail) }
                        }
                    }
                    SecureField(L.s("private_field"), text: $secret)
                        .font(.system(size: 18)).textContentType(.password).autocorrectionDisabled().textInputAutocapitalization(.never)
                        .padding(.horizontal, 16).frame(minHeight: 54)
                        .background(Ink.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .padding(.top, 16)
                    privateNote
                    actions(canSend: !secret.isEmpty) {
                        var units = Array(secret.utf16)
                        secret = ""
                        model.answerHidden(q.approvalId, units: &units)
                    }
                }
            }
        }
        .onDisappear { for i in digits.indices { digits[i] = 0 }; digits = []; secret = "" }
    }

    private var privateNote: some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: "lock.fill")
            Text("Private: it goes straight to your Mac and is not kept on this phone.")
        }
        .type(14, .regular, Ink.detail).padding(.top, 12)
    }

    private func actions(canSend: Bool, send: @escaping () -> Void) -> some View {
        HStack(spacing: 10) {
            Pill(title: L.s("private_decline"), kind: .grey, height: 46, fill: false, onClick: busy ? nil : {
                secret = ""
                model.answer(q.approvalId, .words(text: ""))
            })
            Pill(title: L.s("private_send"), kind: .black, height: 46, onClick: busy || !canSend ? nil : send)
        }
        .padding(.top, 14)
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

/// MobileScreens.swift ProfileSheet with this phone's switches working (PHN-8, PHN-420, PHN-422, PHN-565, PHN-566).
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
                         CardRow(title: L.s("profile_screenshots"), detail: L.s("profile_screenshots_detail")) { RowIcon(symbol: "eye") } trailing: {
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
                     instead: (title: L.s("background_title"), detail: L.s("background_detail"), onClick: { model.show(.background) }))
    }
}

/// Watch live (PHN-454, PHN-518): the task's window on the Mac as FERB.app's preview shows it, full screen, with
/// FERB's line and Stop. The frame lives in memory only; the app is hidden from screen captures while this shows
/// (mustSecure, whatever the owner's switch says).
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
                         onClose: { model.closeLive() }, onStop: { model.stop(v.taskId) })
                if v.ended != nil {
                    Pill(title: L.s("live_again"), kind: .grey, symbol: "eye", height: 46) { model.watchAgain() }
                        .padding(.horizontal, 16).padding(.bottom, insets.bottom + 62)
                }
            }
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
