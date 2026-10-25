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
                SecureField("Paste your key", text: $key)
                Button("Save") {
                    let entered = key
                    key = ""                                   // the field never keeps it
                    Task { await model.saveAPIKey(provider, key: entered) }
                }
                .disabled(key.isEmpty)
            }
            if let line = model.aiMessage { Text(line).font(.callout) }
        }
    }
}

struct PermissionsStepView: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            StepHeader(title: "Let FERB work on your Mac", lines: ["FERB needs these two to see and use your Mac.",
                                                                   "It asks for anything else the first time it needs it."])
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
                Button("Grant") { Task { await model.grant(row) } }.disabled(!row.blocked.isEmpty)
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
                "Add it where you want FERB's help; FERB pairs a profile by itself when you're in it."])
            if let snapshot = model.browsers {
                if snapshot.browsers.isEmpty {
                    Text("No Chrome-family browser has been opened on this Mac yet.").foregroundStyle(.secondary)
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

struct PersonaStep: View {
    @ObservedObject var model: SetupModel
    @State private var words = ""
    @State private var editing = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "Getting to know you", lines: ["FERB looked at which apps you use, on this Mac only.",
                                                             "Save what it found, say it in your own words, or skip."])
            switch model.persona {
            case .loading, .scanning:
                ProgressView("Looking at your apps…")
            case .pending(_, let reason, let proposal, let canSave):
                if !reason.isEmpty { Text(reason) }
                if !proposal.isEmpty { Text(proposal).font(.callout).textSelection(.enabled) }
                if editing {
                    TextField("How you use this Mac, in your words", text: $words, axis: .vertical).lineLimit(3...6)
                    HStack {
                        Button("Save") {
                            let text = words
                            Task { await model.editPersona(text) }
                        }
                        .disabled(words.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        Button("Cancel") { editing = false }
                    }
                } else {
                    HStack {
                        if canSave { Button("Save") { Task { await model.savePersona() } } }
                        Button("Edit") { editing = true }
                        Button("Skip") { Task { await model.skipPersona() } }
                    }
                }
            case .finished:
                Text("Done. FERB keeps this in its memory; you can change it later.").foregroundStyle(.secondary)
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
                    SecureField("Code from the page (only if it shows one)", text: $code)
                    Button("Send") {
                        let entered = code
                        code = ""                              // the field never keeps it
                        Task { await model.sendClaudeCode(entered) }
                    }
                    .disabled(code.isEmpty)
                }
            }
        }
    }
}
