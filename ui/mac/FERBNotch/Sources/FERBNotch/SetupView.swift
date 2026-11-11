import SwiftUI
import NotchCore

// Setup's step views, shown in Settings' pages (there is no setup window, APP-685), in the notch's black glass,
// typography and quiet controls.

struct SetupGlass: NSViewRepresentable {
    func makeNSView(context: Context) -> FerbGlassView {          // APP-785: progressive, heavy at the top
        let view = FerbGlassView()
        view.material = .hudWindow
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }
    func updateNSView(_ view: FerbGlassView, context: Context) {}
}

struct SetupActionStyle: ButtonStyle {
    var primary = false
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .padding(.horizontal, 16).padding(.vertical, 12)
            .foregroundStyle(primary ? Color.black : Color.white.opacity(0.9))
            .background(primary ? Color.white.opacity(0.94) : Color.white.opacity(0.07),
                        in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .opacity(enabled ? (configuration.isPressed ? 0.65 : 1) : 0.3)
            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

struct SetupFieldStyle: TextFieldStyle {
    func _body(configuration: TextField<Self._Label>) -> some View {
        configuration.textFieldStyle(.plain).padding(12)
            .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 10))
    }
}

struct StepHeader: View {
    let title: String
    let lines: [String]
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.system(size: 26, weight: .semibold)).tracking(-0.6)
            ForEach(lines, id: \.self) { Text($0).foregroundStyle(.secondary) }
        }
    }
}

struct AIStepView: View {
    @ObservedObject var model: SetupModel
    @State private var provider: APIProvider = .anthropic
    @State private var key = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            StepHeader(title: "A mind for your Mac.", lines: ["Connect the AI service you already use."])
            if model.daemonSilent { NotAnsweringLine() }
            VStack(alignment: .leading, spacing: 22) { cards }
                .opacity(model.daemonSilent ? 0.45 : 1)           // APP-884: what it shows may be out of date
        }
        .onChange(of: model.ai?.unsavedKeys) { _, unsaved in
            if let first = unsaved?.first, unsaved?.contains(provider) != true { provider = first }
        }
        // APP-882: a change made anywhere (the chat's one-tap switch, a call a service refused, a developer tool)
        // arrives as the daemon's runtimes.changed; coming back to the window reads again too
        .onReceive(NotificationCenter.default.publisher(for: .ferbRuntimesChanged)) { _ in
            Task { await model.refreshAI(changedElsewhere: true) }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await model.refreshAI(changedElsewhere: true) }
        }
    }

    @ViewBuilder private var cards: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 5) {
                    Text("ChatGPT").font(.system(size: 16, weight: .semibold))
                    Text("Connect through your ChatGPT account").font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer()
                RolesBadge(ai: model.ai, runtime: "codex")
            }
            RefusalLine(ai: model.ai, runtime: "codex")
            HStack {
                Button(model.signingIn ? "Waiting for your browser…" : "Sign in with ChatGPT") { model.signInWithChatGPT() }
                    .disabled(model.signingIn)
                if model.signingIn { Button("Cancel") { model.cancelSignIn() } }
                if model.ai?.usable("codex") == true && model.ai?.inUse("codex") != true && !model.signingIn {
                    Text("Signed in").foregroundStyle(.secondary)
                    Button("Use ChatGPT") { Task { await model.useChatGPT() } }
                }
            }
            if model.ai?.signedIn("codex") == true { ModelRow(model: model, runtime: "codex") }
        }
        .padding(18).frame(maxWidth: .infinity, alignment: .leading)
        .background(.white.opacity(0.035), in: RoundedRectangle(cornerRadius: 18))
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Claude").font(.system(size: 16, weight: .semibold))
                Spacer()
                RolesBadge(ai: model.ai, runtime: "claude")
            }
            RefusalLine(ai: model.ai, runtime: "claude")
            ClaudeSignInRow(model: model)
            if model.ai?.signedIn("claude") == true { ModelRow(model: model, runtime: "claude") }
        }
        .padding(18).frame(maxWidth: .infinity, alignment: .leading)
        .background(.white.opacity(0.035), in: RoundedRectangle(cornerRadius: 18))
        // every API key the Keychain holds gets its own card, like ChatGPT's and Claude's (owner 2026-11-08: "when i
        // chagen from anthropic or anu api key i cannot swithc bacj to it"): Use, Replace, Remove, its model
        ForEach(model.ai?.savedKeys ?? []) { saved in APIKeyCard(model: model, provider: saved) }
        if !(model.ai?.unsavedKeys ?? APIProvider.allCases).isEmpty {
            DisclosureGroup {
                VStack(alignment: .leading, spacing: 10) {
                    Picker("Provider", selection: $provider) {
                        ForEach(model.ai?.unsavedKeys ?? APIProvider.allCases) { Text($0.label).tag($0) }
                    }
                    HStack {
                        SecureField("Paste your API key", text: $key).onSubmit(saveKey)
                        Button("Save", action: saveKey).disabled(key.isEmpty)
                    }
                }.padding(.top, 10)
            } label: {
                Text("Other ways to connect: an API key")
            }.font(.system(size: 12)).tint(.white.opacity(0.6))
        }
        if let line = model.aiMessage { Text(line).font(.callout) }
        // APP-882: shown whenever a role runs anywhere, mixed connections included; each role on its own connection
        if let ai = model.ai, !ai.runtimesInUse.isEmpty { RoleModelsView(model: model, ai: ai) }
    }

    private func saveKey() {
        guard !key.isEmpty else { return }
        let entered = key
        key = ""                                               // the field never keeps it
        Task { await model.saveAPIKey(provider, key: entered) }
    }
}

