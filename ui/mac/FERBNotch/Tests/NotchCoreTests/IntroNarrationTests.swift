import XCTest
import CoreGraphics
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class IntroNarrationTests: XCTestCase {
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

    func testTearDownDuringAFlightLeavesNoOrbAndStopsEverySound() async {
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
        XCTAssertEqual(sound.stops, 1)
        XCTAssertFalse(clock.running)
        XCTAssertFalse(tracker.running)
        orb.tearDown()                                       // safe twice
        XCTAssertFalse(surface.visible)
    }

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
    func testAPlanPreviewArrivingAsTheIntroLineClosesShowsThePlanNotTheOldLine() {
        let model = NotchModel(preferences: scratchDefaults())
        model.introLine = IntroLine(text: "Let's try one together.", detail: "Watch the notch.")
        model.introLine = nil                                    // closing; its dismissal hasn't finished
        model.acknowledgement.show("I'll check the weather.")
        XCTAssertNil(model.introDisplayLine)
        XCTAssertEqual(model.compactContent, .plan("I'll check the weather."))
        XCTAssertEqual(model.surfaceSize.height, model.compactPlanHeight)
    }

    func testAnIntroLineOverAPlanPreviewWinsAndGivesItBack() {
        let model = NotchModel(preferences: scratchDefaults())
        model.acknowledgement.show("I'll check the weather.")
        var presented: [Bool] = []
        model.onBackgroundChange = { presented.append($0) }
        let line = IntroLine(text: "That's FERB planning.", detail: nil)
        model.introLine = line
        XCTAssertEqual(model.compactContent, .intro(line))
        XCTAssertEqual(model.surfaceSize.height, model.introSurfaceHeight(line))
        model.introLine = nil
        XCTAssertEqual(model.compactContent, .plan("I'll check the weather."))
        XCTAssertEqual(model.surfaceSize.height, model.compactPlanHeight)
        XCTAssertEqual(presented, [true, true])
        model.acknowledgement.dismiss()                      // the plan closes: the old line never comes back
        XCTAssertNil(model.introDisplayLine)
        XCTAssertNotEqual(model.compactContent, .intro(line))
    }

    func testWithBothSetTheIntroLineWinsWhicheverCameFirst() {
        let model = NotchModel(preferences: scratchDefaults())
        let line = IntroLine(text: "Here's the plan.", detail: nil)
        model.introLine = line
        model.acknowledgement.show("I'll check the weather.")
        XCTAssertEqual(model.compactContent, .intro(line))
        XCTAssertEqual(model.introDisplayLine, line)
        XCTAssertEqual(model.surfaceSize.height, model.introSurfaceHeight(line))
        model.introLine = nil
        XCTAssertEqual(model.compactContent, .plan("I'll check the weather."))
        model.acknowledgement.dismiss()
        XCTAssertNil(model.introDisplayLine)
        XCTAssertNotEqual(model.compactContent, .intro(line))
    }

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
