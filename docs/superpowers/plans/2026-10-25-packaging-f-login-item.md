# Packaging Phase F: Launch at Login Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FERB.app opens at login (on by default once setup is complete, switchable in Settings → General) through
macOS's own login item, `SMAppService.mainApp.register()`, falling back to a user LaunchAgent for the app when macOS
refuses that item without a Developer ID.

**Architecture:** A pure plan in `NotchCore` (`LoginItemPlan`: the fallback agent's plist, the default-once rule, the
notes) and one controller in the app (`LoginItemController`) over two interchangeable services: `AppServiceLoginItem`
(`SMAppService.mainApp`) and `LaunchAgentLoginItem` (`~/Library/LaunchAgents/com.ferb.FERB.login.plist`,
`open -g -a <FERB.app>`). The controller tries macOS's item first and uses the LaunchAgent only when macOS refuses it
(an error, or the item not turning on), and says so. `AppSetupController` (as plan C rewrote it) reports when setup has
settled, which applies the default once. `ferb uninstall` removes the fallback file; the app's Uninstall also
unregisters macOS's item.

**Tech Stack:** Swift 6 tools in Swift 5 mode, AppKit/SwiftUI, ServiceManagement (`SMAppService`, macOS 13+; FERB
needs 14.2), `swift test`; Python 3.12, pytest.

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md` (decision L34, approved by the owner on
2026-10-25: "all 7 parts in full, juat design them bare min. focus on correct working"), "Phase F — Launch at login"
and "What a new user experiences" §5. **The fallback the spec asks this plan to state:** if macOS refuses
`SMAppService.mainApp.register()` for FERB's self-made signature (no Developer ID), FERB writes a user LaunchAgent,
`~/Library/LaunchAgents/com.ferb.FERB.login.plist`, that runs `/usr/bin/open -g -a /Applications/FERB.app` at login,
and Settings → General says "macOS refused FERB's login item (…), so a login LaunchAgent opens FERB instead." Which
path a real Mac takes is decided by the owner's live check on a second macOS user (Task 3), not assumed.

## Global Constraints

- Read `CLAUDE.md` first. Never read `vibed/`. Work on `main`; commit messages cite `L34`.
- Bare-minimum UI: one `Toggle` ("Open FERB at login"), one caption line and, only when macOS needs the owner's OK, one
  button that opens System Settings → General → Login Items. No polish.
- On by default after setup, exactly once ("on by default after setup"): a later "off", in Settings or in System
  Settings, is never undone by FERB.
- Rule 17: at login FERB.app opens in the background (`open -g` for the fallback; macOS's login item opens FERB.app,
  an `LSUIElement` app that takes no focus). Nothing here moves the owner's cursor, focus or Space.
- Rule 10: FERB's shell can't write `~/Library/LaunchAgents` (it is in `DEFAULT_NEVER_WRITE`,
  `ferb/policy/protection.py:60-64`) and can't start launchd jobs (`(deny job-creation)`, `sandbox.py`); only FERB.app,
  the owner's own process, writes the fallback file.
- The background service already starts at login (`com.ferb.daemon`, `RunAtLoad`); this plan adds the app, not the
  daemon. The fallback's label `com.ferb.FERB.login` never collides with `com.ferb.daemon`.
- A development build (`FERB Notch.app`, `dev.ferb.notch`, not packaged) never registers anything.
- Each task: failing test first; Swift `swift test --package-path ui/mac/FERBNotch --filter <Class>`; Python
  `nice -n 10 uv run pytest <files> -q`. Before the last commit: `nice -n 10 uv run pytest -q`,
  `uv run mypy ferb/core`, `git diff --check`, `swift test --package-path ui/mac/FERBNotch`.
- Commit only the files the task names (other agents have uncommitted work here); check `git diff --cached --stat`;
  end each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Interfaces with other phases (assumptions; confirm against plans B/C/D)

- **Plan B (stable signing)** — confirm against plan B: release builds are signed with "FERB Release Signing"
  (`SIGN_IDENTITY=release`), whose designated requirement for `com.ferb.FERB` pins its leaf, so macOS sees the same
  FERB.app across updates. Task 3's live check runs on a B-signed build from the DMG, twice (build N, then N+1), and
  records whether the login item survives the update.
- **Plan C (native setup)** — confirm against plan C (Task 8's `AppSetupController.swift`): `startAtLaunch()` switches
  on `AppSetupPlan.atLaunch(action, setupDone:)` with `case .nothing: return`; `quietUpdate()` runs the service update
  and shows the Permissions step when `model.needsPermissions()`; `show(_:action:)` sets `model.onFinish = { …
  self?.onFinished?() }` (the setup window's Done). Task 2 adds `onSettled` calls at exactly these three places.
- **Plan D (menu bar):** nothing consumed.
- **Plan E (consumed, built before this plan):** `GeneralSettingsPage(feedback:cursor:)` and `SettingsView` in
  `Sources/FERBNotch/SettingsPages.swift`; `FERBAppActions` (`beforeUninstall`) in `Sources/FERBNotch/FERBWindow.swift`;
  `Application.swift`'s actions block.
- **Produced for plan G:** `AppSetupController.onSettled: (() -> Void)?`: called once setup has nothing left to do —
  at launch when this build is set up, after a quiet update that needs no permission, or at the setup window's Done.
  G posts "Updated to …" from the same callback.

## Review Focus

- The owner turns "Open FERB at login" off, then setup settles again (Set Up FERB…, or an update's quiet service
  update): FERB must not switch it back on. Pinned in Task 2 (`testDefaultAfterSetupTurnsTheLoginItemOnOnce`).
- macOS accepts `register()` but leaves the item needing the owner's OK (`.requiresApproval`): that is not a refusal;
  FERB must not add the LaunchAgent as well (two launches at login) and must show the Login Items button. Pinned in
  Task 2 (`testApprovalNeededIsNotARefusal`).
- `register()` returns without an error but the item isn't on (status `.notRegistered`/`.notFound`): treat it as a
  refusal and fall back. Pinned in Task 2 (`testARegisterThatLeavesItOffFallsBack`).
- A file the owner made at `~/Library/LaunchAgents/com.ferb.FERB.login.plist` with another label: neither the app nor
  `ferb uninstall` may delete it. Pinned in Task 2 (`testTheLaunchAgentRemovesOnlyItsOwnFile`) and Task 3
  (`test_uninstall_leaves_a_file_with_another_label`).
- macOS accepts the item after FERB had used the fallback (e.g. a later build): the fallback file must go, or FERB opens
  twice. Pinned in Task 2 (`testAnAcceptedLoginItemRemovesTheFallback`).

---

### Task 1: `LoginItemPlan` in `NotchCore`

**Files:**
- Create: `ui/mac/FERBNotch/Sources/NotchCore/LoginItemPlan.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/LoginItemPlanTests.swift`

**Interfaces:**
- Produces (public): `LoginItemPlan.label` (`"com.ferb.FERB.login"`), `agent(appPath:) -> [String: Any]`,
  `isOurs(_ plist: [String: Any]?) -> Bool`, `shouldApplyDefault(applied:) -> Bool`, `approvalNote`,
  `fallbackNote(_ refusal: String?) -> String`, `LoginItemPlan.Refused` (an `Error`: macOS returned without turning
  the item on).

- [ ] **Step 1: Write the failing tests** (create `Tests/NotchCoreTests/LoginItemPlanTests.swift`)

```swift
import XCTest
@testable import NotchCore