/// A saved API key's card (APP-792): the provider, "Key saved" (never the key), Use (checked with one tiny call; the
/// service in use stays when it fails), Replace key, Remove key and its model.
struct APIKeyCard: View {
    @ObservedObject var model: SetupModel
    let provider: APIProvider
    @State private var replacing = false
    @State private var key = ""

    var body: some View {
        let inUse = model.ai?.inUse(provider.rawValue) == true
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 5) {
                    Text(provider.label).font(.system(size: 16, weight: .semibold))
                    Label("Key saved in your Keychain", systemImage: "key.fill")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer()
                RolesBadge(ai: model.ai, runtime: provider.rawValue)
            }
            RefusalLine(ai: model.ai, runtime: provider.rawValue)
            HStack {
                if !inUse { Button("Use \(provider.label)") { Task { await model.useAPIKey(provider) } } }
                Button(replacing ? "Cancel" : "Replace key") { replacing.toggle(); key = "" }
                Button("Remove key") { Task { await model.removeAPIKey(provider) } }
                    .disabled(inUse).help(inUse ? "Use another service first" : "Delete this key from your Keychain")
            }
            if replacing {
                HStack {
                    SecureField("Paste the new key", text: $key).onSubmit(save)
                    Button("Save", action: save).disabled(key.isEmpty)
                }
            }
            ModelRow(model: model, runtime: provider.rawValue).id(provider.rawValue)
        }
        .padding(18).frame(maxWidth: .infinity, alignment: .leading)
        .background(.white.opacity(0.035), in: RoundedRectangle(cornerRadius: 18))
    }

    private func save() {
        guard !key.isEmpty else { return }
        let entered = key
        key = ""                                               // the field never keeps it
        replacing = false
        Task { await model.saveAPIKey(provider, key: entered) }
    }
}

