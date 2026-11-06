#if FERB_PLAYGROUND
import AppKit
import SwiftUI
import NotchCore

/// What the control panel's buttons do (PlaygroundDelegate supplies them).
struct PlaygroundActions {
    var restartFresh: () -> Void
    var quitAndReopen: () -> Void
    var jump: (IntroBeat, Bool) -> Void
    var restartService: () -> Void
    var control: (IntroControl) -> Void
    var openNotch: () -> Void
    var closeNotch: () -> Void
    var place: (PlaygroundPlacement, CGDirectDisplayID?) -> Void
    var line: () -> String?
    var compareStartup: () -> Void = {}
    var exploreApp: () -> Void = {}
    var showSection: (FERBSection) -> Void = { _ in }
    var showScene: (PlaygroundChatScenario) -> Void = { _ in }
    var finishTask: () -> Void = {}
    var showTaskPreview: () -> Void = {}
    var previewSetup: () -> Void = {}
    /// Play 0.27.1: the player behind the controls (nil before the app playground opens).
    var player: () -> PlaygroundPlayer? = { nil }
    var startPlay: () -> Void = {}
}

/// Play 0.27.1's controls: Play, Pause, Restart, Notch / Window, and the beat now playing.
struct PlayControls: View {
    @ObservedObject var player: PlaygroundPlayer
    let start: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Play 0.27.1").font(.headline)
            Text("A scripted, invented conversation through the real notch and chat: every 0.27.1 change, beat by beat. "
                 + "Nothing real runs.").font(.caption).foregroundStyle(.secondary)
            Picker("Show in", selection: $player.mode) {
                Text("Notch").tag(ChatHost.notch)
                Text("Window").tag(ChatHost.window)
            }.pickerStyle(.segmented)
            HStack {
                Button("Play 0.27.1", action: start).keyboardShortcut("p", modifiers: [.command])
                Button(player.paused ? "Resume" : "Pause") { player.pause() }.disabled(!player.running)
                Button("Restart") { player.restart() }
            }
            if !player.caption.isEmpty { Text(player.caption).font(.callout).fixedSize(horizontal: false, vertical: true) }
            ProgressView(value: Double(player.index), total: Double(max(1, player.count)))
            ForEach(player.failures, id: \.self) { Text($0).font(.caption).foregroundStyle(.red) }
        }
    }
}

