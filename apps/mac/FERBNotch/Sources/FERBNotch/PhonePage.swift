import AppKit
import SwiftUI
import NotchCore

/// Settings → Phone (stage 3 slice S5, kb T-1205): pair a phone (the code drawn as PHN-142 says, then the owner's Yes
/// with the six digits both screens show), the paired phones (rename, remove, the 30-day warning, the limit), what
/// phones did, and the owner's own relay (PHN-205, PHN-219, PHN-453) with the how-to the shared relay's message points
/// to (PHN-418). Functional; the owner polishes it. Every word is in `PhoneWords`.
struct PhoneSettingsPage: View {
    @ObservedObject var model: PhoneSettingsModel
    @State private var renaming: String?
    @State private var newName = ""
    @State private var removing: PhoneEntry?
    @State private var byAddress = false

    var body: some View {
        WindowPage {
            if let problem = model.problem {
                Label(problem, systemImage: "exclamationmark.circle").foregroundStyle(.red).textSelection(.enabled)
            }
            if let note = model.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
            if let screen = model.screenUse, screen.active { untilBack(screen) }
            WindowSettingsSection(title: "Pair a phone") { pairing }
            WindowSettingsSection(title: "Paired phones") { phoneList }
            if let folder = model.uploadFolder {
                WindowSettingsSection(title: PhoneWords.filesTitle) { files(folder) }
            }
            WindowSettingsSection(title: "Recent activity") { activity }
            WindowSettingsSection(title: "Use my own relay") { relay }
            WindowSettingsSection(title: PhoneWords.howToTitle) { howTo }
        }
        .task { await model.load() }
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                await model.tick()
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .ferbPhoneEvent)) { note in
            guard let topic = note.userInfo?["topic"] as? String else { return }
            let data = note.userInfo?["data"] as? [String: Any] ?? [:]
            Task { await model.receive(topic: topic, data: data) }
        }
        .alert(removing.map { PhoneWords.removeTitle($0.name) } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
               presenting: removing) { phone in
            Button("Remove", role: .destructive) { Task { await model.remove(phone.id) } }
            Button("Cancel", role: .cancel) { removing = nil }
        } message: { _ in
            Text(PhoneWords.removeMessage)
        }
    }

    // MARK: "Until I'm back at my Mac" (GATE-615)

    @ViewBuilder private func untilBack(_ screen: ScreenUse) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Label(PhoneWords.screenUntilBack(screen.phoneName), systemImage: "display")
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 12)
                Button(PhoneWords.askAgain) { Task { await model.endScreenUse() } }
            }
            Text(PhoneWords.screenUntilBackEnds).font(.system(size: 12)).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .contain)
    }

    // MARK: Files from your phone (PHN-580, PHN-581)

    @ViewBuilder private func files(_ folder: UploadFolder) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(PhoneWords.filesLandIn(folder.shown)).fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
                .help(folder.path)
            Spacer(minLength: 12)
            Button(PhoneWords.chooseFolder) { chooseFolder(from: folder) }
            if !folder.isDefault {
                Button(PhoneWords.useDefaultFolder) { Task { await model.resetUploadFolder() } }
            }
        }
    }

    private func chooseFolder(from folder: UploadFolder) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = PhoneWords.chooseFolderPrompt
        panel.message = PhoneWords.chooseFolderMessage
        panel.directoryURL = URL(fileURLWithPath: folder.path)
        guard AppWindowPresentation.runModal(panel) == .OK, let url = panel.url else { return }
        Task { await model.chooseUploadFolder(url.path) }
    }

    // MARK: Pair a phone

    @ViewBuilder private var pairing: some View {
        switch model.pairing {
        case .idle, .ended:
            if case let .ended(words) = model.pairing { Text(words).textSelection(.enabled) }
            if model.isFull {
                Text(PhoneWords.full(model.max)).foregroundStyle(.secondary)
            } else if !model.hasLink {
                Text(PhoneWords.noLinkInThisVersion).foregroundStyle(.secondary)
            } else {
                HStack(spacing: 24) {
                    Text(PhoneWords.pairIntro).font(.system(size: 12)).foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                    Button("Pair a phone") { Task { await model.startPairing() } }
                }
            }
        case .starting:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(PhoneWords.starting)
            }
        case let .showing(invite, _):
            HStack(alignment: .top, spacing: 20) {
                if !invite.rows.isEmpty { PairingCodeView(rows: invite.rows) }
                VStack(alignment: .leading, spacing: 8) {
                    Text(PhoneWords.scanThis)
                    Text(PhoneWords.validFor(model.secondsLeft)).foregroundStyle(.secondary)
                        .monospacedDigit()
                    Text(PhoneWords.orType).font(.system(size: 12)).foregroundStyle(.secondary)
                    Text(verbatim: invite.code).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                        .lineLimit(6)
                        .accessibilityLabel("Code to type in the phone app")
                        .accessibilityValue(invite.code)
                    HStack(spacing: 10) {
                        Button("Copy code") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(invite.code, forType: .string)
                        }
                        Button("New code") { Task { await model.startPairing() } }
                        Button("Cancel") { Task { await model.cancelPairing() } }
                            .keyboardShortcut(.cancelAction)
                    }
                }
            }
        case let .asking(request, _):
            VStack(alignment: .leading, spacing: 10) {
                Text(PhoneWords.wantsToPair(request.name)).font(.system(size: 14, weight: .semibold))
                Text(PhoneWords.checkCode)
                Text(verbatim: request.code).font(.system(size: 28, weight: .semibold, design: .monospaced))
                    .textSelection(.enabled)
                    .accessibilityLabel("Pairing code")
                    .accessibilityValue(request.code.map { String($0) }.joined(separator: " "))
                Text(PhoneWords.pairExplain).font(.system(size: 12)).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text(PhoneWords.answerWithin(model.secondsLeft)).font(.system(size: 12)).foregroundStyle(.secondary)
                    .monospacedDigit()
                HStack(spacing: 10) {
                    Button("Pair it") { Task { await model.decide(yes: true) } }
                        .keyboardShortcut(.defaultAction)
                    Button("Don't pair") { Task { await model.decide(yes: false) } }
                        .keyboardShortcut(.cancelAction)
                }
            }
            .accessibilityElement(children: .contain)
        }
    }

    // MARK: Paired phones

    @ViewBuilder private var phoneList: some View {
        if model.phones.isEmpty {
            Text(model.loaded && model.problem == nil ? PhoneWords.noPhones : " ").foregroundStyle(.secondary)
        }
        ForEach(model.phones) { phone in
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text(phone.name).font(.system(size: 13, weight: .semibold))
                    if let connection = phone.connection {
                        Text(connection.words).font(.system(size: 12)).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 12)
                    Button("Rename") { renaming = phone.id; newName = phone.name }
                    Button("Remove…", role: .destructive) { removing = phone }
                }
                Text(facts(phone)).font(.system(size: 12)).foregroundStyle(.secondary)
                if phone.unusedWarning {
                    Label(PhoneWords.unused(model.unusedDays), systemImage: "clock.badge.exclamationmark")
                        .font(.system(size: 12)).foregroundStyle(.orange)
                }
                if renaming == phone.id {
                    HStack(spacing: 10) {
                        TextField("Name", text: $newName).textFieldStyle(.roundedBorder).frame(width: 220)
                            .onSubmit { save(phone.id) }
                        Button("Save") { save(phone.id) }.disabled(newName.trimmingCharacters(in: .whitespaces).isEmpty)
                        Button("Cancel") { renaming = nil }
                    }
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(phone.name)
        }
        if model.isFull && !model.phones.isEmpty {
            Text(PhoneWords.full(model.max)).font(.system(size: 12)).foregroundStyle(.secondary)
        }
    }

    private func facts(_ phone: PhoneEntry) -> String {
        [phone.pairedAt.map(PhoneWords.pairedOn), phone.lastSeen.map(PhoneWords.lastSeen)].compactMap { $0 }
            .joined(separator: ", ")
    }

    private func save(_ id: String) {
        let name = newName
        renaming = nil
        Task { await model.rename(id, to: name) }
    }

    // MARK: Recent activity

    @ViewBuilder private var activity: some View {
        if model.activity.isEmpty {
            Text(PhoneWords.noActivity).foregroundStyle(.secondary)
        }
        let names = model.names
        ForEach(model.activity.prefix(20)) { row in
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(row.line(names: names)).font(.system(size: 12))
                Spacer(minLength: 12)
                if let at = row.at {
                    Text(at.formatted(date: .abbreviated, time: .shortened)).font(.system(size: 11))
                        .foregroundStyle(.secondary).monospacedDigit()
                }
            }
            .accessibilityElement(children: .combine)
        }
    }

    // MARK: Use my own relay

    @ViewBuilder private var relay: some View {
        if let link = model.link {
            Text(link.line).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            if let handover = link.handover, handover.phonesWaiting > 0 {
                Text(handover.closed ? PhoneWords.phonesLeftBehind(handover.phonesWaiting)
                                     : PhoneWords.phonesToCome(handover.phonesWaiting, until: handover.closesAt))
                    .font(.system(size: 12)).foregroundStyle(handover.closed ? .orange : .secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let address = link.ownRelay {
                HStack(spacing: 12) {
                    Text(PhoneWords.savedRelay(address)).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                    Spacer(minLength: 12)
                    Button("Stop using my relay") { Task { await model.stopUsingRelay() } }
                }
                Toggle(PhoneWords.always, isOn: Binding(get: { link.alwaysUseRelay },
                                                       set: { on in Task { await model.setAlwaysUseRelay(on) } }))
                Text(PhoneWords.alwaysExplain).font(.system(size: 12)).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } else if let problem = model.linkProblem {
            Text(problem).foregroundStyle(.secondary)
        }
        if model.hasLink {
            Text(PhoneWords.pasteIntro)
            if byAddress {
                TextField(PhoneWords.addressField, text: $model.relayAddress).textFieldStyle(.roundedBorder)
                    .accessibilityLabel("Relay address")
                SecureField(PhoneWords.tokenField, text: $model.relayToken).textFieldStyle(.roundedBorder)
                    .accessibilityLabel(PhoneWords.tokenField)
            } else {
                SecureField(PhoneWords.relayLinkField, text: $model.relayLink).textFieldStyle(.roundedBorder)
                    .accessibilityLabel("Relay line from the setup")
            }
            Toggle(PhoneWords.useAddressInstead, isOn: $byAddress)
                .onChange(of: byAddress) { _, _ in model.relayLink = ""; model.relayAddress = ""; model.relayToken = "" }
            HStack(spacing: 10) {
                Button(model.relayBusy ? "Testing…" : "Test") { Task { await model.testRelay() } }
                    .disabled(model.relayEntry.isEmpty || model.relayBusy)
                Button("Use this relay") { Task { await model.useRelay() } }
                    .disabled(model.relayEntry.isEmpty || model.relayBusy)
                if model.relayBusy { ProgressView().controlSize(.small) }
            }
            switch model.relayResult {
            case let .reachable(lines):
                Label(PhoneWords.reachable, systemImage: "checkmark.circle").foregroundStyle(.green)
                ForEach(lines, id: \.self) { Text($0).font(.system(size: 12)).foregroundStyle(.secondary) }
            case let .saved(lines):
                Label(PhoneWords.savedNow, systemImage: "checkmark.circle").foregroundStyle(.green)
                ForEach(lines, id: \.self) { Text($0).font(.system(size: 12)).foregroundStyle(.secondary) }
            case let .unreachable(words):
                Label(words, systemImage: "exclamationmark.circle").foregroundStyle(.orange).textSelection(.enabled)
            case nil:
                EmptyView()
            }
            Text(PhoneWords.relayExplain).font(.system(size: 12)).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: The how-to (PHN-418)

    @ViewBuilder private var howTo: some View {
        Text(PhoneWords.howToIntro).fixedSize(horizontal: false, vertical: true)
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(PhoneWords.howToSteps.enumerated()), id: \.offset) { index, step in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("\(index + 1).").monospacedDigit().foregroundStyle(.secondary)
                    Text(step).font(.system(size: 12)).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                }
                .accessibilityElement(children: .combine)
            }
        }
        Text(PhoneWords.howToCheck).font(.system(size: 12)).foregroundStyle(.secondary).textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
        if let script = Self.bundledScript {
            Button(PhoneWords.showScript) { NSWorkspace.shared.activateFileViewerSelecting([script]) }
        } else {
            Text(PhoneWords.scriptInSource).font(.system(size: 12)).foregroundStyle(.secondary).textSelection(.enabled)
        }
    }

    /// The relay's setup script when FERB.app carries it (Contents/Resources/relay/setup.sh); nil in a build without.
    static var bundledScript: URL? { Bundle.main.url(forResource: "setup", withExtension: "sh", subdirectory: "relay") }
}

/// The pairing code as PHN-142 draws it: black on white whatever the appearance, a four-module white margin, whole
/// points per module (each a sharp square), about 320 points wide. `rows` are the link's own matrix (`1` is dark), so
/// every surface shows the same code.
struct PairingCodeView: View {
    let rows: [String]
    static let quiet = 4

    /// The largest whole-point module that keeps the code within `fit` points, never under 2.
    static func module(_ count: Int, fit: CGFloat = 320) -> CGFloat {
        max(2, (fit / CGFloat(count + 2 * quiet)).rounded(.down))
    }

    var body: some View {
        let unit = Self.module(rows.count), side = unit * CGFloat(rows.count + 2 * Self.quiet)
        Canvas(rendersAsynchronously: false) { context, _ in
            context.withCGContext { cg in
                cg.setShouldAntialias(false)
                cg.interpolationQuality = .none
                cg.setFillColor(CGColor(gray: 1, alpha: 1))
                cg.fill(CGRect(x: 0, y: 0, width: side, height: side))
                cg.setFillColor(CGColor(gray: 0, alpha: 1))
                for (y, row) in rows.enumerated() {
                    for (x, cell) in row.utf8.enumerated() where cell == UInt8(ascii: "1") {
                        cg.fill(CGRect(x: CGFloat(x + Self.quiet) * unit, y: CGFloat(y + Self.quiet) * unit,
                                       width: unit, height: unit))
                    }
                }
            }
        }
        .frame(width: side, height: side)
        .environment(\.colorScheme, .light)
        .accessibilityElement()
        .accessibilityLabel("Pairing code to scan with your phone")
        .accessibilityAddTraits(.isImage)
    }
}

extension Notification.Name {
    /// A bus event of the phone area reached FERB.app (`userInfo`: topic, data); Settings → Phone reads it.
    static let ferbPhoneEvent = Notification.Name("FERBPhoneEvent")
}

@MainActor
extension NotchModel {
    /// The phone area's bus events (journal 106). Settings → Phone follows each; a phone asking to pair also reaches the
    /// owner wherever he is: the notch opens with a line naming the phone and its code and a button to Settings → Phone,
    /// where the Yes is given. Nothing here answers it.
    func receivePhoneEvent(topic: String, data: [String: Any]) {
        NotificationCenter.default.post(name: .ferbPhoneEvent, object: nil, userInfo: ["topic": topic, "data": data])
        // GATE-618: the quiet "Are you back?" line, turned into the welcome in place; no sound, no notch opening
        // APP-1107: shown above the composer, never kept in the chat
        if topic == "screen.until_back" {
            var next = alerts
            if ScreenBackLine.apply(data, to: &next) { changeAlerts { $0 = next } }
            return
        }
        guard topic == "phone.pair_requested", let request = data["request_id"] as? String ?? data["phone"] as? String else { return }
        let name = (data["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? PhoneWords.unnamedPhone
        let card = ResultCard(["type": "page", "page": "phone", "line": PhoneWords.openPhoneSettings])
        showOwnAlert(id: "phone-pair-" + request, text: PhoneWords.notchAsks(name), card: card)
        announceAttention("phone-pair:" + request)
    }
}