/// A connection's model (RUN-429) and effort (APP-881): the one in use, the connection's list (an API key's from its
/// provider) and "Other…" to type any model id; beside it the model's effort levels, "Default (<the model's default>)"
/// and "Other…" to type a level. One Use saves both; FERB checks them before it uses them and keeps the old ones when
/// they fail.
struct ModelRow: View {
    @ObservedObject var model: SetupModel
    let runtime: String
    /// A role's own model (APP-793, owner: "is it not smart based on the service connected adn show its option + a
    /// text field???"): the same picker, plus the connection's model; nil for the connection's own row.
    var role: String? = nil
    @State private var list: ModelList?
    @State private var choice = ""
    @State private var typed = ""
    @State private var effortChoice = ModelRow.byDefault
    @State private var typedEffort = ""
    @State private var line: String?
    @State private var busy = false
    private static let other = "\u{0}other"
    private static let connection = "\u{0}connection"
    private static let byDefault = "\u{0}default"

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Model").font(.system(size: 12)).frame(width: 52, alignment: .leading)
                Picker("Model", selection: $choice) {
                    if role != nil { Text("The connection's model (\(list?.current ?? ""))").tag(Self.connection) }
                    ForEach(list?.options(for: role) ?? [], id: \.self) { Text($0).tag($0) }
                    Text("Other…").tag(Self.other)
                }.labelsHidden()
                if choice == Self.other {
                    TextField("Model id, e.g. claude-opus-5-5", text: $typed).onSubmit(save)
                }
            }
            HStack {
                Text("Effort").font(.system(size: 12)).frame(width: 52, alignment: .leading)
                Picker("Effort", selection: $effortChoice) {
                    Text(list?.defaultLabel(for: levelsModel) ?? "Default").tag(Self.byDefault)
                    ForEach(list?.levels(for: levelsModel, keeping: savedEffort) ?? [], id: \.self) { Text($0).tag($0) }
                    Text("Other…").tag(Self.other)
                }.labelsHidden()
                if effortChoice == Self.other {
                    TextField("Effort level, e.g. high", text: $typedEffort).onSubmit(save)
                }
                Button(busy ? "Checking…" : "Use") { save() }.disabled(busy || !changed || incomplete)
            }
            if role == nil, let list, !list.current.isEmpty {
                // APP-882: "In use" only where a role runs on this connection
                Text((model.ai?.inUse(runtime) == true ? "In use: " : "Model: ") + list.current
                     + (list.effort.isEmpty ? "" : " · effort \(list.effort)"))
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }
            if role == nil, let problem = list?.problem {
                Text("No model list from the provider (\(problem)). You can type a model id under Other….")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }
            if let line { Text(line).font(.system(size: 11)).textSelection(.enabled) }
        }
        .task { await load() }
        // a model or effort changed elsewhere: the pickers follow (a refusal or sign-in alone doesn't re-read the list)
        .onChange(of: savedKey) { old, new in if old != new { Task { await load() } } }
        .onChange(of: choice) { _, _ in
            if ![Self.byDefault, Self.other].contains(effortChoice),
               !(list?.levels(for: levelsModel, keeping: savedEffort).contains(effortChoice) ?? false) {
                effortChoice = Self.byDefault
            }
        }
    }

    /// What the daemon says this row runs now (its model and effort), so a change made elsewhere reloads it.
    private var savedKey: String {
        if let role { let r = model.ai?.roleDetails[role]; return "\(r?.runtime ?? "")|\(r?.model ?? "")|\(r?.effort ?? "")" }
        let row = model.ai?.row(runtime)
        return "\(row?.model ?? "")|\(row?.effort ?? "")"
    }

    private var chosen: String {
        switch choice {
        case Self.other: return typed.trimmingCharacters(in: .whitespacesAndNewlines)
        case Self.connection: return ""
        default: return choice
        }
    }

    /// The model whose levels the effort picker lists: the one picked ("" : the connection's).
    private var levelsModel: String { chosen }

    private var savedEffort: String { list?.effort(for: role) ?? "" }

    private var chosenEffort: String {
        switch effortChoice {
        case Self.byDefault: return ""
        case Self.other: return typedEffort.trimmingCharacters(in: .whitespacesAndNewlines)
        default: return effortChoice
        }
    }

    private var incomplete: Bool {
        (chosen.isEmpty && choice != Self.connection) || (effortChoice == Self.other && chosenEffort.isEmpty)
    }

    private var changed: Bool {
        guard let list else { return false }
        return chosen != list.current(for: role) || chosenEffort != list.effort(for: role)
    }

    private func load() async {
        list = await model.models(runtime)
        guard let list else { return }
        let mine = list.current(for: role)
        choice = mine.isEmpty ? (role == nil ? Self.other : Self.connection) : mine
        let effort = list.effort(for: role)
        effortChoice = effort.isEmpty ? Self.byDefault : effort
    }

    private func save() {
        let picked = chosen
        guard (!picked.isEmpty || role != nil), !incomplete else { return }
        busy = true
        Task {
            line = await model.setModel(runtime, picked, effort: chosenEffort, role: role,
                                        connectionModel: list?.current ?? "")
            await load()
            busy = false
        }
    }
}

