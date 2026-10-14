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
        // Hover never overrides another app's menu.
        XCTAssertTrue(controller.pointerMoved(to: notchPoint))
        windows = [F.maximizedBrowser]
        XCTAssertTrue(controller.evaluate())
        XCTAssertFalse(model.isYielded)
        XCTAssertTrue(model.expanded)
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
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertTrue(model.isYielded)
        XCTAssertEqual(controller.coveredApp, F.browser)
    }
}
