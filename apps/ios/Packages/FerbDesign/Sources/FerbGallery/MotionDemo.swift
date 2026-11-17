import FerbDesign
import SwiftUI

/// The app's motion (kb PHN-620..624) over the lab's pages, with the same hosts the app uses (Android's gallery
/// MotionDemo): tap a tab, a row of what happened while away (the question), the link picture (the relay sheet) or the
/// profile button; drag a sheet down. Invented data only.
struct MotionDemo: View {
    let moment: SkyMoment
    let insets: PhoneInsets
    @State private var tab: PhoneTab = .chat
    @State private var home = true
    @State private var sheet: String?

    private struct Key: Hashable { let tab: PhoneTab; let home: Bool }

    var body: some View {
        let close = { sheet = nil }
        let choose: (PhoneTab) -> Void = { t in if t == .chat && tab == .chat { home.toggle() }; tab = t }
        PhoneScreen(moment: moment, insets: insets, season: tab == .chat && home) {
            MotionSwitch(sheet, kind: .sheet) { s in
                switch s {
                case "question": QuestionSheet(insets: insets, onNo: close, onYes: close, onDismiss: close)
                case "relay": RelaySheet(insets: insets, onClose: close)
                default: ProfileSheet(insets: insets, onClose: close)
                }
            }
        } page: {
            TabBarHost(chosen: tab, waiting: tab == .tasks ? nil : .tasks, onTab: choose, onProfile: { sheet = "profile" }) {
                MotionSwitch(Key(tab: tab, home: home), kind: .tab,
                             direction: { a, b in (PhoneTab.allCases.firstIndex(of: b.tab) ?? 0) - (PhoneTab.allCases.firstIndex(of: a.tab) ?? 0) }) { k in
                    switch k.tab {
                    case .chat:
                        if k.home { HomePage(insets: insets, onRow: { _ in sheet = "question" }, onMore: { tab = .tasks }) }
                        else { ChatPage(insets: insets) }
                    case .tasks: TasksPage(onRow: { _, _ in sheet = "question" })
                    case .files: FilesPage(insets: insets)
                    case .mac: MacPage(link: .relay, onLink: { sheet = "relay" })
                    }
                }
            }
        }
        .task { if UserDefaults.standard.bool(forKey: "FERBMotionTour") { await tour() } }
    }

    /// Launched with `-FERBMotionTour YES` (for a screen recording, the simulator having no taps of its own): the tabs one
    /// by one and back, then the question, the profile and the relay sheets up and down. Changes as a tap makes them.
    private func tour() async {
        let steps: [() -> Void] = [
            { tab = .tasks }, { tab = .files }, { tab = .mac }, { tab = .chat },
            { sheet = "question" }, { sheet = nil }, { sheet = "profile" }, { sheet = nil },
            { tab = .mac }, { sheet = "relay" }, { sheet = nil }, { tab = .tasks }, { tab = .chat },
        ]
        try? await Task.sleep(nanoseconds: 1_500_000_000)
        for step in steps {
            step()
            try? await Task.sleep(nanoseconds: 1_300_000_000)
        }
    }
}