/// Advanced (RUN-432, APP-793, APP-882): a model and effort for each role on the connection it runs on now, picked as
/// the connection's are; the connection's when none is chosen. Shown whenever a role runs anywhere.
struct RoleModelsView: View {
    @ObservedObject var model: SetupModel
    let ai: AISnapshot

    var body: some View {
        DisclosureGroup("Advanced: a model and effort for each role") {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(AISnapshot.roleOrder, id: \.self) { role in
                    if let runtime = ai.roles[role] {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("\(RoleCopy.label(role)) · on \(AISnapshot.label(runtime))").font(.system(size: 12, weight: .medium))
                            if role == "orchestrator" {
                                Text("Default: low effort, so replies come quickly.").font(.system(size: 11))
                                    .foregroundStyle(.secondary)
                            } else if role == "checker" {
                                Text("Default: high effort for a step that changed files, else the model's default.")
                                    .font(.system(size: 11)).foregroundStyle(.secondary)
                            }
                            ModelRow(model: model, runtime: runtime, role: role).id(role + "/" + runtime)
                        }
                    }
                }
            }.padding(.top, 6)
        }
        .font(.system(size: 12)).tint(.white.opacity(0.6))
    }
}

/// The roles FERB runs on a connection now (the daemon's roles), never merely one that is signed in (APP-655, APP-882):
/// a tick and "Planner · Worker · Checker".
private struct RolesBadge: View {
    let ai: AISnapshot?
    let runtime: String

    var body: some View {
        if let ai, ai.inUse(runtime) {
            HStack(spacing: 6) {
                Text(ai.rolesText(runtime)).font(.system(size: 11)).foregroundStyle(.secondary)
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
            }
            .help("FERB uses this now for: " + ai.rolesText(runtime)).accessibilityLabel("In use: " + ai.rolesText(runtime))
        }
    }
}

/// The last call a connection refused for its account and the owner's step (APP-882), until a later call it answers.
private struct RefusalLine: View {
    let ai: AISnapshot?
    let runtime: String

    var body: some View {
        if let refused = ai?.row(runtime)?.refused {
            Label(refused.text(), systemImage: "exclamationmark.triangle.fill")
                .font(.system(size: 11)).foregroundStyle(.orange).textSelection(.enabled)
        }
    }
}

/// FERB's background service didn't answer the page's last read (APP-884).
struct NotAnsweringLine: View {
    var body: some View {
        Label(SetupModel.notAnswering, systemImage: "bolt.horizontal.circle")
            .font(.callout).foregroundStyle(.orange)
    }
}

