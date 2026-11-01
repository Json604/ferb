import SwiftUI
import NotchCore

/// FERB.app's setup window (L34 C): one title, two lines, one primary button, progress dots. Bare minimum on purpose;
/// the owner styles it later.
struct SetupView: View {
    @ObservedObject var model: SetupModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            switch model.step {
            case .welcome: WelcomeStep(model: model)
            case .ai: AIStepView(model: model)
            case .permissions: PermissionsStepView(model: model)
            case .browsers: BrowsersStepView(model: model)
            case .identity: IdentityStepView(model: model)
            case .persona: PersonaStep(model: model)
            case .done: DoneStep()
            }
            Spacer(minLength: 0)
            if let message = model.message { Text(message).font(.callout).foregroundStyle(.secondary) }
            HStack {
                SetupDots(current: model.step)
                Spacer()
                Button(model.primaryTitle) { Task { await model.primary() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(model.primaryDisabled)
            }
        }
        .padding(24)
        .frame(width: 560, height: 500)
    }
}

struct SetupDots: View {
    let current: SetupStep
    var body: some View {
        HStack(spacing: 6) {
            ForEach(SetupStep.allCases, id: \.rawValue) { step in
                Circle().fill(step == current ? Color.primary : Color.secondary.opacity(0.35)).frame(width: 7, height: 7)
            }
        }
    }
}

struct StepHeader: View {
    let title: String
    let lines: [String]
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.title2).bold()
            ForEach(lines, id: \.self) { Text($0).foregroundStyle(.secondary) }
        }
    }
}

struct WelcomeStep: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "FERB works on your Mac in the background", lines: [
                "What FERB keeps (its memory, history and settings) stays on this Mac, in ~/.ferb.",
                "What FERB sends to the AI service you choose is redacted first: passwords, codes and keys never leave."])
            if model.serviceRunning {
                ProgressView()
                Text(model.serviceLine).font(.callout).lineLimit(2)
            }
            if let error = model.serviceError {
                Text("FERB couldn't set up its background service:").bold()
                ScrollView { Text(error).font(.system(.callout, design: .monospaced)).textSelection(.enabled) }
                    .frame(maxHeight: 140)
                if model.serviceNeedsApproval {
                    Button("Open Login Items Settings") { model.openLoginItems() }
                }
            }
        }
    }
}

struct AIStepView: View {
    @ObservedObject var model: SetupModel
    @State private var provider: APIProvider = .anthropic
    @State private var key = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "Choose your AI", lines: ["FERB thinks with an AI service you already use.",
                                                        "Pick one; you can change it later."])
            if let current = model.ai?.current { Text("FERB uses: \(AISnapshot.label(current))").font(.callout) }
            HStack {
                Button(model.signingIn ? "Waiting for your browser…" : "Sign in with ChatGPT") { model.signInWithChatGPT() }
                    .disabled(model.signingIn)
                if model.signingIn { Button("Cancel") { model.cancelSignIn() } }
                if model.ai?.usable("codex") == true { Text("Signed in").foregroundStyle(.secondary) }
            }
            ClaudeSignInRow(model: model)
            HStack {
                Picker("API key", selection: $provider) {
                    ForEach(APIProvider.allCases) { Text($0.label).tag($0) }
                }
                .frame(width: 190)
                SecureField("Paste your key", text: $key).onSubmit(saveKey)     // Enter saves (2026-10-27)
                Button("Save", action: saveKey).disabled(key.isEmpty)
            }
            if let line = model.aiMessage { Text(line).font(.callout) }
        }
    }

    private func saveKey() {
        guard !key.isEmpty else { return }
        let entered = key
        key = ""                                               // the field never keeps it
        Task { await model.saveAPIKey(provider, key: entered) }
    }
}

struct PermissionsStepView: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            StepHeader(title: "Let FERB work on your Mac", lines: ["Allow these once and FERB has everything it needs.",
                                                                   "Each row ticks itself when it's on."])
            if !SetupModel.allowAllOrder(model.setupRows).isEmpty {
                Button(model.allowingAll ? "Allowing… flip each switch as it opens" : "Allow all") {
                    Task { await model.allowAll() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(model.allowingAll)
            }
            if model.notificationHint {
                Text("Notifications: click Allow on FERB's banner at the top right of your screen (under Options if "
                     + "it shows no Allow).").font(.caption).foregroundStyle(.secondary)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(model.setupRows) { row in PermissionRowView(row: row, model: model) }
                }
            }
            if model.offersRestart {
                Button(model.restarting ? "Restarting FERB…" : "Switched it on? Restart FERB to check") {
                    Task { await model.restartService() }
                }
                .disabled(model.restarting)
            }
            if model.showsFullDiskHint, let target = model.permissions?.grantTarget {
                Text("Full Disk Access: drag FERB from the Finder window into the list.\n\(target)")
                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await model.cameBack() }
        }
    }
}