/// L34 Phase F: FERB.app opens at login. The fallback LaunchAgent opens the app in the background, and the default
/// applies once.
final class LoginItemPlanTests: XCTestCase {
    func testTheFallbackAgentOpensFERBInTheBackgroundAtLoginOnly() {
        let agent = LoginItemPlan.agent(appPath: "/Applications/FERB.app")
        XCTAssertEqual(agent["Label"] as? String, "com.ferb.FERB.login")
        XCTAssertEqual(agent["ProgramArguments"] as? [String], ["/usr/bin/open", "-g", "-a", "/Applications/FERB.app"])
        XCTAssertEqual(agent["RunAtLoad"] as? Bool, true)
        XCTAssertEqual(agent["LimitLoadToSessionType"] as? String, "Aqua")
        XCTAssertEqual(agent["AssociatedBundleIdentifiers"] as? [String], ["com.ferb.FERB"])
        XCTAssertNil(agent["KeepAlive"])                                   // opened once per login, never relaunched
    }

    func testOnlyFERBsOwnAgentCountsAsOurs() {
        XCTAssertTrue(LoginItemPlan.isOurs(LoginItemPlan.agent(appPath: "/Applications/FERB.app")))
        XCTAssertFalse(LoginItemPlan.isOurs(["Label": "com.example.other"]))
        XCTAssertFalse(LoginItemPlan.isOurs(nil))
    }

    func testTheDefaultAppliesOnce() {
        XCTAssertTrue(LoginItemPlan.shouldApplyDefault(applied: false))
        XCTAssertFalse(LoginItemPlan.shouldApplyDefault(applied: true))
    }

