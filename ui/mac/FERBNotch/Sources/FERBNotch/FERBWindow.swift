import AppKit
import SwiftUI
import NotchCore

@MainActor
final class FERBWindowSelection: ObservableObject {
    @Published var section: FERBSection? = .tasks
    @Published var settingsTab: SettingsTab = .general
}

/// What Settings needs from the rest of the app. Plan C's setup model and its `ferb` runner exist only in the installed
/// FERB.app: in a development build they are nil and their pages say so.
@MainActor
struct FERBAppActions {
    let feedback: SoundFeedback
    let cursor: CursorController
    var appVersion: String? = NotchModel.appVersion
    /// Plan C's: Settings → AI, Permissions and Browsers embed its step views on this model.
    var setup: SetupModel?
    /// Set Up FERB… (plan C's setup window).
    var openSetup: (() -> Void)?
    /// Runs FERB.app's own `ferb` with these arguments (plan C's runner, one at a time): each output line, then the exit
    /// status. False when another run is under way.
    var runFerb: ((_ arguments: [String], _ line: @escaping (String) -> Void,
                   _ done: @escaping (Int32) -> Void) -> Bool)?
    /// Runs before `ferb uninstall` (plan F removes FERB.app's login item here).
    var beforeUninstall: (() -> Void)?
    /// Plan F's: Settings → General → Startup.
    var loginItems: LoginItemController?
}

/// The regular FERB window (L34 Phase E). It opens only when the owner asks (menu bar, ⌘,): their own foreground
/// action (rule 17).
@MainActor
final class FERBWindowController: NSObject, NSWindowDelegate {
    let store: FERBWindowStore
    let selection = FERBWindowSelection()
    private let actions: FERBAppActions
    private var window: NSWindow?

    init(store: FERBWindowStore, actions: FERBAppActions) {
        self.store = store
        self.actions = actions
    }

    func select(_ section: FERBSection, tab: SettingsTab? = nil) {
        selection.section = section
        if let tab { selection.settingsTab = tab }
    }

    func show(_ section: FERBSection, tab: SettingsTab? = nil) {
        select(section, tab: tab)
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 600),
                                   styleMask: [.titled, .closable, .miniaturizable, .resizable],
                                   backing: .buffered, defer: false)
            created.title = "FERB"
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.contentView = NSHostingView(rootView: FERBWindowView(store: store, selection: selection,
                                                                         actions: actions))
            created.center()
            created.setFrameAutosaveName("FERBWindow")
            window = created
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}

struct FERBWindowView: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var selection: FERBWindowSelection
    let actions: FERBAppActions

    var body: some View {
        NavigationSplitView {
            List(FERBSection.allCases, selection: $selection.section) { section in
                Label(section.title, systemImage: section.symbol)
            }
            .navigationSplitViewColumnWidth(min: 170, ideal: 190)
        } detail: {
            VStack(alignment: .leading, spacing: 8) {
                if let problem = store.problem { Text(problem).foregroundStyle(.red).textSelection(.enabled) }
                if let note = store.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
                page
            }
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .task(id: selection.section) { await store.refresh(selection.section ?? .tasks) }
        }
        .frame(minWidth: 760, minHeight: 480)
    }

    @ViewBuilder private var page: some View {
        switch selection.section ?? .tasks {
        case .tasks: TasksPage(store: store)
        case .schedules: SchedulesPage(store: store)
        case .noticed: NoticedPage(store: store)
        case .memory: MemoryPage(store: store)
        case .settings: SettingsView(store: store, selection: selection, actions: actions)
        }
    }
}