struct PermissionRowView: View {
    let row: PermissionRow
    @ObservedObject var model: SetupModel
    var body: some View {
        HStack(alignment: .top) {
            Text(row.status == .granted ? "✓" : "○").frame(width: 16)
            VStack(alignment: .leading, spacing: 2) {
                Text(PermissionCopy.label(row.id))
                Text(row.blocked.isEmpty ? PermissionCopy.reason(row.id) : row.blocked)
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if row.status != .granted {
                VStack(alignment: .trailing, spacing: 2) {
                    Button("Grant") { Task { await model.grant(row) } }.disabled(!row.blocked.isEmpty)
                    if model.askedRows.contains(row.id) && row.id != "full_disk_access" && row.process == "daemon" {
                        Button("Not in the list? Show FERB to drag in", action: model.showAgentApp)
                            .buttonStyle(.link).font(.caption)
                    }
                }
            }
        }
    }
}

struct BrowsersStepView: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            StepHeader(title: "Browsers", lines: [
                "FERB works in your browsers through its extension.",
                "Add it where you want FERB's help and turn it on, then pair: all at once, or one profile at a time."])
            if let snapshot = model.browsers {
                if snapshot.browsers.isEmpty {
                    Text("No Chrome-family browser has been opened on this Mac yet.").foregroundStyle(.secondary)
                }
                if snapshot.browsers.contains(where: { $0.profiles.contains { $0.state == .on && !$0.ferbIdentity } }) {
                    HStack {
                        Button("Pair all") { Task { await model.pairAll() } }.disabled(model.pairing)
                        if model.pairing { ProgressView().controlSize(.small) }
                    }
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(snapshot.browsers) { browser in
                            Text(browser.label).bold()
                            ForEach(browser.profiles) { profile in
                                HStack {
                                    Text(profile.name + (profile.ferbIdentity ? " (FERB's own)" : ""))
                                    Spacer()
                                    Text(profile.state.text).foregroundStyle(.secondary)
                                    if profile.state.canAdd {
                                        Button("Add to \(browser.label)") { Task { await model.addExtension(profile) } }
                                    }
                                    if profile.state == .on && !profile.ferbIdentity {
                                        Button("Pair") { Task { await model.pairProfile(profile) } }
                                            .disabled(model.pairing)
                                    }
                                }
                            }
                            if browser.manual {
                                Text("Or load it unpacked from the browser's extensions page (Developer mode): \(snapshot.extensionDir)")
                                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                            }
                        }
                    }
                }
            } else {
                ProgressView()
            }
        }
    }
}

/// FERB's own Google account (APP-1, optional; also Settings → Browsers): the owner makes the account and signs it
/// into a browser profile of its own; FERB finds that profile and pairs it. Skipping it, the owner picks one of their
/// paired profiles for FERB to browse in. Bare minimum; the owner polishes it.
struct IdentityStepView: View {
    @ObservedObject var model: SetupModel
    var showsHeader = true

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if showsHeader {
                StepHeader(title: "Give FERB its own Google account (recommended)", lines: [
                    "Make a new Google account for FERB, then sign it into a browser profile of its own (nothing else signed in there).",
                    "FERB uses it for its own sign-ups and codes, so it never needs yours. You can skip this and add it later in Settings → Browsers."])
            }
            if let state = model.identity, state.isSetUp {
                LabeledContent("FERB's account", value: state.address)
                LabeledContent("Its profile", value: "\(state.browser) / \(state.profile)" + (state.paired ? " · paired" : " · not paired"))
                HStack {
                    if !state.paired { Button("Pair it") { Task { await model.pairIdentity() } } }
                    Button("Stop using it") { Task { await model.removeIdentity() } }
                }
            } else {
                HStack {
                    TextField("FERB's Google address", text: $model.identityAddress).onSubmit { Task { await model.refreshIdentity() } }
                    Button("Find its profile") { Task { await model.refreshIdentity() } }
                }
                ForEach(model.identity?.candidates ?? []) { candidate in
                    HStack {
                        Text("\(candidate.browserName): \(candidate.name)")
                        Spacer()
                        if candidate.only {
                            Button("Use this profile") { Task { await model.useIdentity(candidate) } }.disabled(model.identityBusy)
                        } else {
                            Text("also signed in to \(candidate.otherAccounts) other account(s)").foregroundStyle(.secondary)
                        }
                    }
                }
                if let state = model.identity, !model.identityAddress.isEmpty, state.candidates.isEmpty {
                    Text("No browser profile is signed into that account yet.").foregroundStyle(.secondary)
                }
                OwnerProfilePicker(model: model)
            }
            if model.identityBusy { ProgressView().controlSize(.small) }
            if let line = model.identityLine { Text(line).font(.callout).textSelection(.enabled) }
        }
    }
}