    func testTheNotesSayWhatHappened() {
        XCTAssertEqual(LoginItemPlan.approvalNote,
                       "macOS needs your OK: System Settings → General → Login Items → allow FERB.")
        XCTAssertEqual(LoginItemPlan.fallbackNote("Operation not permitted"),
                       "macOS refused FERB's login item (Operation not permitted), so a login LaunchAgent opens FERB instead.")
        XCTAssertEqual(LoginItemPlan.fallbackNote(nil),
                       "macOS refused FERB's login item, so a login LaunchAgent opens FERB instead.")
        XCTAssertEqual(LoginItemPlan.Refused().localizedDescription, "the login item didn't turn on")
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter LoginItemPlanTests`
Expected: FAIL to build: `cannot find 'LoginItemPlan' in scope`.

- [ ] **Step 3: Create `Sources/NotchCore/LoginItemPlan.swift`**

```swift
import Foundation

/// FERB.app opening at login (L34 Phase F). macOS's own login item (`SMAppService.mainApp`) is tried first; when
/// macOS refuses it for FERB's self-made signature, a user LaunchAgent opens the app instead. The background service
/// already starts at login on its own (`com.ferb.daemon`).
public enum LoginItemPlan {
    /// The fallback LaunchAgent's label and file name (`~/Library/LaunchAgents/<label>.plist`).
    public static let label = "com.ferb.FERB.login"

    public static let approvalNote = "macOS needs your OK: System Settings → General → Login Items → allow FERB."

    public static func fallbackNote(_ refusal: String?) -> String {
        "macOS refused FERB's login item" + (refusal.map { " (\($0))" } ?? "") +
            ", so a login LaunchAgent opens FERB instead."
    }

    /// macOS returned from `register()` without turning the item on (and without asking for the owner's OK).
    public struct Refused: LocalizedError {
        public init() {}
        public var errorDescription: String? { "the login item didn't turn on" }
    }

    /// Opens FERB.app once per login, in the background (rule 17), in the owner's GUI session only; System Settings
    /// lists it under FERB.
    public static func agent(appPath: String) -> [String: Any] {
        ["Label": label, "ProgramArguments": ["/usr/bin/open", "-g", "-a", appPath], "RunAtLoad": true,
         "LimitLoadToSessionType": "Aqua", "AssociatedBundleIdentifiers": ["com.ferb.FERB"]]
    }

    /// A plist FERB wrote (anything else at that path is the owner's and is left alone).
    public static func isOurs(_ plist: [String: Any]?) -> Bool { plist?["Label"] as? String == label }

    /// On by default once: after that, only the owner's switch decides.
    public static func shouldApplyDefault(applied: Bool) -> Bool { !applied }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter LoginItemPlanTests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ui/mac/FERBNotch/Sources/NotchCore/LoginItemPlan.swift ui/mac/FERBNotch/Tests/NotchCoreTests/LoginItemPlanTests.swift
git diff --cached --stat
git commit -m "feat(L34): LoginItemPlan: the fallback login agent and the default-once rule

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `LoginItemController`, its two services, the default after setup, Settings → General, Uninstall

**Files:**
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/LoginItemController.swift`
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/AppSetupController.swift` (plan C's: `onFinished` property,
  `startAtLaunch()`, `quietUpdate()`, `show(_:action:)`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/FERBWindow.swift` (`FERBAppActions`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift` (`SettingsView`, `GeneralSettingsPage`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/Application.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/LoginItemControllerTests.swift`

**Interfaces:**
- Consumes: Task 1's `LoginItemPlan`; plan E's `FERBAppActions.beforeUninstall`, `GeneralSettingsPage`; plan C's
  `AppSetupController`.
- Produces: `@MainActor protocol LoginItemService` (`enabled`, `needsApproval`, `enable() throws`, `disable() throws`);
  `AppServiceLoginItem`; `LaunchAgentLoginItem(plist:appPath:)`; `@MainActor final class LoginItemController:
  ObservableObject` (`enabled`, `needsApproval`, `note`, `available`, `setEnabled(_:)`, `applyDefaultAfterSetup()`,
  `removeForUninstall()`, `openSystemSettings()`, `static installed(available:)`); `LoginItemRow`;
  `AppSetupController.onSettled: (() -> Void)?`; `FERBAppActions.loginItems`;
  `GeneralSettingsPage(feedback:cursor:loginItems:)` with a "Startup" section.

- [ ] **Step 1: Write the failing tests** (create `Tests/NotchCoreTests/LoginItemControllerTests.swift`)

```swift
import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
private final class FakeLoginItem: LoginItemService {
    var enabled = false
    var needsApproval = false
    var refuse: Error?
    var staysOff = false
    var asksApproval = false
    var enables = 0
    var disables = 0

    func enable() throws {
        enables += 1
        if let refuse { throw refuse }
        if asksApproval { needsApproval = true; return }
        if !staysOff { enabled = true }
    }

    func disable() throws {
        disables += 1
        enabled = false
        needsApproval = false
    }
}

/// L34 Phase F: on by default once after setup; macOS's login item first; the LaunchAgent only when macOS refuses it.
@MainActor
final class LoginItemControllerTests: XCTestCase {
    private var defaults: UserDefaults!

    override func setUp() {
        super.setUp()
        defaults = UserDefaults(suiteName: "ferb-login-\(UUID().uuidString)")
    }

    private func make(_ app: FakeLoginItem, _ agent: FakeLoginItem, available: Bool = true) -> LoginItemController {
        LoginItemController(appService: app, launchAgent: agent, defaults: defaults, available: available)
    }

    func testDefaultAfterSetupTurnsTheLoginItemOnOnce() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        let items = make(app, agent)
        items.applyDefaultAfterSetup()
        XCTAssertTrue(items.enabled)
        XCTAssertEqual(app.enables, 1)
        items.setEnabled(false)
        items.applyDefaultAfterSetup()                 // a later setup or update settles again
        XCTAssertFalse(items.enabled)
        XCTAssertEqual(app.enables, 1)
        XCTAssertEqual(agent.enables, 0)
    }

    func testARefusedLoginItemFallsBackToTheLaunchAgentAndSaysSo() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.refuse = NSError(domain: "SMAppServiceErrorDomain", code: 1,
                             userInfo: [NSLocalizedDescriptionKey: "Operation not permitted"])
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertTrue(items.enabled)
        XCTAssertEqual(agent.enables, 1)
        XCTAssertEqual(items.note, LoginItemPlan.fallbackNote("Operation not permitted"))
        XCTAssertFalse(items.needsApproval)
    }

    func testARegisterThatLeavesItOffFallsBack() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.staysOff = true
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertTrue(agent.enabled)
        XCTAssertEqual(items.note, LoginItemPlan.fallbackNote("the login item didn't turn on"))
    }

    func testApprovalNeededIsNotARefusal() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.asksApproval = true
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertEqual(agent.enables, 0)               // never two launches at login
        XCTAssertTrue(items.needsApproval)
        XCTAssertEqual(items.note, LoginItemPlan.approvalNote)
    }

    func testAnAcceptedLoginItemRemovesTheFallback() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        agent.enabled = true                            // an earlier build used the fallback
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertTrue(app.enabled)
        XCTAssertFalse(agent.enabled)
        XCTAssertNil(items.note)
    }

