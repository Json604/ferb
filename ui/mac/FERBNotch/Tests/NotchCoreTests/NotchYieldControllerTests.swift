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
        model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        windows = []
        controller = NotchYieldController(model: model, ownPIDs: [F.ferb], windows: { [unowned self] in self.windows },
                                          layout: { [unowned self] in self.layout() })
    }

    override func tearDown() async throws {
        controller.stop()
    }

    /// The same frames PanelCoordinator derives from the model: FERB's drawn surface while expanded (nothing while
    /// collapsed on a notched display), which is also the pinned chat.
    private func layout() -> NotchYieldController.Layout {
        let surface = model.bodyVisible ? F.openSurface : F.headerSurface
        return .init(occupied: model.expanded ? [surface] : [],
                     pinnedChat: model.isPinned && model.expanded ? surface : nil,
                     notch: F.notch, menuBar: F.menuBar, display: F.display)
    }

    private func pinOpenChat() {
        model.expand()
        model.togglePin()
        model.revealPinnedBody()
    }

    private let notchPoint = CGPoint(x: 735, y: 10)
    private let browserPoint = CGPoint(x: 200, y: 600)

    func testPinnedChatYieldsToAnActivatedAppUnderItAndHoveringTheNotchBringsItBack() {
        pinOpenChat()
        windows = [F.maximizedBrowser]
        controller.appActivated(F.browser)
        XCTAssertEqual(model.yielding.reasons, [.activeAppUnderPinnedChat])
        XCTAssertEqual(controller.coveredApp, F.browser)
        // Moving over the (invisible) chat or the browser keeps FERB out of the way.
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 735, y: 200)), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: browserPoint), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .broughtBack)
        XCTAssertFalse(model.isYielded)
        XCTAssertNil(controller.coveredApp)
        XCTAssertTrue(model.isPinned)
    }

    func testTheFoldedPinnedHeaderSitsInTheMenuBarAndDoesNotYield() {
        model.expand()
        model.togglePin()
        XCTAssertFalse(model.bodyVisible)
        windows = [F.maximizedBrowser]
        controller.appActivated(F.browser)
        XCTAssertFalse(model.isYielded)
    }

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

    func testAClickOnAWindowThatIsNotUnderTheChatDoesNotYield() {
        pinOpenChat()
        let finderWindow = F.window(F.finder, layer: 0, 0, 500, 400, 300)
        windows = [finderWindow, F.maximizedBrowser]
        controller.ownerClicked(at: browserPoint)
        XCTAssertFalse(model.isYielded)
    }

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

    func testHoveringAMenuThatCoversTheNotchDoesNotBringFERBBack() {
        model.expand()
        windows = [F.window(F.clipboardApp, layer: NotchOverlap.popUpMenuLayer, 640, 0, 200, 300)]
        controller.evaluate()
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .hidden)
        XCTAssertFalse(controller.ownerClicked(at: notchPoint))
        XCTAssertTrue(model.isYielded)
    }

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

    func testOnlyEnteringTheNotchCountsNotAPointerAlreadyRestingThere() {
        model.expand()
        windows = [F.shareBar]
        controller.evaluate()
        controller.yieldBegan(pointer: notchPoint)
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 740, y: 12)), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: CGPoint(x: 735, y: 300)), .hidden)
        XCTAssertEqual(controller.pointerMoved(to: notchPoint), .broughtBack)
    }

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

    func testWindowsOverFERBDuringDictationDontHideItAfterwards() {
        model.expand()
        model.session.beginVoicePreview()
        windows = [F.shareBar]
        controller.evaluate()
        model.session.finishVoicePreview()
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
    }

    func testANormalWindowUnderTheUnpinnedChatNeverYields() {
        model.expand()
        windows = [F.maximizedBrowser]
        controller.evaluate()
        controller.appActivated(F.browser)
        controller.ownerClicked(at: browserPoint)
        XCTAssertFalse(model.isYielded)
    }

    func testWithNothingOfFERBOnScreenAMenuNoLongerHidesIt() {
        model.expand()
        windows = [F.clipboardMenu]
        controller.evaluate()
        XCTAssertTrue(model.isYielded)
        model.collapse()
        XCTAssertTrue(controller.evaluate())
        XCTAssertFalse(model.isYielded)
    }

    func testDictationIsNeverHiddenByAnotherAppsMenu() {
        model.expand()
        model.session.beginVoicePreview()
        windows = [F.clipboardMenu]
        controller.evaluate()
        XCTAssertFalse(model.isYielded)
    }

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