struct PermissionsStepView: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            StepHeader(title: "Let FERB work on your Mac", lines: ["Allow these once and FERB has everything it needs.",
                                                                   "Each row ticks itself when it's on."])
            if !SetupModel.quickPermissionOrder(model.setupRows).isEmpty {
                Button(model.allowingAll ? "Follow the permission prompts…" : "Set up quick permissions") {
                    Task { await model.allowQuickPermissions() }
                }
                .buttonStyle(SetupActionStyle(primary: true))
                .disabled(model.allowingAll)
            }
            if model.notificationHint {
                Text("Notifications: click Allow on FERB's banner at the top right of your screen (under Options if "
                     + "it shows no Allow).").font(.caption).foregroundStyle(.secondary)
            }
            if model.daemonSilent { NotAnsweringLine() }
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(model.setupRows) { row in PermissionRowView(row: row, model: model) }
                }
            }
            .opacity(model.daemonSilent ? 0.45 : 1)
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
            // APP-884: the status word, never a bare mark
            Text(PermissionCopy.status(row.status)).font(.caption.weight(.medium))
                .foregroundStyle(row.status == .granted ? Color.green : Color.secondary).frame(width: 84, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(PermissionCopy.label(row.id))
                Text(row.blocked.isEmpty ? PermissionCopy.reason(row.id) : row.blocked)
                    .font(.caption).foregroundStyle(.secondary)
                if row.restartNeeded && row.status != .granted {
                    Text("On in macOS; FERB needs a restart to use it.").font(.caption).foregroundStyle(.orange)
                }
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
                ForEach(snapshot.unsupported) { other in       // HAND-415: named, never passed over
                    Text(other.message + ".").font(.callout).foregroundStyle(.secondary)
                }
                if snapshot.browsers.contains(where: { $0.profiles.contains { $0.state == .on && !$0.ferbIdentity } }) {
                    HStack {
                        Button("Pair all") { Task { await model.pairAll() } }.disabled(model.pairing)
                        if model.pairing { ProgressView().controlSize(.small) }
                    }
                }
                if model.pairing, let progress = model.pairProgress, progress.tried > 0 {
                    // HAND-412: each profile's pairing as it lands
                    Text("Pairing: \(progress.landed) of \(progress.tried) done"
                         + (progress.pairing.isEmpty ? "" : " — " + progress.pairing.map(\.text).joined(separator: ", ")))
                        .font(.callout).foregroundStyle(.secondary)
                }
                if let waiting = model.addWait {
                    // HAND-413: after Add to, until FERB is on there
                    HStack {
                        ProgressView().controlSize(.small)
                        Text("Waiting for FERB to be added in \(ProfileName(name: waiting.name, email: waiting.email).quoted)…")
                            .font(.callout)
                        Button("Stop waiting") { model.stopWatchingForExtension() }
                    }
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(snapshot.browsers) { browser in
                            Text(browser.label).bold()
                            ForEach(browser.profiles) { profile in
                                HStack {
                                    ProfileNameText(name: profile.name + (profile.ferbIdentity ? " (FERB's own)" : ""),
                                                    email: profile.email)
                                    Spacer()
                                    // APP-883: the row's own state, version and problem, read every few seconds
                                    VStack(alignment: .trailing, spacing: 1) {
                                        Text(profile.stateText)
                                            .foregroundStyle(profile.needsAttention ? Color.orange : Color.secondary)
                                            .multilineTextAlignment(.trailing).textSelection(.enabled)
                                        if !profile.version.isEmpty {
                                            Text("Extension \(profile.version)").font(.caption).foregroundStyle(.secondary)
                                        }
                                    }
                                    if profile.state.canAdd {
                                        Button("Add to \(browser.label)") { Task { await model.addExtension(profile) } }
                                    }
                                    if profile.offersPair {
                                        Button("Pair") { Task { await model.pairProfile(profile) } }
                                            .disabled(model.pairing || profile.tooOld)
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
                LabeledContent("Its profile", value: "\(state.browser) / \(state.profileShown.quoted)"
                               + (state.paired ? " — paired" : " — not paired"))
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
                        ProfileNameText(name: "\(candidate.browserName): \(candidate.name)", email: candidate.email)
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

/// A browser profile's name and the account it is signed in to (HAND-424) as two texts: only the account shortens, in
/// the middle so its domain stays, and shows whole on hover. No account: the name alone.
struct ProfileNameText: View {
    let name: String
    let email: String

    var body: some View {
        HStack(spacing: 6) {
            Text(name).lineLimit(1).layoutPriority(1)
            if !email.isEmpty {
                Text(email).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle).help(email)
            }
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
                ForEach(paired, id: \.1.id) { label, profile in
                    Text(ProfileName.join("\(label): \(profile.name)", profile.email)).tag(profile.id)
                }
            }
            .labelsHidden()
        }
        .task { if model.browsers == nil { await model.refreshBrowsers() } }
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