    func testTurningOffRemovesBothAndUninstallDoesToo() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.enabled = true
        agent.enabled = true
        let items = make(app, agent)
        items.setEnabled(false)
        XCTAssertFalse(app.enabled || agent.enabled)
        app.enabled = true
        items.removeForUninstall()
        XCTAssertFalse(app.enabled)
        XCTAssertFalse(items.enabled)
    }

    func testADevelopmentBuildRegistersNothing() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        let items = make(app, agent, available: false)
        items.applyDefaultAfterSetup()
        items.setEnabled(true)
        XCTAssertEqual(app.enables + agent.enables, 0)
        XCTAssertFalse(defaults.bool(forKey: LoginItemController.defaultAppliedKey))
    }

    func testTheLaunchAgentRemovesOnlyItsOwnFile() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-agents-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: folder) }
        let plist = folder.appendingPathComponent(LoginItemPlan.label + ".plist")
        let agent = LaunchAgentLoginItem(plist: plist, appPath: "/Applications/FERB.app")
        XCTAssertFalse(agent.enabled)
        try agent.enable()
        XCTAssertTrue(agent.enabled)
        let written = try XCTUnwrap(NSDictionary(contentsOf: plist) as? [String: Any])
        XCTAssertEqual(written["ProgramArguments"] as? [String], ["/usr/bin/open", "-g", "-a", "/Applications/FERB.app"])
        try agent.disable()
        XCTAssertFalse(FileManager.default.fileExists(atPath: plist.path))
        try (["Label": "com.example.mine"] as NSDictionary).write(to: plist)
        try agent.disable()
        XCTAssertTrue(FileManager.default.fileExists(atPath: plist.path))     // the owner's own file stays
    }

    func testTheStartupSectionLaysOut() {
        let items = make(FakeLoginItem(), FakeLoginItem())
        let page = GeneralSettingsPage(feedback: SoundFeedback(), cursor: CursorController(makePanels: false),
                                       loginItems: items)
        let hosting = NSHostingView(rootView: page.frame(width: 700, height: 500))
        hosting.frame = NSRect(x: 0, y: 0, width: 700, height: 500)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter LoginItemControllerTests`
Expected: FAIL to build: `cannot find type 'LoginItemService' in scope`.

- [ ] **Step 3: Create `Sources/FERBNotch/LoginItemController.swift`**

```swift
import AppKit
import ServiceManagement
import SwiftUI
import NotchCore

/// Something that can make FERB.app open at login (L34 Phase F).
@MainActor
protocol LoginItemService {
    var enabled: Bool { get }
    /// macOS registered the item but waits for the owner's OK in System Settings → General → Login Items.
    var needsApproval: Bool { get }
    func enable() throws
    func disable() throws
}

/// macOS's own login item for this app.
@MainActor
struct AppServiceLoginItem: LoginItemService {
    var enabled: Bool { SMAppService.mainApp.status == .enabled }
    var needsApproval: Bool { SMAppService.mainApp.status == .requiresApproval }
    func enable() throws { try SMAppService.mainApp.register() }
    func disable() throws { try SMAppService.mainApp.unregister() }
}

/// The fallback when macOS refuses the login item without a Developer ID: a user LaunchAgent that opens FERB.app in
/// the background at login. launchd loads it at the next login; nothing starts now. Only FERB's own file is removed.
@MainActor
struct LaunchAgentLoginItem: LoginItemService {
    let plist: URL
    let appPath: String

    var enabled: Bool { LoginItemPlan.isOurs(NSDictionary(contentsOf: plist) as? [String: Any]) }
    var needsApproval: Bool { false }

    func enable() throws {
        try FileManager.default.createDirectory(at: plist.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try PropertyListSerialization.data(fromPropertyList: LoginItemPlan.agent(appPath: appPath),
                                                      format: .xml, options: 0)
        try data.write(to: plist, options: .atomic)
    }

    func disable() throws {
        guard enabled else { return }
        try FileManager.default.removeItem(at: plist)
    }
}

/// Open FERB at login (L34 Phase F): macOS's login item first; the LaunchAgent only when macOS refuses it, and the
/// note says so. On by default once, after setup; then only the owner's switch decides.
@MainActor
final class LoginItemController: ObservableObject {
    static let defaultAppliedKey = "loginItem.defaultApplied"
    static let refusalKey = "loginItem.refusal"

    @Published private(set) var enabled = false
    @Published private(set) var needsApproval = false
    @Published private(set) var note: String?
    /// The installed FERB.app only; a development build registers nothing.
    let available: Bool

    private let appService: LoginItemService
    private let launchAgent: LoginItemService
    private let defaults: UserDefaults

    init(appService: LoginItemService, launchAgent: LoginItemService, defaults: UserDefaults = .standard,
         available: Bool) {
        self.appService = appService
        self.launchAgent = launchAgent
        self.defaults = defaults
        self.available = available
        refresh()
    }

    static func installed(available: Bool) -> LoginItemController {
        let agents = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents")
        let agent = LaunchAgentLoginItem(plist: agents.appendingPathComponent(LoginItemPlan.label + ".plist"),
                                         appPath: Bundle.main.bundleURL.resolvingSymlinksInPath().path)
        return LoginItemController(appService: AppServiceLoginItem(), launchAgent: agent, available: available)
    }

    func refresh() {
        enabled = appService.enabled || launchAgent.enabled
        needsApproval = appService.needsApproval
        if needsApproval { note = LoginItemPlan.approvalNote }
        else if launchAgent.enabled { note = LoginItemPlan.fallbackNote(defaults.string(forKey: Self.refusalKey)) }
        else { note = nil }
    }

    func setEnabled(_ on: Bool) {
        guard available else { return }
        note = nil
        if on { turnOn() } else { turnOff() }
        let failure = note                             // turnOn's "couldn't" outlasts refresh()
        refresh()
        if let failure { note = failure }
    }

    private func turnOn() {
        do {
            try appService.enable()
            guard appService.enabled || appService.needsApproval else { throw LoginItemPlan.Refused() }
            try? launchAgent.disable()                 // macOS accepted it: the fallback would open FERB twice
            defaults.removeObject(forKey: Self.refusalKey)
        } catch {
            defaults.set(error.localizedDescription, forKey: Self.refusalKey)
            do {
                try launchAgent.enable()
            } catch let failure {
                note = "FERB couldn't set itself to open at login: \(failure.localizedDescription)"
            }
        }
    }

    private func turnOff() {
        try? appService.disable()
        try? launchAgent.disable()
    }

    /// On by default once, after setup: a later "off" (here or in System Settings) is never undone.
    func applyDefaultAfterSetup() {
        guard available, LoginItemPlan.shouldApplyDefault(applied: defaults.bool(forKey: Self.defaultAppliedKey)) else {
            return
        }
        defaults.set(true, forKey: Self.defaultAppliedKey)
        setEnabled(true)
    }

    /// Settings → Advanced → Uninstall: the login item goes with the background service.
    func removeForUninstall() {
        turnOff()
        refresh()
    }

    func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }
}

/// Settings → General → Startup.
struct LoginItemRow: View {
    @ObservedObject var loginItems: LoginItemController

    var body: some View {
        Toggle("Open FERB at login", isOn: Binding(get: { loginItems.enabled }, set: { loginItems.setEnabled($0) }))
            .disabled(!loginItems.available)
        if let note = loginItems.note { Text(note).font(.caption).foregroundStyle(.secondary) }
        if loginItems.needsApproval { Button("Open Login Items Settings") { loginItems.openSystemSettings() } }
        if !loginItems.available {
            Text("Only the installed FERB.app opens at login.").font(.caption).foregroundStyle(.secondary)
        }
    }
}
```

- [ ] **Step 4: `AppSetupController.onSettled`** (plan C's file)

After `    var onFinished: (() -> Void)?` add:

```swift
    /// Setup has nothing left to do (L34 Phase F): at launch when this build is set up, after a quiet update that
    /// needs no permission, or at the setup window's Done. The app applies the login-item default here (plan G adds
    /// "Updated to …").
    var onSettled: (() -> Void)?
```

In `startAtLaunch()`, replace `            case .nothing: return` with:

```swift
            case .nothing: self.onSettled?(); return
```

In `quietUpdate()`, replace

```swift
                if await self.model.needsPermissions() { self.show(.permissions, action: .none) }
```

with

```swift
                if await self.model.needsPermissions() { self.show(.permissions, action: .none) }
                else { self.onSettled?() }
```

In `show(_:action:)`, replace `                self?.onFinished?()` (inside `model.onFinish = { … }`) with:

```swift
                self?.onFinished?()
                self?.onSettled?()
```

- [ ] **Step 5: Settings → General → Startup, Uninstall, wiring**

`FERBWindow.swift`, in `FERBAppActions`, after `var beforeUninstall: (() -> Void)?` add:

```swift
    /// Plan F's: Settings → General → Startup.
    var loginItems: LoginItemController?
```

`SettingsPages.swift`: in `GeneralSettingsPage` add a property after `let cursor: CursorController`:

```swift
    var loginItems: LoginItemController? = nil
```

and as the first section of its `Form`:

```swift
            if let loginItems {
                Section("Startup") { LoginItemRow(loginItems: loginItems) }
            }
```

In `SettingsView.body`, change `GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor)` to
`GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor, loginItems: actions.loginItems)`.

`Application.swift`: add a property `    private var loginItems: LoginItemController?`. In plan E's actions block, right
after `var actions = FERBAppActions(...)`, add:

```swift
            let loginItems = LoginItemController.installed(available: appSetup.isPackaged)
            self.loginItems = loginItems
            actions.loginItems = loginItems
            // Uninstall unregisters macOS's login item first; `ferb uninstall` then removes the fallback file
            actions.beforeUninstall = { [loginItems] in loginItems.removeForUninstall() }
            appSetup.onSettled = { [loginItems] in loginItems.applyDefaultAfterSetup() }
