import AppKit
import XCTest
@testable import FERBNotch

@MainActor
final class NotchWindowYieldTests: XCTestCase {
    private var panel: FakeNotchWindow!
    private var tracer: FakeNotchWindow!
    private var rejoined: [String] = []
    private var left: [String] = []
    private var yield: NotchWindowYield!
    private let ownLevel = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 1)

    override func setUp() async throws {
        panel = FakeNotchWindow("panel")
        panel.level = ownLevel
        tracer = FakeNotchWindow("tracer", visible: false)
        rejoined = []
        left = []
        yield = NotchWindowYield(panel: panel, tracer: tracer, rejoinOverlay: { [unowned self] window in
            self.rejoined.append((window as! FakeNotchWindow).name)
        }, leaveOverlay: { [unowned self] window in
            self.left.append((window as! FakeNotchWindow).name)
        })
    }

    /// Owner, 2026-10-24: "take it one layer behind rather than closing it back". The panel stays on screen, just
    /// behind the app window it yields to, and comes back at its own level in the overlay space.
    func testYieldStepsThePanelOneLayerBackAndBringsItBackIntoTheOverlaySpace() {
        panel.isKeyWindow = true
        yield.yield(behind: 42)
        XCTAssertTrue(panel.isVisible)
        XCTAssertEqual(panel.level, .normal)
        XCTAssertEqual(left, ["panel"])
        XCTAssertEqual(yield.behind, 42)
        yield.comeBack(refocus: true)
        XCTAssertTrue(panel.isVisible)
        XCTAssertEqual(panel.level, ownLevel)
        XCTAssertTrue(panel.isKeyWindow)
        XCTAssertEqual(panel.log, ["below 42", "front", "key"])
        XCTAssertEqual(rejoined, ["panel"])
        XCTAssertNil(yield.behind)
        XCTAssertEqual(panel.mouseTransparencyWrites, 0)
    }

    /// Yielding to a menu, popover or floating window (all above the normal level) or to FERB's file picker, the
    /// panel stays in front of ordinary windows, under that surface.
    func testWithNoAppWindowToSitBehindThePanelStaysInFrontOfOrdinaryWindows() {
        yield.yield(behind: nil)
        XCTAssertTrue(panel.isVisible)
        XCTAssertEqual(panel.level, .normal)
        XCTAssertEqual(panel.log, ["front"])
    }

    func testRestackingMovesThePanelOnlyWhenWhatItYieldsToChanged() {
        yield.restack(behind: 7)                       // not yielding: nothing moves
        XCTAssertEqual(panel.log, [])
        yield.yield(behind: nil)
        yield.restack(behind: nil)
        yield.restack(behind: 42)                      // an app window came over the chat
        yield.restack(behind: 42)
        yield.restack(behind: 43)                      // another of its windows came forward
        XCTAssertEqual(panel.log, ["front", "below 42", "below 43"])
        XCTAssertEqual(yield.behind, 43)
    }

    func testComingBackTakesFocusOnlyIfItHadItAndTheOwnerStayed() {
        yield.yield(behind: nil)
        yield.comeBack(refocus: true)
        XCTAssertFalse(panel.isKeyWindow)

        panel.isKeyWindow = true
        yield.yield(behind: 42)
        panel.isKeyWindow = false                      // the owner's click into the other app took the focus
        yield.ownerMovedElsewhere()
        yield.comeBack(refocus: true)
        XCTAssertFalse(panel.isKeyWindow)

        panel.isKeyWindow = true
        yield.yield(behind: 42)
        panel.isKeyWindow = false
        yield.comeBack(refocus: false)
        XCTAssertFalse(panel.isKeyWindow)
    }

    func testTheOwnerBringingFERBBackFocusesTheVisibleInputButNeverWhileYielded() {
        yield.yield(behind: 42)
        XCTAssertFalse(yield.focusInput(bodyVisible: true))
        XCTAssertFalse(panel.isKeyWindow)
        yield.comeBack(refocus: false)
        XCTAssertFalse(yield.focusInput(bodyVisible: false))
        XCTAssertFalse(panel.isKeyWindow)
        XCTAssertTrue(yield.focusInput(bodyVisible: true))
        XCTAssertTrue(panel.isKeyWindow)
    }

    /// A presentation while FERB yields (a background reply resizing the chat) leaves the panel where it stepped
    /// back to; one that finds it off screen (after the displays slept) puts it back there, not on top.
    func testPresentationsKeepAYieldedPanelWhereItSteppedBack() {
        yield.yield(behind: 42)
        yield.showPanel()
        XCTAssertEqual(panel.log, ["below 42"])
        panel.isVisible = false
        yield.showPanel()
        XCTAssertEqual(panel.log, ["below 42", "below 42"])
        XCTAssertEqual(panel.level, .normal)
        yield.comeBack(refocus: false)
        yield.showPanel()
        XCTAssertTrue(panel.isVisible)
        XCTAssertEqual(rejoined, ["panel"])
    }

    func testOnlyTheAttentionLightStaysOnScreenWhileYielded() {
        yield.yield(behind: nil)
        yield.updateTracer(glowing: true, lightShows: false)
        XCTAssertFalse(tracer.isVisible)
        yield.updateTracer(glowing: true, lightShows: true)
        XCTAssertTrue(tracer.isVisible)
        XCTAssertEqual(rejoined, ["tracer"])
        yield.updateTracer(glowing: false, lightShows: true)
        XCTAssertFalse(tracer.isVisible)
        XCTAssertEqual(tracer.mouseTransparencyWrites, 0)
    }

    /// Once `ignoresMouseEvents` is set on a window, even back to false, AppKit stops passing clicks through its
    /// transparent areas. The only writes allowed are FERB's click-through windows' own, each always true: the
    /// light (NotchLightPanel), the pointer overlay (CursorPanel) and the intro's orb (OrbPanel), at creation and in
    /// the setter that keeps it on (D31 fix round 4). The notch's own panel is never written.
    func testNoCodeMakesTheNotchPanelMouseTransparent() throws {
        let sources = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Sources/FERBNotch")
        let files = try FileManager.default.contentsOfDirectory(at: sources, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "swift" }
        XCTAssertFalse(files.isEmpty)
        var writes: [String] = []
        for file in files {
            for line in try String(contentsOf: file, encoding: .utf8).split(separator: "\n") {
                let code = line.trimmingCharacters(in: .whitespaces)
                guard !code.hasPrefix("//"), code.contains("ignoresMouseEvents") else { continue }
                if code.range(of: #"ignoresMouseEvents\s*="#, options: .regularExpression) != nil { writes.append(code) }
            }
        }
        XCTAssertEqual(writes.sorted(), ["ignoresMouseEvents = true", "ignoresMouseEvents = true",
                                         "ignoresMouseEvents = true",
                                         "set { super.ignoresMouseEvents = true }",
                                         "set { super.ignoresMouseEvents = true }",
                                         "set { super.ignoresMouseEvents = true }"])
    }
}

@MainActor
private final class FakeNotchWindow: NotchYieldWindow {
    let name: String
    var isVisible: Bool
    var isKeyWindow = false
    var level: NSWindow.Level = .normal
    var log: [String] = []
    /// Not reachable through NotchYieldWindow; counted to show yielding never makes a window mouse-transparent.
    var ignoresMouseEvents = false { didSet { mouseTransparencyWrites += 1 } }
    var mouseTransparencyWrites = 0

    init(_ name: String, visible: Bool = true) {
        self.name = name
        isVisible = visible
    }

    func orderOut(_ sender: Any?) {
        isVisible = false
        isKeyWindow = false
        log.append("out")
    }

    func orderFrontRegardless() {
        isVisible = true
        log.append("front")
    }

    func order(_ place: NSWindow.OrderingMode, relativeTo otherWindowNumber: Int) {
        isVisible = true
        log.append("\(place == .below ? "below" : "above") \(otherWindowNumber)")
    }

    func makeKey() {
        isKeyWindow = true
        log.append("key")
    }
}
