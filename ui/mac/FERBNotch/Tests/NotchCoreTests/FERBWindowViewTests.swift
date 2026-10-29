import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase E: every section and Settings tab builds and lays out with real rows, and ⌘, lands on Settings.
@MainActor
final class FERBWindowViewTests: XCTestCase {
    func filledStore() -> FERBWindowStore {
        let store = FERBWindowStore(rpc: ScriptedRPC())
        store.tasks = [TaskHistoryRow(["task_id": "t-1", "goal": "Rename the invoices", "status": "done",
                                       "answer": "Renamed 4 files."])!,
                       TaskHistoryRow(["task_id": "t-2", "goal": "Watch the build", "status": "running"])!]
        store.schedules = [ScheduleRow(["id": "sc-1", "request": "Post the update", "rule": "every day at 09:00",
                                        "state": "active", "last_result": ["status": "done", "answer": "Posted."]])!]
        store.reminders = [ReminderRow(["id": "tm-1", "kind": "reminder", "message": "Call the bank"])!]
        store.notices = [NoticeRow(["id": "n-1", "text": "Your build finished.",
                                    "quotes": [["text": "BUILD OK", "origin": "external:observation"]]])!]
        store.proposedMemory = [MemoryRow(["id": "m-1", "content": "Prefers mornings", "status": "proposed"])!]
        store.activeMemory = [MemoryRow(["id": "m-2", "content": "Uses VS Code", "status": "active"])!]
        store.settings = SettingsSnapshot(["values": ["sense.window_titles": true, "retention.trace_days": 30,
                                                      "privacy.exclude_apps": ["com.1password.1password"]]])
        store.banners = ["question": true, "reminder": false]
        return store
    }

    func actions() -> FERBAppActions {
        FERBAppActions(feedback: SoundFeedback(), cursor: CursorController(makePanels: false))
    }

    func lays(_ view: some View) {
        let hosting = NSHostingView(rootView: view.frame(width: 900, height: 600))
        hosting.frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
    }

    func testEverySectionLaysOut() {
        let store = filledStore()
        for section in FERBSection.allCases {
            let selection = FERBWindowSelection()
            selection.section = section
            lays(FERBWindowView(store: store, selection: selection, actions: actions()))
        }
    }

    func testSettingsLaysOutAtGeneral() {
        let selection = FERBWindowSelection()
        selection.section = .settings
        lays(SettingsView(store: filledStore(), selection: selection, actions: actions()))
        lays(GeneralSettingsPage(feedback: SoundFeedback(), cursor: CursorController(makePanels: false)))
    }

    func testSettingsOpensTheWindowAtTheTabAsked() {
        let controller = FERBWindowController(store: filledStore(), actions: actions())
        controller.select(.settings, tab: .privacy)
        XCTAssertEqual(controller.selection.section, .settings)
        XCTAssertEqual(controller.selection.settingsTab, .privacy)
        controller.select(.tasks)
        XCTAssertEqual(controller.selection.section, .tasks)
        XCTAssertEqual(controller.selection.settingsTab, .privacy)        // the tab is kept for next time
    }

    func testEverySettingsPageLaysOutInADevelopmentBuild() {
        let store = filledStore()
        lays(AISettingsPage(store: store, actions: actions()))
        lays(PermissionsSettingsPage(actions: actions()))
        lays(BrowsersSettingsPage(actions: actions()))
        lays(NotificationsSettingsPage(store: store))
        lays(PrivacySettingsPage(store: store))
        lays(AdvancedSettingsPage(store: store, actions: actions()))
        for tab in SettingsTab.allCases {
            let selection = FERBWindowSelection()
            selection.section = .settings
            selection.settingsTab = tab
            lays(SettingsView(store: store, selection: selection, actions: actions()))
        }
    }

    func testSidebarSelectionKeepsExistingDeepLinksAndRememberedSettings() {
        let controller = FERBWindowController(store: filledStore(), actions: actions())
        controller.select(.settings, tab: .privacy)
        XCTAssertEqual(controller.selection.destination, .settings(.privacy))
        XCTAssertEqual(controller.selection.title, "Privacy")
        controller.selection.destination = .section(.memory)
        XCTAssertEqual(controller.selection.section, .memory)
        XCTAssertEqual(controller.selection.settingsTab, .privacy)
        controller.select(.settings)
        XCTAssertEqual(controller.selection.destination, .settings(.privacy))
        controller.selection.destination = .settings(.notifications)
        XCTAssertEqual(controller.selection.section, .settings)
        XCTAssertEqual(controller.selection.settingsTab, .notifications)
        XCTAssertEqual(controller.selection.title, "Notifications")
    }

    func testEverySettingsDestinationRoutesWithoutASecondSelectionState() {
        let selection = FERBWindowSelection()
        for tab in SettingsTab.allCases {
            selection.destination = .settings(tab)
            XCTAssertEqual(selection.section, .settings)
            XCTAssertEqual(selection.settingsTab, tab)
            XCTAssertEqual(selection.destination, .settings(tab))
            // Existing app/menu routes remain the source of truth as well.
            selection.section = .tasks
            XCTAssertEqual(selection.destination, .section(.tasks))
        }
        selection.destination = nil // Clearing List selection must not blank the detail.
        XCTAssertEqual(selection.destination, .section(.tasks))
    }

    func testCategoryRecreationKeepsRunningCommandAndPrivacyDraft() {
        let store = filledStore()
        let output = FerbOutput()
        var emit: ((String) -> Void)?
        var finish: ((Int32) -> Void)?
        var commandActions = actions()
        commandActions.runFerb = { _, line, done in
            emit = line
            finish = done
            return true
        }
        output.run(["setup", "--cli"], with: commandActions)
        XCTAssertTrue(output.running)
        emit?("Still installing")
        // A category is conditionally recreated, but its parent retains this model.
        let returnedPage = AdvancedSettingsPage(store: store, actions: commandActions, output: output)
        XCTAssertTrue(returnedPage.output === output)
        XCTAssertTrue(returnedPage.output.running)
        XCTAssertEqual(returnedPage.output.lines, ["Still installing"])
        finish?(0)
        XCTAssertFalse(returnedPage.output.running)
        XCTAssertEqual(returnedPage.output.status, 0)
        let draft = PrivacySettingsDraft()
        draft.newSite = "example.com"
        draft.newApp = "com.example.app"
        let returnedPrivacy = PrivacySettingsPage(store: store, draft: draft)
        XCTAssertEqual(returnedPrivacy.draft.newSite, "example.com")
        XCTAssertEqual(returnedPrivacy.draft.newApp, "com.example.app")
    }

    func testFERBsOwnCommandsShowTheirOutputAndUninstallRunsItsHookFirst() {
        var order: [String] = []
        var packaged = actions()
        packaged.beforeUninstall = { order.append("login item") }
        packaged.runFerb = { arguments, line, done in
            order.append(arguments.joined(separator: " "))
            line("`ferb` command: /Users/o/.local/bin/ferb")
            done(0)
            return true
        }
        let output = FerbOutput()
        output.run(["setup", "--cli"], with: packaged)
        XCTAssertEqual(output.lines, ["`ferb` command: /Users/o/.local/bin/ferb"])
        XCTAssertEqual(output.status, 0)
        output.uninstall(with: packaged)
        XCTAssertEqual(order, ["setup --cli", "login item", "uninstall"])
        XCTAssertNil(actions().runFerb)                                    // a development build runs nothing
    }
}