```

(`appSetup.startAtLaunch()` runs after this block, so `onSettled` is set before setup settles.)

- [ ] **Step 6: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter LoginItemControllerTests`
Expected: PASS. Then `swift test --package-path ui/mac/FERBNotch` (plan C's setup tests and plan E's window tests
still pass: `onSettled` is nil there, and `GeneralSettingsPage` without login items lays out as before).

- [ ] **Step 7: Commit**

```bash
git add ui/mac/FERBNotch/Sources/FERBNotch/LoginItemController.swift ui/mac/FERBNotch/Sources/FERBNotch/AppSetupController.swift ui/mac/FERBNotch/Sources/FERBNotch/FERBWindow.swift ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift ui/mac/FERBNotch/Sources/FERBNotch/Application.swift ui/mac/FERBNotch/Tests/NotchCoreTests/LoginItemControllerTests.swift
git diff --cached --stat
git commit -m "feat(L34): FERB.app opens at login: macOS's login item, a LaunchAgent only when macOS refuses it, on once after setup

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `ferb uninstall` removes the fallback; documentation; the owner's live check

**Files:**
- Modify: `ferb/device/install.py` (constants after `LABEL` at `:35`; `uninstall` at `:517-539`)
- Create: `tests/test_login_item_uninstall.py`
- Modify: `README.md`, `ARCHITECTURE.md`, `CHANGELOG.md`, `docs/0.27_DECISIONS.toml`, `docs/OWNER_ACCEPTANCE.md`

**Interfaces:**
- Produces: `ferb.device.install.APP_LOGIN_LABEL = "com.ferb.FERB.login"`; `Installer.uninstall()` also removes
  `<launch_agents>/com.ferb.FERB.login.plist` when its `Label` is that.

- [ ] **Step 1: Write the failing tests** (create `tests/test_login_item_uninstall.py`)

```python
"""L34 Phase F: `ferb uninstall` removes FERB.app's login LaunchAgent (the fallback when macOS refuses the app's login
item), and only a file FERB wrote."""

