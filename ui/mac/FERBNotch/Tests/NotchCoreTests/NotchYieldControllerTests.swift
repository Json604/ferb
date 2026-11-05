import XCTest
import CoreGraphics
@testable import FERBNotch
import NotchCore

@MainActor
final class NotchYieldControllerTests: XCTestCase {
    private typealias F = YieldFixture
    private var model: NotchModel!
    private var windows: [ScreenWindow] = []
    private var controller: NotchYieldController!

    override func setUp() async throws {
        model = NotchModel(preferences: scratchDefaults())
        windows = []
        controller = NotchYieldController(model: model, ownPIDs: [F.ferb], windows: { [unowned self] in self.windows },
                                          layout: { [unowned self] in self.layout() })
        controller.canBeActive = { _ in true }                  // the fixture's processes are ordinary apps
        controller.frontmostApp = { nil }
        controller.onAnotherSpace = { _ in false }              // fixture windows are on the Space showing
        controller.isWindowManager = { _ in false }
    }

    override func tearDown() async throws {
        controller.stop()
    }

    /// The same frames PanelCoordinator derives from the model: FERB's drawn surface while expanded (nothing while
    /// collapsed on a notched display), which is also the pinned chat.
    private func layout() -> NotchYieldController.Layout {
        let surface = model.bodyVisible || model.introLine != nil ? F.openSurface : F.headerSurface
        return .init(occupied: model.expanded || model.introLine != nil ? [surface] : [],
                     pinnedChat: (model.isPinned && model.expanded) || model.introLine != nil ? surface : nil,
                     notch: F.notch, menuBar: F.menuBar, display: F.display)
    }

    private func pinOpenChat() {
        model.expand()
        model.togglePin()
        model.revealPinnedBody()
    }

    private let notchPoint = CGPoint(x: 735, y: 10)
    private let browserPoint = CGPoint(x: 200, y: 600)

    // decision: APP-614
    func testIntroYieldsToSettingsAndItsSameProcessCompanionCanBeClickedForwardAgain() {
        model.introLine = IntroLine(text: "Permissions")
        windows = [F.maximizedBrowser]
        controller.appActivated(F.browser)
        XCTAssertTrue(model.isYielded)
        controller.summon()
        controller.companionFocused(123)
        XCTAssertEqual(controller.coveringWindow(), 123)
        XCTAssertTrue(model.isYielded)
        controller.summon()
        XCTAssertFalse(model.isYielded)
        controller.companionFocused(123)
        XCTAssertTrue(model.isYielded)
        controller.companionFocused(nil)
        XCTAssertFalse(model.isYielded)
        controller.companionFocused(123)
        model.introLine = nil
        XCTAssertFalse(model.isYielded)
    }

