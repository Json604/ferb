import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Owner 2026-11-18, with a screenshot of his menu bar: FERB's icon was a generic waveform between the headphones and
/// the other system glyphs. The icon is now FERB's own "f." (APP-960): a template image drawn in code, the dot saying
/// the state (APP-961). These read the drawn pixels at 1x and 2x.
@MainActor
final class MenuBarIconTests: XCTestCase {
    private static let states: [MenuBarState] = [.connecting, .settingUp, .notRunning, .needsYou(1), .needsYou(3),
                                                 .stopped, .paused, .working(1), .working(4), .ready]
    private static let marks = Set(states.map(\.mark))

    // decision: APP-960, APP-961
    func testEveryStateIsATemplateImageOfTheMenuBarsSizeDescribedByItsLine() {
        for state in Self.states {
            let image = MenuBarIcon.image(for: state)
            XCTAssertTrue(image.isTemplate, "\(state): macOS tints only a template image for the bar")
            XCTAssertEqual(image.size, NSSize(width: 18, height: 18))
            XCTAssertEqual(image.accessibilityDescription, state.statusLine)
        }
    }

    /// Alpha only (every pixel black, so the template's tint is the only colour) and inside the canvas, in both
    /// drawings (the tile is 15 points; the letter 15 to the hook's top, as tall as the bar's other icons, its dot
    /// below the baseline as the logo's).
    // decision: APP-961, APP-962, APP-963, APP-964
    func testTheMarksAreAlphaOnlyAndSizedForTheBar() {
        for style in MenuBarMark.Style.allCases {
            for mark in Self.marks {
                for scale in [1, 2] as [CGFloat] {
                    let rep = MenuBarIcon.bitmap(mark, scale: scale, style: style)
                    XCTAssertEqual(rep.pixelsWide, Int(18 * scale))
                    XCTAssertEqual(rep.pixelsHigh, Int(18 * scale))
                    let px = Pixels(rep)
                    let what = "\(style) \(mark) at \(scale)x"
                    XCTAssertTrue(px.alphaOnly, "\(what) draws colour")
                    let ink = px.inkBounds(scale: scale)
                    XCTAssertGreaterThanOrEqual(ink.height, 12.5, what)
                    XCTAssertLessThanOrEqual(ink.height, 16.5, what)
                    XCTAssertGreaterThanOrEqual(ink.minX, 0.5, what)
                    XCTAssertGreaterThanOrEqual(ink.minY, 0.5, what)
                    XCTAssertLessThanOrEqual(ink.maxX, 17.5, what)
                    XCTAssertLessThanOrEqual(ink.maxY, 17.5, what)
                }
            }
        }
    }

    /// Each state the menu says differently looks different, in both drawings at 1x and 2x; only connecting and
    /// setting up (both FERB coming up) share the hollow dot.
    // decision: APP-961
    func testEachStateLooksDifferentExceptTheTwoWaysOfComingUp() {
        XCTAssertEqual(MenuBarState.connecting.mark, MenuBarState.settingUp.mark)
        XCTAssertEqual(MenuBarState.working(1).mark, MenuBarState.working(4).mark)
        XCTAssertEqual(Self.marks.count, 7)
        for style in MenuBarMark.Style.allCases {
            for scale in [1, 2] as [CGFloat] {
                let drawn = Set(Self.marks.map { Pixels(MenuBarIcon.bitmap($0, scale: scale, style: style)).bytes })
                XCTAssertEqual(drawn.count, Self.marks.count, "two states draw the same pixels: \(style) \(scale)x")
            }
        }
    }

    /// The f's stem lands on whole pixels: a row through it has no half-covered pixels, and the stem (ink on the bare
    /// letter, a cut through the tile) is the stem's width: the logo's own f.
    // decision: APP-962, APP-965
    func testTheStemIsCrispAtOneAndTwoTimes() {
        for style in MenuBarMark.Style.allCases {
            for scale in [1, 2] as [CGFloat] {
                let row = Pixels(MenuBarIcon.bitmap(MenuBarState.ready.mark, scale: scale, style: style))
                    .row(atPoint: 9, scale: scale)
                XCTAssertTrue(row.allSatisfy { $0 == 0 || $0 == 255 }, "a soft edge: \(style) \(scale)x: \(row)")
                let stem = Int((MenuBarIcon.stemWidth(style, scale: scale) * scale).rounded())
                let runs = Pixels.runs(row)
                switch style {
                case .letter: XCTAssertEqual(runs.filter { $0.value == 255 }.map(\.count), [stem], "\(scale)x: \(row)")
                case .tile: XCTAssertTrue(runs.dropFirst().contains { $0.value == 0 && $0.count == stem }, "\(row)")
                }
            }
        }
    }