import plistlib
import subprocess
from pathlib import Path

from ferb.device.install import APP_LOGIN_LABEL, Installer


def _quiet(cmd: list[str]) -> "subprocess.CompletedProcess[str]":
    """launchctl answers 'not loaded'; nothing else runs."""
    return subprocess.CompletedProcess(cmd, 113, "", "")


def _inst(tmp_path: Path) -> Installer:
    return Installer(tmp_path / ".ferb", run=_quiet, launch_agents=tmp_path / "LaunchAgents")


def test_the_label_matches_the_apps_plan() -> None:
    source = (Path(__file__).resolve().parent.parent / "ui" / "mac" / "FERBNotch" / "Sources" / "NotchCore"
              / "LoginItemPlan.swift").read_text()
    assert f'public static let label = "{APP_LOGIN_LABEL}"' in source


def test_uninstall_removes_the_apps_login_agent(tmp_path: Path) -> None:
    inst = _inst(tmp_path)
    login = tmp_path / "LaunchAgents" / f"{APP_LOGIN_LABEL}.plist"
    login.parent.mkdir(parents=True)
    login.write_bytes(plistlib.dumps({"Label": APP_LOGIN_LABEL, "ProgramArguments": ["/usr/bin/open", "-g", "-a", "x"]}))
    removed = inst.uninstall()
    assert not login.exists() and str(login) in removed


