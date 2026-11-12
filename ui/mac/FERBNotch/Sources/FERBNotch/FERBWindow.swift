import AppKit
import SwiftUI
import NotchCore

/// One sidebar route, derived from the existing menu/deep-link selection.
enum FERBWindowDestination: Hashable {
    case section(FERBSection)
    case settings(SettingsTab)
}

@MainActor
final class FERBWindowSelection: ObservableObject {
    @Published var section: FERBSection? = .tasks
    @Published var settingsTab: SettingsTab = .general

    var destination: FERBWindowDestination? {
        get { section == .settings ? .settings(settingsTab) : .section(section ?? .tasks) }
        set {
            guard let newValue else { return }
            switch newValue {
            case .section(let next): section = next
            case .settings(let tab):
                settingsTab = tab
                section = .settings
            }
        }
    }

    var title: String { section == .settings ? settingsTab.title : (section ?? .tasks).title }
}

/// What Settings needs from the rest of the app. Plan C's setup model and its `ferb` runner exist only in the installed
/// FERB.app: in a development build they are nil and their pages say so.
@MainActor
struct FERBAppActions {
    let feedback: SoundFeedback
    let cursor: CursorController
    var setHoverEnabled: ((Bool) -> Void)?
    /// Settings → Keyboard & voice records the owner's shortcut to open FERB here (D24).
    var hotKeys: HotKeyCenter?
    var correctMemory: ((String) -> Void)?
    var appVersion: String? = NotchModel.appVersion
    /// Plan C's: Settings → AI, Permissions and Browsers embed its step views on this model.
    var setup: SetupModel?
    /// Settings → General → Replay intro: the first-run intro's every beat again (kb area app).
    var replayIntro: (() -> Void)?
    /// Runs FERB.app's own `ferb` with these arguments (plan C's runner, one at a time): each output line, then the exit
    /// status. False when another run is under way.
    var runFerb: ((_ arguments: [String], _ line: @escaping (String) -> Void,
                   _ done: @escaping (Int32) -> Void) -> Bool)?
    /// Runs before `ferb uninstall` (plan F removes FERB.app's login item and unregisters the background service
    /// here): nil to go on, else why not, and the uninstall doesn't run.
    var beforeUninstall: (@MainActor () async -> String?)?
    /// Plan F's: Settings → General → Startup.
    var loginItems: LoginItemController?
    var updates: UpdateCoordinator?
    var logsDirectory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".ferb/logs", isDirectory: true)
    var openNotificationSettings: () -> Void = {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.notifications") { NSWorkspace.shared.open(url) }
    }
}

/// The regular FERB window (L34 Phase E). It opens only when the owner asks (menu bar, ⌘,): their own foreground
/// action (rule 17).
@MainActor
final class FERBWindowController: NSObject, NSWindowDelegate {
    let store: FERBWindowStore
    let selection = FERBWindowSelection()
    private let actions: FERBAppActions
    private(set) var window: NSWindow?

    init(store: FERBWindowStore, actions: FERBAppActions) {
        self.store = store
        self.actions = actions
    }

    func select(_ section: FERBSection, tab: SettingsTab? = nil) {
        selection.section = section
        if let tab { selection.settingsTab = tab }
    }

    /// The Memory page, filtered to one import's source when given (a memory import notice's page card).
    func showMemory(importedFrom source: String?) {
        store.memoryFilter = source
        show(.memory)
        Task { await store.loadMemory() }
    }

    func show(_ section: FERBSection, tab: SettingsTab? = nil) {
        select(section, tab: tab)
        if window == nil {
            let created = FERBAppWindow(contentRect: NSRect(x: 0, y: 0, width: 1020, height: 760),
                                        styleMask: [.titled, .closable, .miniaturizable, .resizable,
                                                    .fullSizeContentView],
                                        backing: .buffered, defer: false)
            created.title = "FERB"
            created.titleVisibility = .hidden
            created.titlebarAppearsTransparent = true
            created.titlebarSeparatorStyle = .none
            created.backgroundColor = .black
            created.isOpaque = false
            created.minSize = NSSize(width: 880, height: 620)
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.contentView = NSHostingView(rootView: FERBWindowView(store: store, selection: selection,
                                                                         actions: actions))
            created.center()
            created.setFrameAutosaveName("FERBWindow")
            window = created
        }
        if let window { AppWindowPresentation.show(window) }
    }

    /// Command-Q while this window is in front closes it and FERB keeps running (APP-654; owner, 2026-11-04: "cmd+q
    /// works but it quits it from the notch too"). True when it closed the window; otherwise the quit goes on.
    func closeOnQuitKey(keyWindow: NSWindow?) -> Bool {
        guard let window, keyWindow === window, window.isVisible else { return false }
        window.performClose(nil)
        return true
    }
}

