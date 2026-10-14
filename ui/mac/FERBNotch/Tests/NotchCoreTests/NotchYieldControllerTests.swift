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

    /// The same frames PanelCoordinator derives from the model: the panel window while expanded, and the drawn
    /// surface for a pinned chat.
    private func layout() -> NotchYieldController.Layout {
        let surface = model.bodyVisible ? F.openSurface : F.headerSurface
        let panel = model.bodyVisible ? F.openPanel : F.headerPanel
        return .init(occupied: model.expanded ? [panel] : [],
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
        XCTAssertTrue(controller.pointerMoved(to: CGPoint(x: 735, y: 200)))
        XCTAssertTrue(controller.pointerMoved(to: browserPoint))
        XCTAssertFalse(controller.pointerMoved(to: notchPoint))
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
        XCTAssertTrue(controller.pointerMoved(to: CGPoint(x: 900, y: 100)))
        XCTAssertTrue(controller.pointerMoved(to: CGPoint(x: 735, y: 200)))
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
        XCTAssertFalse(controller.pointerMoved(to: notchPoint))
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

    func testOnlyEnteringTheNotchCountsNotAPointerAlreadyRestingThere() {
        model.expand()
        windows = [F.shareBar]
        controller.evaluate()
        controller.yieldBegan(pointer: notchPoint)
        XCTAssertTrue(controller.pointerMoved(to: CGPoint(x: 740, y: 12)))
        XCTAssertTrue(controller.pointerMoved(to: CGPoint(x: 735, y: 300)))
        XCTAssertFalse(controller.pointerMoved(to: notchPoint))
    }

    func testTheNotchDoesNotSummonFERBWhereAnotherAppsMenuReachesUnderIt() {
        model.expand()
        let menuUnderNotch = F.window(F.clipboardApp, layer: 101, 640, 33, 200, 300)
        windows = [menuUnderNotch]
        controller.evaluate()
        let onMenuEdge = CGPoint(x: 700, y: 33.5)
        XCTAssertTrue(controller.pointerMoved(to: onMenuEdge))
        XCTAssertFalse(controller.ownerClicked(at: onMenuEdge))
        XCTAssertTrue(model.isYielded)
        XCTAssertFalse(controller.pointerMoved(to: CGPoint(x: 700, y: 10)))
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