def test_uninstall_leaves_a_file_with_another_label(tmp_path: Path) -> None:
    inst = _inst(tmp_path)
    login = tmp_path / "LaunchAgents" / f"{APP_LOGIN_LABEL}.plist"
    login.parent.mkdir(parents=True)
    login.write_bytes(plistlib.dumps({"Label": "com.example.mine"}))
    inst.uninstall()
    assert login.exists()
    login.write_text("not a plist")
    inst.uninstall()
    assert login.exists()
```

- [ ] **Step 2: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_login_item_uninstall.py -q`
Expected: FAIL: `ImportError: cannot import name 'APP_LOGIN_LABEL'`.

- [ ] **Step 3: Implement** (`ferb/device/install.py`)

After `LABEL = "com.ferb.daemon"` add:

```python
# FERB.app's login LaunchAgent: the fallback the app writes when macOS refuses its login item (L34 Phase F;
# ui/mac/FERBNotch/Sources/NotchCore/LoginItemPlan.swift). Uninstall removes it, and only a file with this label.
APP_LOGIN_LABEL = "com.ferb.FERB.login"
```

In `uninstall`, after the block that removes `self.plist_path`, add:

```python
        login = self.launch_agents / f"{APP_LOGIN_LABEL}.plist"
        if _labelled(login, APP_LOGIN_LABEL):
            login.unlink()
            removed.append(str(login))
```

and at module level (after `read_only_volume`):