struct FERBWindowView: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var selection: FERBWindowSelection
    let actions: FERBAppActions

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            VStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 7) {
                    Text(selection.title).font(.system(size: 27, weight: .semibold)).tracking(-0.7)
                    Text(subtitle).font(.system(size: 13)).foregroundStyle(.white.opacity(0.70))
                }
                .padding(.horizontal, 30).padding(.top, 55).padding(.bottom, 26)
                if store.problem != nil || store.note != nil {
                    VStack(alignment: .leading, spacing: 6) {
                        if let problem = store.problem {
                            Label(problem, systemImage: "exclamationmark.circle")
                                .foregroundStyle(.red).textSelection(.enabled)
                        }
                        if let note = store.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
                    }
                    .padding(.horizontal, 30).padding(.bottom, 16)
                }
                page
                    .scrollContentBackground(.hidden)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .task(id: selection.section) { await store.refresh(selection.section ?? .tasks) }
        }
        .frame(minWidth: 880, minHeight: 620)
        .font(.system(size: 13))
        .buttonStyle(SetupActionStyle())
        .foregroundStyle(.white.opacity(0.92))
        .background {
            SetupGlass()
                .overlay(LinearGradient(stops: [
                    .init(color: .black.opacity(0.94), location: 0),
                    .init(color: .black.opacity(0.66), location: 0.38),
                    .init(color: .black.opacity(0.30), location: 1)
                ], startPoint: .top, endPoint: .bottom))
                .overlay(alignment: .leading) {
                    HStack(spacing: 0) {
                        Color.black.opacity(0.60).frame(width: 174)
                        LinearGradient(colors: [.black.opacity(0.60), .clear],
                                       startPoint: .leading, endPoint: .trailing).frame(width: 120)
                    }.allowsHitTesting(false)
                }
        }
        .preferredColorScheme(.dark)
        .ignoresSafeArea(.container, edges: .top)
    }

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 1) {
                Text("ferb").font(.system(size: 28, weight: .semibold)).tracking(-1.2)
                Text(".").font(.system(size: 30, weight: .bold)).foregroundStyle(.orange)
            }
            .padding(.horizontal, 22).padding(.top, 53).padding(.bottom, 26)
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(FERBSection.allCases.filter { $0 != .settings }) { section in
                        sidebarButton(section.title, symbol: section.symbol, destination: .section(section))
                    }
                    settingsGroup("Experience", tabs: [.general, .notch, .notifications])
                    settingsGroup("Connections", tabs: [.ai, .browsers, .permissions])
                    settingsGroup("Manage FERB", tabs: [.privacy, .data, .advanced, .about])
                }.padding(.horizontal, 12).padding(.bottom, 24)
            }.scrollIndicators(.automatic)
        }
        .frame(width: 224)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private func settingsGroup(_ title: String, tabs: [SettingsTab]) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title.uppercased()).font(.system(size: 9, weight: .semibold)).tracking(1.5)
                .foregroundStyle(.white.opacity(0.55)).padding(.horizontal, 13)
                .padding(.top, 17).padding(.bottom, 6)
            ForEach(tabs) { tab in
                sidebarButton(tab.title, symbol: tab.symbol, destination: .settings(tab))
            }
        }
    }

    private func sidebarButton(_ title: String, symbol: String, destination: FERBWindowDestination) -> some View {
        let selected = selection.destination == destination
        return Button { selection.destination = destination } label: {
            HStack(spacing: 11) {
                Image(systemName: symbol).font(.system(size: 14, weight: .regular)).frame(width: 19)
                    .foregroundStyle(.white.opacity(selected ? 0.9 : 0.4))
                Text(title).font(.system(size: 12, weight: selected ? .semibold : .medium))
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 13).frame(minHeight: 37)
            .foregroundStyle(.white.opacity(selected ? 0.95 : 0.61))
            .background(.white.opacity(selected ? 0.09 : 0), in: RoundedRectangle(cornerRadius: 10))
            .contentShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? [.isSelected] : [])
    }

    private var subtitle: String {
        switch selection.section ?? .tasks {
        case .tasks: return "What FERB is working on, and what’s done."
        case .schedules: return "The things you want to happen, right on time."
        case .noticed: return "The little things FERB picked up along the way."
        case .memory: return "What FERB knows about you. Yours to change."
        case .settings: return selection.settingsTab.subtitle
        }
    }

    @ViewBuilder private var page: some View {
        switch selection.section ?? .tasks {
        case .tasks: TasksPage(store: store)
        case .schedules: SchedulesPage(store: store)
        case .noticed: NoticedPage(store: store)
        case .memory: MemoryPage(store: store, correctMemory: actions.correctMemory)
        case .settings: SettingsView(store: store, selection: selection, actions: actions)
        }
    }
}

/// The same quiet, rounded surfaces and controls as the setup and notch.
struct WindowCard<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        content.frame(maxWidth: .infinity, alignment: .leading).padding(20)
            .background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
    }
}

struct WindowPage<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) { content }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 30).padding(.bottom, 28)
        }
    }
}

struct WindowSectionLabel: View {
    let title: String
    var count: Int? = nil
    var body: some View {
        HStack(spacing: 8) {
            Text(title.uppercased()).tracking(1.5)
            if let count { Text(String(count)).monospacedDigit().foregroundStyle(.white.opacity(0.28)) }
        }
        .font(.system(size: 10, weight: .semibold)).foregroundStyle(.white.opacity(0.65))
        .padding(.top, 6).padding(.bottom, 3)
    }
}

struct WindowSettingsSection<Content: View>: View {
    var title: String? = nil
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let title { WindowSectionLabel(title: title) }
            WindowCard { VStack(alignment: .leading, spacing: 16) { content } }
                .toggleStyle(WindowToggleStyle())
                .labeledContentStyle(WindowSettingLabelStyle())
        }.padding(.bottom, 10)
    }
}

/// Keep the controls on one trailing axis while retaining the native switch interaction.
private struct WindowToggleStyle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 20) {
            configuration.label.accessibilityHidden(true)
            Spacer(minLength: 12)
            Toggle(isOn: configuration.$isOn) { configuration.label }
                .labelsHidden().toggleStyle(.switch)
        }
    }
}

private struct WindowSettingLabelStyle: LabeledContentStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 24) {
            configuration.label
            Spacer(minLength: 12)
            configuration.content.foregroundStyle(.secondary).multilineTextAlignment(.trailing)
        }
    }
}