/// The playground's control panel: a plain window, deliberately not styled like FERB. Functional, not pretty.
struct PlaygroundPanel: View {
    @ObservedObject var state: PlaygroundState
    let actions: PlaygroundActions
    @AppStorage("concept.arrivalStyle") private var arrivalStyle = StartupStudy.gravityDense.rawValue
    @State private var completeEarlier = true
    @State private var placement: PlaygroundPlacement = .edge(.right)
    @State private var display: CGDirectDisplayID?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                appReview
                Divider()
                DisclosureGroup("Startup & setup") { intro }
                Divider()
                permissions
                Divider()
                setup
                Divider()
                tryBeat
                Divider()
                controls
                Divider()
                look
                Divider()
                log
            }
            .padding(14)
        }
        .frame(minWidth: 400, minHeight: 500)
        .onAppear {
            let defaults = UserDefaults.standard
            placement = NotchDocking.isDocked(in: defaults, hasNotch: true) ? .notch : .edge(NotchDocking.edge(in: defaults))
            display = (defaults.object(forKey: "notch.indicatorDisplayID") as? NSNumber)?.uint32Value
        }
    }

    private var appReview: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Explore FERB").font(.title2.bold())
            Text("Real app screens with sample data. Messages and actions are simulated.")
                .font(.caption).foregroundStyle(.secondary)
            if !state.exploringApp {
                Button("Open app playground") { actions.exploreApp() }
            } else {
                if let player = actions.player() { PlayControls(player: player, start: actions.startPlay) }
                HStack {
                    Button("Open notch / chat") { actions.openNotch() }
                    Button("Close notch") { actions.closeNotch() }
                }
                Text("App window").font(.headline)
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], alignment: .leading) {
                    ForEach(FERBSection.allCases) { section in
                        Button(section.title) { actions.showSection(section) }
                    }
                    Button("Task preview") { actions.showTaskPreview() }
                }
                Text("Notch & message states").font(.headline)
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], alignment: .leading) {
                    ForEach(PlaygroundChatScenario.allCases) { scene in
                        Button(scene.rawValue) { actions.showScene(scene) }
                    }
                }
                Button("Finish running sample tasks") { actions.finishTask() }
                Text("You can type in the chat too. Replies use the last selected sample state. Restart the app playground to reset its records.")
                    .font(.caption).foregroundStyle(.secondary)
                Button("Reset app playground") { actions.exploreApp() }
            }
        }
        .buttonStyle(.bordered)
    }

    // MARK: Intro

    private var intro: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(PlaygroundMode.usesArrival ? "Logo reveal entrance" : "Notch entrance").font(.headline)
            Button(PlaygroundMode.usesArrival ? "Try notch-only entrance" : "Try logo reveal entrance") {
                actions.compareStartup()
            }
            if PlaygroundMode.usesArrival {
                Picker("Startup study", selection: $arrivalStyle) {
                    ForEach(StartupStudy.allCases) { Text($0.title).tag($0.rawValue) }
                }
                Text((StartupStudy(rawValue: arrivalStyle) ?? .gravityDense).note)
                    .font(.caption).foregroundStyle(.secondary)
                Button("Play selected study from the top") { actions.restartFresh() }
            }
            Button("Review the service step") { actions.previewSetup() }
            TimelineView(.periodic(from: .now, by: 0.5)) { _ in
                VStack(alignment: .leading, spacing: 2) {
                    Text("Beat: " + beatName(IntroProgress(defaults: .standard).resumeBeat)).font(.callout.monospaced())
                    Text("Line: " + (actions.line() ?? "—")).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                }
            }
            HStack {
                Button("Restart from the top") { actions.restartFresh() }
                Button("Quit & reopen FERB") { actions.quitAndReopen() }
            }
            Text("Jump to (quits and reopens at that beat):").font(.caption)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 88))], alignment: .leading, spacing: 6) {
                ForEach(IntroBeat.allCases, id: \.rawValue) { beat in
                    Button(beatName(beat)) { actions.jump(beat, completeEarlier) }
                }
            }
            Toggle("Mark the steps before it done when jumping", isOn: $completeEarlier)
        }
    }

    // MARK: Permissions

    private var permissions: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Permissions (faked; macOS is never asked)").font(.headline)
            ForEach(PlaygroundState.permissions, id: \.self) { id in
                HStack {
                    Text(PermissionCopy.label(id)).frame(width: 150, alignment: .leading)
                    Picker("", selection: Binding(get: { state.grant(id) }, set: { state.set(id, $0) })) {
                        Text("Not asked").tag(GrantStatus.notDetermined)
                        Text("On").tag(GrantStatus.granted)
                        Text("Off").tag(GrantStatus.denied)
                    }
                    .pickerStyle(.segmented).labelsHidden()
                    if state.saved.afterRestart.contains(id) { Text("after restart").font(.caption2).foregroundStyle(.orange) }
                }
            }
            Picker("When FERB asks, the owner", selection: $state.saved.answer) {
                ForEach(PlaygroundState.Answer.allCases) { Text($0.rawValue).tag($0) }
            }
            ForEach(state.asking, id: \.self) { id in
                HStack {
                    Text("macOS asks: \(PermissionCopy.label(id))").bold()
                    Button("Allow") { state.answer(id, .granted) }
                    Button("Don't Allow") { state.answer(id, .denied) }
                }
            }
            Toggle("Screen Recording, Input Monitoring and Full Disk Access apply only after a restart",
                   isOn: $state.saved.grantsNeedRestart)
            HStack {
                Button("Restart background service") { actions.restartService() }
                Button("All on") { for id in PlaygroundState.permissions { state.set(id, .granted) } }
                Button("All not asked") { for id in PlaygroundState.permissions { state.set(id, .notDetermined) } }
            }
        }
    }

    // MARK: Meet, sign-in, browsers

    private var setup: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Background service, sign-in, browsers").font(.headline)
            Toggle("Installing the background service fails", isOn: $state.saved.serviceInstallFails)
            Toggle("macOS wants an OK in Login Items first", isOn: $state.saved.loginItemsNeedOK)
            Toggle("Allowed in Login Items", isOn: $state.saved.loginItemsAllowed)
            Toggle("Sign-in fails", isOn: $state.saved.signInFails)
            HStack {
                Text("Signed in: \(state.saved.runtime.map(AISnapshot.label) ?? "no")")
                Button("Sign out") { state.saved.runtime = nil; state.saved.claudeSignIn = nil }
            }
            Toggle("Pairing fails", isOn: $state.saved.pairingFails)
            Text("Browsers paired: \(state.saved.browsersPaired ? "yes" : "no") · FERB's email: " +
                 (state.saved.identityAddress.isEmpty ? "none" : state.saved.identityAddress)).font(.caption)
        }
    }

    // MARK: Try

    private var tryBeat: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Try (suggestions and the simulated task)").font(.headline)
            Picker("Suggestions (intro.suggest)", selection: $state.saved.suggest) {
                ForEach(PlaygroundState.Outcome.allCases) { Text($0.rawValue).tag($0) }
            }.pickerStyle(.segmented)
            Picker("Plan preview (intro.preview)", selection: $state.saved.preview) {
                ForEach(PlaygroundState.Outcome.allCases) { Text($0.rawValue).tag($0) }
            }.pickerStyle(.segmented)
            Toggle("The Mac scan saw something (\"Here's what I saw\")", isOn: $state.saved.scanSeesSomething)
        }
    }

    // MARK: Control beat

    private var controls: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Controls (the real keys work too)").font(.headline)
            HStack {
                Button("Double-tap ⌥") { actions.control(.toggle) }
                Button("Hold ⌘⌥") { actions.control(.talk) }
            }
            HStack {
                Button("Pause FERB") { actions.control(.pause) }
                Button("Open notch") { actions.openNotch() }
                Button("Close notch") { actions.closeNotch() }
            }
        }
    }

    // MARK: Look

    private var look: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Look").font(.headline)
            Toggle("Sound", isOn: $state.saved.soundOn)
            Toggle("Simulate a call (quiet sounds)", isOn: $state.simulatesCall)
            Text("Microphone activity is simulated. Mac sound settings still apply.")
                .font(.caption).foregroundStyle(.secondary)
            Picker("Speed", selection: Binding(get: { state.saved.speed }, set: { state.setSpeed($0) })) {
                ForEach([0.25, 0.5, 1.0, 2.0, 4.0], id: \.self) { Text("\($0, specifier: "%g")×").tag($0) }
            }.pickerStyle(.segmented)
            Text("Speed scales the tour's waits, the orb's motion and the fake daemon; the notch's own springs run at "
                 + "normal speed.").font(.caption).foregroundStyle(.secondary)
            Picker("Notch", selection: $placement) {
                ForEach(PlaygroundPlacement.all, id: \.self) { Text($0.label).tag($0) }
            }
            Picker("Display", selection: $display) {
                Text("FERB's choice").tag(CGDirectDisplayID?.none)
                ForEach(NSScreen.screens, id: \.self) { screen in
                    Text(screen.localizedName + (screen.safeAreaInsets.top > 0 ? " (has a notch)" : " (no notch)"))
                        .tag(Self.displayID(screen))
                }
            }
            Button("Place the notch") { actions.place(placement, display) }
            Text("“In the notch” needs a display with a camera housing; elsewhere FERB stays on an edge.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    // MARK: Log

    private var log: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Log").font(.headline)
            ForEach(Array(state.log.suffix(60).reversed().enumerated()), id: \.offset) { _, line in
                Text(line).font(.caption.monospaced()).textSelection(.enabled)
            }
        }
    }

    private func beatName(_ beat: IntroBeat?) -> String {
        guard let beat else { return "—" }
        switch beat {
        case .meet: return "Meet"
        case .signIn: return "Sign in"
        case .permissions: return "Permissions"
        case .browsers: return "Browsers"
        case .identity: return "Email"
        case .tryIt: return "Try"
        case .control: return "Control"
        case .home: return "Home"
        }
    }

    private static func displayID(_ screen: NSScreen) -> CGDirectDisplayID? {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value
    }
}
#endif