    /// Paused, stopped and not running quiet the mark: the letter fades, the tile becomes its outline; the state's
    /// own shape stays at full strength so it still reads.
    // decision: APP-961
    func testPausedStoppedAndNotRunningQuietTheMarkButNotTheDot() {
        for state in Self.states {
            let dimmed = state == .paused || state == .stopped || state == .notRunning
            XCTAssertEqual(state.mark.dimmed, dimmed)
            let letter = Pixels(MenuBarIcon.bitmap(state.mark, scale: 2, style: .letter))
            let stem = letter.row(atPoint: 9, scale: 2).max() ?? 0
            if dimmed {
                XCTAssertEqual(Double(stem) / 255, Double(MenuBarIcon.dimmedAlpha), accuracy: 0.01, "\(state)")
            } else {
                XCTAssertEqual(stem, 255, "\(state)")
            }
            XCTAssertEqual(letter.maxAlpha, 255, "\(state): the dot is drawn at full strength")
            // Inside the tile, left of the f: filled, or empty within the outline.
            let tile = Pixels(MenuBarIcon.bitmap(state.mark, scale: 2, style: .tile))
            XCTAssertEqual(tile.alpha(7, tile.height - 1 - 18), dimmed ? 0 : 255, "\(state)")
            XCTAssertEqual(tile.alpha(3, tile.height - 1 - 18), 255, "\(state): the tile's edge")
        }
    }

    /// Owner 2026-11-18, of the approved letter a little taller than its neighbours: "only if you can withouth
    /// breaking it. if you can then it would be a nice symmetry". The letter's ink sits level with the headphones'
    /// (both centred in the bar) and its stem keeps the approved weight.
    // decision: APP-966
    func testTheLetterSitsLevelWithTheBarsSystemGlyphs() throws {
        let symbol = try XCTUnwrap(NSImage(systemSymbolName: "headphones", accessibilityDescription: nil))
        let rep = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 36, pixelsHigh: 36,
                                                 bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                                 colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        rep.size = MenuBarIcon.size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        symbol.draw(in: NSRect(x: (18 - symbol.size.width) / 2, y: (18 - symbol.size.height) / 2,
                               width: symbol.size.width, height: symbol.size.height))
        NSGraphicsContext.restoreGraphicsState()
        let theirs = Pixels(rep).inkBounds(scale: 2)
        let ours = Pixels(MenuBarIcon.bitmap(MenuBarState.ready.mark, scale: 2, style: .letter)).inkBounds(scale: 2)
        XCTAssertEqual(ours.maxY, theirs.maxY, accuracy: 0.5, "the hook's top against theirs")
        XCTAssertEqual(ours.minY, theirs.minY, accuracy: 0.5, "the dot's bottom against theirs")
        XCTAssertEqual(MenuBarIcon.stemWidth(.letter, scale: 2), 3)
        XCTAssertEqual(MenuBarIcon.stemWidth(.letter, scale: 1), 3)
    }

    /// The image the status item gets draws, at a Retina bar's scale, exactly what the bitmap tests read.
    // decision: APP-962
    func testTheStatusItemsImageDrawsTheSamePixels() throws {
        let state = MenuBarState.needsYou(2)
        let rep = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 36, pixelsHigh: 36,
                                                 bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                                 colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        rep.size = MenuBarIcon.size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        MenuBarIcon.image(for: state).draw(in: NSRect(origin: .zero, size: MenuBarIcon.size))
        NSGraphicsContext.restoreGraphicsState()
        XCTAssertEqual(Pixels(rep).bytes, Pixels(MenuBarIcon.bitmap(state.mark, scale: 2)).bytes)
    }
}