    /// Owner, 2026-11-04: "ferb comes forward now, but doesnt go back. even press on settings". After a grant the tour
    /// comes forward over System Settings (APP-687); a click into Settings' pane lands on a window its Privacy &
    /// Security extension owns (a process that can never be active), which has nothing under the tour, so FERB stayed
    /// in front. The click counts for the frontmost app, Settings, at every grant.
    // decision: APP-690
    func testAClickIntoSettingsPaneDrawnByItsExtensionStepsBehindSettingsAfterEachAcknowledgement() {
        let settings: Int32 = 500
        let pane: Int32 = 501                                   // the extension drawing the pane
        let settingsWindow = F.window(settings, layer: 0, 400, 40, 700, 600, id: 21)
        let paneWindow = F.window(pane, layer: 0, 620, 440, 480, 200, id: 22)   // below the tour's surface
        windows = [paneWindow, settingsWindow, F.window(F.finder, layer: 0, 1200, 600, 200, 200)]
        controller.canBeActive = { $0 != pane }
        controller.frontmostApp = { settings }
        model.introLine = IntroLine(text: "Turn FERB on")
        let inPane = CGPoint(x: 800, y: 520)
        for _ in 0..<2 {
            controller.appActivated(settings)                   // the grant's dock steps behind Settings (APP-642)
            XCTAssertTrue(model.isYielded)
            controller.summon()                                 // macOS confirmed it: forward to say so (APP-687)
            XCTAssertFalse(model.isYielded)
            controller.ownerClicked(at: inPane)                 // the owner clicks into Settings' pane
            XCTAssertTrue(model.isYielded)
            XCTAssertEqual(controller.coveredApp, settings)
            XCTAssertEqual(controller.coveringWindow(), 21)
        }
        // A regular app's own window with nothing under the tour still never yields to the frontmost app.
        controller.summon()
        controller.ownerClicked(at: CGPoint(x: 1300, y: 700))
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testPinnedChatYieldsToAnActivatedAppUnderItAndHoveringTheNotchBringsItBack() {
        pinOpenChat()
        windows = [F.maximizedBrowser]
        controller.appActivated(F.browser)
        XCTAssertEqual(model.yielding.reasons, [.activeAppUnderPinnedChat])
        XCTAssertEqual(controller.coveredApp, F.browser)
        // Moving over the stepped-back chat or the browser keeps FERB out of the way.
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 735, y: 200)), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: browserPoint), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .broughtBack)
        XCTAssertFalse(model.isYielded)
        XCTAssertNil(controller.coveredApp)
        XCTAssertTrue(model.isPinned)
    }

    /// The panel steps back just behind the covered app's frontmost window under the chat (owner, 2026-10-24),
    /// so that window is known when the yield applies; a menu over FERB has no window to sit behind.
    // decision: APP-559
    func testTheWindowToStepBehindIsKnownWhenTheYieldApplies() {
        pinOpenChat()
        let front = F.window(F.browser, layer: 0, 100, 40, 1000, 800, id: 11)
        let back = F.window(F.browser, layer: 0, 0, 33, 1470, 923, id: 12)
        windows = [front, back]
        var seen: [Int?] = []
        model.onYieldChange = { [unowned self] yielded in if yielded { seen.append(self.controller.coveringWindow()) } }
        controller.appActivated(F.browser)
        XCTAssertEqual(seen, [11])
        windows = [back]                                  // its front window closed: the next one is behind it
        XCTAssertEqual(controller.coveringWindow(), 12)
        controller.summon()
        XCTAssertNil(controller.coveringWindow())

        windows = [F.clipboardMenu]
        controller.evaluate()
        XCTAssertEqual(model.yielding.reasons, [.foreignOverlay])
        XCTAssertNil(controller.coveringWindow())
    }

    /// Owner, 2026-11-05: "IT SHOULD STICK". macOS activates the new Space's front app as the owner switches Spaces;
    /// that app wasn't brought forward over FERB, so a pinned chat stays in front of its window. The same app brought
    /// forward afterwards still makes the chat step back (APP-559).
    // decision: APP-729, APP-732, APP-559
    func testTheNewSpacesFrontAppDoesNotMakeThePinnedChatStepBack() {
        pinOpenChat()
        windows = [F.maximizedBrowser]
        XCTAssertFalse(controller.appActivated(F.browser, bySpaceChange: true))
        XCTAssertFalse(model.isYielded)
        XCTAssertNil(controller.coveredApp)
        controller.appActivated(F.browser)
        XCTAssertEqual(model.yielding.reasons, [.activeAppUnderPinnedChat])
    }

    /// After a Space change the floating windows already over FERB on the new Space don't make it step aside (as when
    /// the owner opens FERB over them); one that newly covers FERB afterwards, and a menu, still do.
    // decision: APP-729, APP-732, APP-559
    func testWindowsAlreadyOverFERBOnTheNewSpaceAreOverriddenNewOnesStillYield() {
        model.expand()
        controller.evaluate()                                   // the open chat, nothing over it
        controller.spaceChanged()
        windows = [F.shareBar]                                  // already on the new Space, over the chat
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
        windows = [F.shareBar, F.clipboardMenu]                 // a menu opens over FERB afterwards
        controller.evaluate()
        XCTAssertEqual(model.yielding.reasons, [.foreignOverlay])
        windows = [F.shareBar]
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
    }

    /// Mid-swipe the window list still has the other Space's windows sliding across the display: a window that belongs
    /// only to another Space is never over FERB.
    // decision: APP-729, APP-732
    func testAWindowOfAnotherSpaceSlidingAcrossFERBDoesNotYield() {
        model.expand()
        let sliding = F.window(F.clipboardApp, layer: 3, 500, 40, 400, 300, id: 77)
        controller.onAnotherSpace = { $0 == 77 }
        windows = [sliding]
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
        controller.onAnotherSpace = { _ in false }              // the same window on the Space showing
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
    }

    /// Live 2026-11-05: with the chat open, Mission Control's Spaces bar (the window manager's layer-14 strip over the
    /// top of the display) counted as over FERB; the stepped-back panel became one of Mission Control's thumbnails.
    /// While Mission Control shows nothing counts; afterwards a menu over FERB still does.
    // decision: APP-729, APP-734
    func testNothingCountsAsOverFERBWhileMissionControlShows() {
        let manager: Int32 = 990
        controller.isWindowManager = { $0 == manager }
        model.expand()
        let spacesBar = F.window(manager, layer: 14, 0, 0, 1470, 128)
        let mission = F.window(manager, layer: 19, 0, 0, 1470, 956)
        windows = [mission, spacesBar, F.clipboardMenu]
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
        windows = [F.clipboardMenu]
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
    }

    // decision: APP-559
    func testTheFoldedPinnedHeaderSitsInTheMenuBarAndDoesNotYield() {
        model.expand()
        model.togglePin()
        XCTAssertFalse(model.bodyVisible)
        windows = [F.maximizedBrowser]
        controller.appActivated(F.browser)
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testAnAppWithNothingUnderTheChatOrAnUnpinnedChatDoesNotYield() {
        pinOpenChat()
        windows = [F.window(F.finder, layer: 0, 0, 500, 400, 300)]
        controller.appActivated(F.finder)
        XCTAssertFalse(model.isYielded)
        model.togglePin()
        windows = [F.maximizedBrowser]
        controller.appActivated(F.browser)
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testFERBComesBackWhenTheAppsWindowsUnderTheChatGoAway() {
        pinOpenChat()
        windows = [F.maximizedBrowser]
        controller.appActivated(F.browser)
        XCTAssertFalse(controller.evaluate())
        XCTAssertTrue(model.isYielded)
        windows = [F.window(F.browser, layer: 0, 0, 500, 400, 300)]
        XCTAssertTrue(controller.evaluate())
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testFERBComingForwardItsShortcutOrAnotherAppNotUnderTheChatBringsItBack() {
        pinOpenChat()
        windows = [F.maximizedBrowser, F.window(F.finder, layer: 0, 0, 500, 400, 300)]
        controller.appActivated(F.browser)
        XCTAssertTrue(controller.appActivated(F.ferb))
        controller.appActivated(F.browser)
        XCTAssertTrue(controller.summon())
        controller.appActivated(F.browser)
        XCTAssertTrue(controller.appActivated(F.finder))
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testAClickIntoTheFrontmostAppUnderTheChatYieldsAndAClickOnTheNotchBringsFERBBack() {
        // An answer opened the pinned chat over the browser the owner is using: no app activation happens.
        pinOpenChat()
        windows = [F.maximizedBrowser]
        XCTAssertFalse(controller.ownerClicked(at: browserPoint))
        XCTAssertTrue(model.isYielded)
        XCTAssertFalse(controller.ownerClicked(at: CGPoint(x: 300, y: 700)))
        XCTAssertTrue(model.isYielded)
        XCTAssertTrue(controller.ownerClicked(at: notchPoint))
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testAClickOnAWindowThatIsNotUnderTheChatDoesNotYield() {
        pinOpenChat()
        let finderWindow = F.window(F.finder, layer: 0, 0, 500, 400, 300)
        windows = [finderWindow, F.maximizedBrowser]
        controller.ownerClicked(at: browserPoint)
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testAnotherAppsMenuOverFERBYieldsUntilItCloses() {
        model.expand()
        windows = [F.maximizedBrowser, F.clipboardMenu]
        XCTAssertFalse(controller.evaluate())
        XCTAssertEqual(model.yielding.reasons, [.foreignOverlay])
        // Moving through the menu or over the hidden chat doesn't bring FERB back.
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 900, y: 100)), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 735, y: 200)), .hidden)
        windows = [F.maximizedBrowser]
        XCTAssertTrue(controller.evaluate())
        XCTAssertFalse(model.isYielded)
        XCTAssertTrue(model.expanded)
    }

    // decision: APP-559
    func testEnteringTheNotchShowsFERBOverAWindowThatNeverClosesUntilANewOneAppears() {
        model.expand()
        windows = [F.shareBar]
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .broughtBack)
        XCTAssertEqual(controller.overriddenOverlays, [F.shareBar.windowID])
        XCTAssertFalse(controller.evaluate())
        XCTAssertFalse(model.isYielded)
        // A menu that newly opens over FERB still hides it, and FERB comes back over the bar once it closes.
        windows = [F.shareBar, F.clipboardMenu]
        controller.evaluate()
        XCTAssertEqual(model.yielding.reasons, [.foreignOverlay])
        windows = [F.shareBar]
        XCTAssertTrue(controller.evaluate())
        // Collapsing and reopening over the same bar keeps the override.
        model.collapse()
        controller.evaluate()
        model.expand()
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
        // The bar going away forgets it; a new bar hides FERB again.
        windows = []
        controller.evaluate()
        windows = [F.window(800, layer: 3, 600, 36, 280, 40, id: 77)]
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
    }

    // decision: APP-559
    func testOpeningFERBOverAWindowThatIsAlreadyThereKeepsItShown() {
        // Collapsed on a notched display nothing of FERB is drawn, so there is nothing to check yet.
        windows = [F.shareBar]
        controller.evaluate()
        // The owner opens FERB (shortcut, menu, click or the opening hover); the coordinator reports it when the
        // body opens in a presentation that asks for focus.
        controller.ownerOpened()
        model.expand()
        XCTAssertFalse(controller.evaluate())
        XCTAssertFalse(model.isYielded)
        XCTAssertEqual(controller.overriddenOverlays, [F.shareBar.windowID])
        // Only the windows found at that first check: a menu that opens afterwards still hides FERB.
        windows = [F.shareBar, F.clipboardMenu]
        controller.evaluate()
        XCTAssertEqual(model.yielding.reasons, [.foreignOverlay])
    }

    // decision: APP-559
    func testRevealingThePinnedChatOverAWindowKeepsItShown() {
        model.expand()
        model.togglePin()
        windows = [F.shareBar]
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
        // Hovering the folded header reveals the body over the bar.
        controller.ownerOpened()
        model.revealPinnedBody()
        XCTAssertFalse(controller.evaluate())
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testAnOverrideLastsWhileTheWindowIsOnScreenEvenWhenItNoLongerOverlapsFERB() {
        pinOpenChat()
        windows = [F.maximizedBrowser, F.shareBar]
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
        _ = controller.pointerMoved(to: notchPoint)
        XCTAssertFalse(model.isYielded)
        // The body folds away from the bar, then comes back over it (an arriving question, not a reach).
        model.foldPinnedBody()
        controller.evaluate()
        model.announceAttention("test:override-survives-fold")
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
        // Once the bar leaves the screen it is forgotten.
        windows = [F.maximizedBrowser]
        controller.evaluate()
        XCTAssertTrue(controller.overriddenOverlays.isEmpty)
    }

    // decision: APP-559
    func testHoverOrAClickOnTheNotchOverridesAWindowThatCoversIt() {
        model.expand()
        windows = [F.window(810, layer: 25, 620, 0, 230, 60)]   // another notch app
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .broughtBack)
        windows = [F.window(811, layer: 27, 620, 0, 230, 60)]
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
        XCTAssertTrue(controller.ownerClicked(at: notchPoint))
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testHoveringAMenuThatCoversTheNotchDoesNotBringFERBBack() {
        model.expand()
        windows = [F.window(F.clipboardApp, layer: NotchOverlap.popUpMenuLayer, 640, 0, 200, 300)]
        controller.evaluate()
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .hidden)
        XCTAssertFalse(controller.ownerClicked(at: notchPoint))
        XCTAssertTrue(model.isYielded)
    }

    // decision: APP-559
    func testSystemInputWindowsOverFERBNeverHideIt() {
        // An accent popup, emoji palette or input-method candidates opened from FERB's own composer belong to a
        // system input process, not to FERB.
        controller.bundleID = { $0 == 950 ? "com.apple.PressAndHold" : nil }
        model.expand()
        windows = [F.window(950, layer: NotchOverlap.popUpMenuLayer, 700, 300, 220, 40)]
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
        windows.append(F.clipboardMenu)
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
    }

    // decision: APP-559
    func testOnlyEnteringTheNotchCountsNotAPointerAlreadyRestingThere() {
        model.expand()
        windows = [F.shareBar]
        controller.evaluate()
        controller.yieldBegan(pointer: notchPoint)
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 740, y: 12)), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 735, y: 300)), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .broughtBack)
    }

    // decision: APP-559
    func testTheNotchDoesNotSummonFERBWhereAnotherAppsMenuReachesUnderIt() {
        model.expand()
        let menuUnderNotch = F.window(F.clipboardApp, layer: 101, 640, 33, 200, 300)
        windows = [menuUnderNotch]
        controller.evaluate()
        let onMenuEdge = CGPoint(x: 700, y: 33.5)
        XCTAssertEqual(controller.pointerMoved(to: onMenuEdge), .hidden)
        XCTAssertFalse(controller.ownerClicked(at: onMenuEdge))
        XCTAssertTrue(model.isYielded)
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 700, y: 10)), .broughtBack)
    }

    // decision: APP-559
    func testTheShortcutOrAClickOnTheNotchOverridesAnotherAppsWindow() {
        model.expand()
        windows = [F.shareBar]
        controller.evaluate()
        XCTAssertTrue(controller.summon())
        XCTAssertFalse(controller.evaluate())
        XCTAssertFalse(model.isYielded)
        windows = [F.window(800, layer: 3, 600, 36, 280, 40, id: 78)]
        controller.evaluate()
        XCTAssertTrue(controller.ownerClicked(at: notchPoint))
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testWindowsOverFERBDuringDictationDontHideItAfterwards() {
        model.expand()
        model.session.beginVoicePreview()
        windows = [F.shareBar]
        controller.evaluate()
        model.session.finishVoicePreview()
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testANormalWindowUnderTheUnpinnedChatNeverYields() {
        model.expand()
        windows = [F.maximizedBrowser]
        controller.evaluate()
        controller.appActivated(F.browser)
        controller.ownerClicked(at: browserPoint)
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testWithNothingOfFERBOnScreenAMenuNoLongerHidesIt() {
        model.expand()
        windows = [F.clipboardMenu]
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
        model.collapse()
        XCTAssertTrue(controller.evaluate())
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testDictationIsNeverHiddenByAnotherAppsMenu() {
        model.expand()
        model.session.beginVoicePreview()
        windows = [F.clipboardMenu]
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testAWindowThatAppearsJustAfterActivationStillYields() async throws {
        pinOpenChat()
        controller.activationRecheckDelay = .milliseconds(10)
        controller.appActivated(F.browser)
        XCTAssertFalse(model.isYielded)
        windows = [F.maximizedBrowser]
        // Wait on the condition, not a fixed time, so a busy machine can't fail this.
        for _ in 0..<300 where !model.isYielded { try await Task.sleep(for: .milliseconds(10)) }
        XCTAssertTrue(model.isYielded)
        XCTAssertEqual(controller.coveredApp, F.browser)
    }
}
