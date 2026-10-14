import XCTest
@testable import FERBNotch

@MainActor
final class NotchWindowYieldTests: XCTestCase {
    private var panel: FakeNotchWindow!
    private var tracer: FakeNotchWindow!
    private var rejoined: [String] = []
    private var yield: NotchWindowYield!

    override func setUp() async throws {
        panel = FakeNotchWindow("panel")
        tracer = FakeNotchWindow("tracer", visible: false)
        rejoined = []
        yield = NotchWindowYield(panel: panel, tracer: tracer) { [unowned self] window in
            self.rejoined.append((window as! FakeNotchWindow).name)
        }
    }

    func testYieldOrdersThePanelOutAndBackIntoTheOverlaySpace() {
        panel.isKeyWindow = true
        yield.yield()
        XCTAssertFalse(panel.isVisible)
        XCTAssertFalse(panel.isKeyWindow)
        yield.comeBack(refocus: true)
        XCTAssertTrue(panel.isVisible)
        XCTAssertTrue(panel.isKeyWindow)
        XCTAssertEqual(panel.log, ["out", "front", "key"])
        XCTAssertEqual(rejoined, ["panel"])
        XCTAssertEqual(panel.mouseTransparencyWrites, 0)
    }

    func testComingBackTakesFocusOnlyIfItHadItAndTheOwnerStayed() {
        yield.yield()
        yield.comeBack(refocus: true)
        XCTAssertFalse(panel.isKeyWindow)

        panel.isKeyWindow = true
        yield.yield()
        yield.ownerMovedElsewhere()
        yield.comeBack(refocus: true)
        XCTAssertFalse(panel.isKeyWindow)

        panel.isKeyWindow = true
        yield.yield()
        yield.comeBack(refocus: false)
        XCTAssertFalse(panel.isKeyWindow)
    }

    func testTheOwnerBringingFERBBackFocusesTheVisibleInputButNeverWhileYielded() {
        yield.yield()
        XCTAssertFalse(yield.focusInput(bodyVisible: true))
        XCTAssertFalse(panel.isKeyWindow)
        yield.comeBack(refocus: false)
        XCTAssertFalse(yield.focusInput(bodyVisible: false))
        XCTAssertFalse(panel.isKeyWindow)
        XCTAssertTrue(yield.focusInput(bodyVisible: true))
        XCTAssertTrue(panel.isKeyWindow)
    }

    func testPresentationsDoNotBringAYieldedPanelBack() {
        yield.yield()
        yield.showPanel()
        XCTAssertFalse(panel.isVisible)
        yield.comeBack(refocus: false)
        yield.showPanel()
        XCTAssertTrue(panel.isVisible)
        XCTAssertEqual(rejoined, ["panel"])
    }

    func testOnlyTheAttentionLightStaysOnScreenWhileYielded() {
        yield.yield()
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
    /// transparent areas. The only write allowed is the light window's permanent click-through at creation.
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
        XCTAssertEqual(writes, ["tracerPanel.ignoresMouseEvents = true"])
    }
}

@MainActor
private final class FakeNotchWindow: NotchYieldWindow {
    let name: String
    var isVisible: Bool
    var isKeyWindow = false
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

    func makeKey() {
        isKeyWindow = true
        log.append("key")
    }
}