/// Owner 2026-11-18, on the states and their looks: "good, me tion it somewhere ferb knows it too". FERB's guide
/// (AppGuide, sent to the daemon as `about.guide`) explains the menu bar icon from the same map that draws it.
final class MenuBarGuideTests: XCTestCase {
    /// Every state `derive` can give is in `every`, so none can exist without a description in the guide.
    // decision: APP-963
    func testEveryStateTheMenuBarCanShowIsDescribed() {
        func kind(_ state: MenuBarState) -> MenuBarState {
            switch state {
            case .working: return .working(1)
            case .needsYou: return .needsYou(1)
            default: return state
            }
        }
        for link in [DaemonLink.State.connecting, .connected, .unreachable] {
            for paused in [false, true] {
                for stopped in [false, true] {
                    for questions in [0, 2] {
                        for running in [0, 3] {
                            for settingUp in [false, true] {
                                let state = MenuBarState.derive(link: link, paused: paused, stopped: stopped,
                                                                questions: questions, running: running,
                                                                settingUp: settingUp)
                                XCTAssertTrue(MenuBarState.every.contains(kind(state)), "\(state) isn't described")
                            }
                        }
                    }
                }
            }
        }
        XCTAssertEqual(Set(MenuBarState.every.map(\.statusLine)).count, MenuBarState.every.count)
    }

    /// The guide names each look and each meaning, the pill-hidden colours, and fits the daemon's 400 characters a
    /// how-to keeps (about.rs GUIDE_CHARS).
    // decision: APP-963
    func testTheGuideSaysWhatEachLookMeans() {
        let titles = Set(MenuBarGuide.howtos.map(\.title))
        XCTAssertTrue(titles.isSubset(of: Set(AppGuide.howtos.map(\.title))), "AppGuide carries them")
        let text = MenuBarGuide.howtos.map(\.steps).joined(separator: " ")
        for state in MenuBarState.every {
            XCTAssertTrue(text.localizedCaseInsensitiveContains(state.mark.look), "\(state): its look")
            XCTAssertTrue(text.contains(state.meaning), "\(state): its meaning")
        }
        for tint in [StatusTint.amber, .red, .green] { XCTAssertTrue(text.contains(tint.name)) }
        XCTAssertEqual(Set(MenuBarState.every.map(\.mark.look)).count, Set(MenuBarState.every.map(\.mark)).count)
        for howto in MenuBarGuide.howtos {
            XCTAssertLessThanOrEqual(howto.steps.count, 400, howto.title)
            XCTAssertNil(howto.steps.range(of: #"\bAI\b"#, options: .regularExpression))
        }
    }
}

/// A drawn bitmap's RGBA bytes (premultiplied, top row first).
private struct Pixels {
    let width: Int, height: Int, rowBytes: Int
    let bytes: [UInt8]

    init(_ rep: NSBitmapImageRep) {
        width = rep.pixelsWide
        height = rep.pixelsHigh
        rowBytes = rep.bytesPerRow
        bytes = Array(UnsafeBufferPointer(start: rep.bitmapData, count: rep.bytesPerRow * rep.pixelsHigh))
    }

    func alpha(_ x: Int, _ y: Int) -> UInt8 { bytes[y * rowBytes + x * 4 + 3] }

    /// A row's runs of equal alpha, in order.
    static func runs(_ row: [UInt8]) -> [(value: UInt8, count: Int)] {
        var out: [(value: UInt8, count: Int)] = []
        for v in row {
            if let last = out.last, last.value == v { out[out.count - 1].count += 1 } else { out.append((v, 1)) }
        }
        return out
    }

    var alphaOnly: Bool {
        (0..<height).allSatisfy { y in
            (0..<width).allSatisfy { x in (0..<3).allSatisfy { bytes[y * rowBytes + x * 4 + $0] == 0 } }
        }
    }

    var maxAlpha: UInt8 {
        (0..<height).flatMap { y in (0..<width).map { alpha($0, y) } }.max() ?? 0
    }

    /// The alpha of the pixel row through a height in points (from the bottom), across the f's half of the canvas.
    func row(atPoint y: CGFloat, scale: CGFloat) -> [UInt8] {
        let r = height - 1 - Int(y * scale)
        return (0..<Int(9 * scale)).map { alpha($0, r) }
    }

    /// The inked pixels' bounds in points, y up.
    func inkBounds(scale: CGFloat) -> CGRect {
        var minX = width, maxX = -1, minY = height, maxY = -1
        for y in 0..<height {
            for x in 0..<width where alpha(x, y) > 0 {
                minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
            }
        }
        guard maxX >= 0 else { return .zero }
        return CGRect(x: CGFloat(minX) / scale, y: CGFloat(height - 1 - maxY) / scale,
                      width: CGFloat(maxX - minX + 1) / scale, height: CGFloat(maxY - minY + 1) / scale)
    }
}