```python
def _labelled(path: Path, label: str) -> bool:
    """A launchd plist FERB wrote: it exists and names `label` (anything else at that path is left alone)."""
    try:
        return plistlib.loads(path.read_bytes()).get("Label") == label
    except (OSError, ValueError, AttributeError):          # missing, not a plist (InvalidFileException), not a dict
        return False
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `nice -n 10 uv run pytest tests/test_login_item_uninstall.py tests/test_install.py -q`
Expected: PASS.

- [ ] **Step 5: Documentation**

README, "FERB.app (disk image)" section, after the paragraph that starts "In Privacy & Security, FERB appears as":

```markdown
After setup, FERB.app opens at login by itself (Settings → General → **Open FERB at login**, on by default once; turn
it off there or in System Settings → General → Login Items). FERB uses macOS's own login item; if macOS refuses it for
FERB's self-made signature, FERB adds `~/Library/LaunchAgents/com.ferb.FERB.login.plist`, which opens FERB in the
background at login, and Settings says so. The background service starts at login either way.
```

ARCHITECTURE.md, after plan E's section:

```markdown
### Launch at login (L34 Phase F)

`LoginItemController` (`ui/mac/FERBNotch/Sources/FERBNotch/LoginItemController.swift`) tries
`SMAppService.mainApp.register()`; when it throws, or returns with the item neither on nor waiting for the owner's OK,
it writes `~/Library/LaunchAgents/com.ferb.FERB.login.plist` (`LoginItemPlan.agent`: `open -g -a <FERB.app>`,
`RunAtLoad`, Aqua) and keeps macOS's refusal for the Settings note. `.requiresApproval` is not a refusal (no second
launcher). The default applies once (`loginItem.defaultApplied`) when `AppSetupController.onSettled` fires: at launch
on a set-up build, after a quiet update that needs no permission, or at the setup window's Done. FERB's shell can't
write `~/Library/LaunchAgents` (never-write) or start launchd jobs; `ferb uninstall` removes the fallback file when its
label is FERB's, and the app's Uninstall unregisters macOS's item first.
```

CHANGELOG.md, under the L34 entry:

```markdown
Phase F, launch at login (L34): FERB.app opens at login, on by default once after setup (Settings → General → Open FERB
at login). macOS's login item (`SMAppService.mainApp`) first; when macOS refuses it, a user LaunchAgent
(`com.ferb.FERB.login`) opens FERB in the background, and Settings says so. `ferb uninstall` removes that file.
```

`docs/0.27_DECISIONS.toml`: append to `[L34] tests`: `"tests/test_login_item_uninstall.py"`,
`"ui/mac/FERBNotch/Tests/NotchCoreTests/LoginItemPlanTests.swift"`,
`"ui/mac/FERBNotch/Tests/NotchCoreTests/LoginItemControllerTests.swift"`; append to its `note`: ` Phase F ruling (not
the owner's): macOS's login item first, the LaunchAgent only on refusal; the live check on a second macOS user records
which a self-signed build gets.`

`docs/OWNER_ACCEPTANCE.md`, append:

```markdown
## Launch at login (L34 Phase F) — pending live check (2026-10-25)

On a fresh second macOS user, with a DMG built and signed with plan B's certificate (build N), the owner watching:

1. Install, Open Anyway, finish setup. **Expected:** Settings → General → Startup shows "Open FERB at login" on. Record
   which path: System Settings → General → Login Items lists FERB under **Open at Login** (macOS's item; no note) or
   FERB appears under **Allow in the Background** and Settings' note says "macOS refused FERB's login item (…)" (the
   LaunchAgent). Copy the note's text.
2. Log out and in. **Expected:** the notch appears without opening FERB; nothing takes focus; `ferb doctor` shows the
   daemon running.
3. Settings → General: turn it off; log out and in. **Expected:** FERB.app doesn't open (the daemon still runs). Turn it
   back on.
4. Install build N+1 over it (same certificate). **Expected:** still on after the next login; one FERB process
   (`pgrep -lf FERBNotch`).
5. Settings → Advanced → Uninstall FERB. **Expected:** Login Items no longer lists FERB;
   `~/Library/LaunchAgents/com.ferb.FERB.login.plist` is gone.
```

- [ ] **Step 6: Full verification**

Run: `nice -n 10 uv run pytest -q`; `uv run mypy ferb/core`; `swift test --package-path ui/mac/FERBNotch`;
`git diff --check`. Expected: all pass, no `git diff --check` output.

- [ ] **Step 7: Commit**

```bash
git add ferb/device/install.py tests/test_login_item_uninstall.py README.md ARCHITECTURE.md CHANGELOG.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git diff --cached --stat
git commit -m "feat(L34): ferb uninstall removes FERB.app's login LaunchAgent; launch-at-login docs and the owner's live check

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
