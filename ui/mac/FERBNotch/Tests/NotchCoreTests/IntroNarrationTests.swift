import XCTest
import CoreGraphics
import SwiftUI
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class IntroNarrationTests: XCTestCase {
    // decision: APP-616
    func testSurfaceFitsTheRenderedLineWithoutAnEmptyFooter() throws {
        let model = NotchModel(preferences: scratchDefaults())
        let lines = [IntroCopy.hello, IntroCopy.signIn, IntroCopy.pasteKey(.anthropic),
                     IntroCopy.suggest(["Find files I could tidy up on my Mac.",
                        "Compare three well-reviewed electric kettles on the web and tell me which is the best value."])]
        for width: CGFloat in [360, 420, 540] {
            model.preferredPanelWidth = width
            for interactive in [false, true] {
                model.onIntroAction = interactive ? { _ in } : nil
                for (contentWidth, inset) in [(width, IntroNarrationView.surfaceInset),
                                               (width - 50, IntroNarrationView.chatInset)] {
                    for line in lines {
                        let renderer = ImageRenderer(content: IntroNarrationView(line: line, cameraSpace: 0,
                            act: model.introActions, inset: inset).frame(width: contentWidth))
                        let rendered = CGFloat(try XCTUnwrap(renderer.cgImage).height)
                        XCTAssertEqual(model.introContentHeight(line, width: contentWidth, inset: inset), rendered, accuracy: 4,
                                       "width=\(contentWidth), interactive=\(interactive), line=\(line.text)")
                    }
                }
            }
        }
    }

    func testPermissionOverviewMakesRoomForEveryGrant() {
        let model = NotchModel(preferences: scratchDefaults())
        let basic = IntroLine(text: "Permissions")
        var overview = basic
        overview.checklist = (0..<7).map {
            IntroChecklistItem(id: "grant-\($0)", title: "Permission \($0)", detail: "Why", complete: false)
        }
        XCTAssertGreaterThanOrEqual(model.introContentHeight(overview) - model.introContentHeight(basic), 7 * 32)
    }

    // decision: APP-501
    func testSettingAnIntroLineSwellsTheNotchToShowItWithoutOpeningTheChat() {
        let model = NotchModel(preferences: scratchDefaults())
        var presented: [Bool] = []
        model.onBackgroundChange = { presented.append($0) }
        let collapsed = model.surfaceSize
        let line = IntroLine(text: "Hi, I'm FERB.", detail: "I work on your Mac while you keep working.")
        model.introLine = line
        XCTAssertTrue(model.surfaceVisible)
        XCTAssertFalse(model.expanded)                       // the chat stays closed; only the line shows
        XCTAssertEqual(model.introDisplayLine, line)
        XCTAssertGreaterThan(model.surfaceSize.width, collapsed.width)
        XCTAssertGreaterThan(model.surfaceSize.height, collapsed.height)
        XCTAssertEqual(presented, [true])
        let oneLine = model.surfaceSize
        model.introLine = IntroLine(text: "Hi, I'm FERB.", detail: nil)
        XCTAssertLessThan(model.surfaceSize.height, oneLine.height)   // the detail line has its own room
    }

    // decision: APP-501
    func testClearingTheIntroLineShrinksTheNotchBack() {
        let model = NotchModel(preferences: scratchDefaults())
        let collapsed = model.surfaceSize
        model.introLine = IntroLine(text: "Let's set things up.", detail: nil)
        var presented: [Bool] = []
        model.onBackgroundChange = { presented.append($0) }
        model.introLine = nil
        XCTAssertFalse(model.surfaceVisible)
        XCTAssertEqual(model.surfaceSize, collapsed)
        XCTAssertEqual(presented, [true])
        XCTAssertNotNil(model.introDisplayLine)              // kept while the surface closes, as the plan preview is
        model.finishCompactPreviewDismissal()
        XCTAssertNil(model.introDisplayLine)
    }

    // decision: APP-618
    func testTearDownDuringAFlightLeavesNoOrbAndSchedulesNoMoreSound() async {
        let surface = FakeOrbSurface()
        let sound = FakeSoundPlayer()
        let clock = ManualClock()
        let tracker = FakeTracker()
        let orb = OrbController(surface: surface, sound: sound, clock: clock, home: { CGPoint(x: 720, y: 885) },
                                reduceMotion: { false }, makeTracker: { tracker })
        let flight = Task { await orb.launch(to: CGPoint(x: 200, y: 400), instruction: IntroLine(text: "Drag me in")) }
        await Task.yield()
        for _ in 0..<5 { clock.advance(1.0 / 60) }
        XCTAssertTrue(surface.visible)
        XCTAssertEqual(sound.played.first, .orbDrip)
        XCTAssertNotEqual(surface.center, CGPoint(x: 200, y: 400))   // still in the air
        orb.dockBesideSettings(dragging: URL(fileURLWithPath: "/Applications/Example.app"))
        XCTAssertTrue(tracker.running)
        orb.tearDown()
        await flight.value                                   // the flight's caller is released, not left waiting
        XCTAssertFalse(surface.visible)
        XCTAssertNil(surface.dragItem)
        XCTAssertEqual(sound.stops, 0)                       // the shared engine is IntroController.end's to stop
        XCTAssertFalse(clock.running)
        XCTAssertFalse(tracker.running)
        let heard = sound.played
        clock.advance(1)                                     // no melt is issued for the flight that was cut
        XCTAssertEqual(sound.played, heard)
        orb.tearDown()                                       // safe twice
        XCTAssertFalse(surface.visible)
    }

    // decision: APP-535
    func testEverySoundHasItsFiles() {
        for sound in FerbSound.allCases {
            for subtle in [false, true] {
                let files = FerbSoundFiles.files(for: sound, subtle: subtle)
                XCTAssertFalse(files.isEmpty, "\(sound) subtle: \(subtle)")
                for file in files { XCTAssertTrue(FileManager.default.fileExists(atPath: file.path), file.path) }
            }
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: FerbSoundFiles.roomImpulse?.path ?? "/missing"))
        // Signature sounds never vary; the everyday ones pick among their variants.
        XCTAssertEqual(FerbSoundFiles.files(for: .intro, subtle: false).count, 1)
        XCTAssertEqual(FerbSoundFiles.files(for: .firstLaunch, subtle: true).count, 1)
        XCTAssertEqual(FerbSoundFiles.files(for: .done, subtle: false).count, 3)
        XCTAssertTrue(FerbSoundFiles.files(for: .done, subtle: true).allSatisfy { $0.lastPathComponent.contains("subtle") })
    }
}

/// The closed notch shows one thing; the live intro line wins while set (fix round 1).
@MainActor
final class IntroPrecedenceTests: XCTestCase {
    // decision: APP-501
    func testThePulseSwellsTheNotchOnceAndShrinksBack() async throws {
        let model = NotchModel(preferences: scratchDefaults())
        let collapsed = model.surfaceSize
        var presented: [Bool] = []
        model.onBackgroundChange = { presented.append($0) }
        model.pulseIntro(seconds: 0.05)
        XCTAssertTrue(model.surfaceVisible)
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(model.compactContent, .pulse)
        XCTAssertGreaterThan(model.surfaceSize.height, collapsed.height)
        try await Task.sleep(for: .milliseconds(200))
        XCTAssertFalse(model.surfaceVisible)
        XCTAssertEqual(model.surfaceSize, collapsed)
        XCTAssertEqual(presented, [true, true])
    }
}