/// Without an account of its own, FERB browses as the owner in the paired profile they pick here.
struct OwnerProfilePicker: View {
    @ObservedObject var model: SetupModel

    var body: some View {
        let paired = (model.browsers?.browsers ?? []).flatMap { browser in
            browser.profiles.filter { $0.state == .paired && !$0.ferbIdentity }.map { (browser.label, $0) }
        }
        VStack(alignment: .leading, spacing: 6) {
            Text("Without its own account, FERB browses as you in:").font(.callout)
            Picker("Profile", selection: Binding(get: { model.identity?.ownerProfile ?? "" },
                                                 set: { value in Task { await model.browseAsOwner(in: value) } })) {
                Text("A profile I name in each request").tag("")
                ForEach(paired, id: \.1.id) { label, profile in Text("\(label): \(profile.name)").tag(profile.id) }
            }
            .labelsHidden()
        }
        .task { if model.browsers == nil { await model.refreshBrowsers() } }
    }
}

struct PersonaStep: View {
    @ObservedObject var model: SetupModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "Getting to know you", lines: ["FERB looked at which apps you use, on this Mac only."])
            switch model.persona {
            case .loading, .scanning:
                ProgressView("Looking at your apps…")
            case .seen(let summary):
                Text(summary).textSelection(.enabled)
                Text("Tell FERB if any of it is wrong; you can see and change it on the Memory page.")
                    .foregroundStyle(.secondary)
            case .finished:
                Text("Done.").foregroundStyle(.secondary)
            case .failed(let why):
                Text("FERB couldn't look at your apps: \(why)").foregroundStyle(.secondary)
            }
            if let line = model.personaMessage { Text(line).font(.callout) }
        }
    }
}

struct DoneStep: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "FERB is ready", lines: ["FERB lives in the notch at the top of your screen.",
                                                       "Three things to know:"])
            Text("• Double-tap the right Option key to open FERB, or click the notch.")
            Text("• Hold right Command + Option and speak to ask by voice.")
            Text("• Close the notch and keep working: FERB tells you when it needs you or finishes.")
        }
    }
}

struct ClaudeSignInRow: View {
    @ObservedObject var model: SetupModel
    @State private var code = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Button(model.claudeSigningIn ? "Waiting for your browser…" : "Sign in with Claude") {
                    Task { await model.signInWithClaude() }
                }
                .disabled(model.claudeSigningIn)
                if model.claudeSigningIn { Button("Cancel") { Task { await model.cancelClaudeSignIn() } } }
                if model.ai?.signedIn("claude") == true && !model.claudeSigningIn {
                    Text("Signed in").foregroundStyle(.secondary)
                    if model.ai?.current != "claude" { Button("Use Claude") { Task { await model.useClaude() } } }
                }
            }
            if model.claudeSigningIn {
                if model.claudeSignIn?.url != nil {
                    Button("Open the sign-in page") { model.openClaudeSignInPage() }
                }
                HStack {
                    SecureField("Code from the page (only if it shows one)", text: $code).onSubmit(sendCode)
                    Button("Send", action: sendCode).disabled(code.isEmpty)
                }
            }
        }
    }

    private func sendCode() {
        guard !code.isEmpty else { return }
        let entered = code
        code = ""                                              // the field never keeps it
        Task { await model.sendClaudeCode(entered) }
    }
}
