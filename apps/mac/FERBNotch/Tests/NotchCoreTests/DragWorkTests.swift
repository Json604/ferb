import AppKit
import Combine
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Owner 2026-11-19: "i just noticed that mac app hold and drag jitters sooo much, maybe its resrawign it too" ... "its
/// the drag when i hold the notch mode and drag it on the screen. windwos is ok ig." Every hold-and-drag the app has,
/// driven through its own views with a stand-in pointer (never the owner's), on a long day's chat. Counted per pointer
/// event: announcements of the model every chat view observes, geometry handed to SwiftUI, evaluations of the
/// transcript and the composer (ChatWork), messages typeset again, outline images rasterized, window frames set.
///
/// Before APP-971 a held drag of the open notch cost 23-31 ms of the main thread per event (worst 78-88 ms) on this
/// test's 60-entry day: the frost's outline was drawn into a display-sized mask image, which AppKit made the window's
/// shape, rasterizing it twice per event (Instruments' sample: 85% of the main thread in NSImage and
/// SLSTransactionSetWindowCornerMask), and SwiftUI was handed the new origin on every event.
@MainActor
final class DragWorkTests: XCTestCase {
    struct Work: CustomStringConvertible {
        var events = 0
        var model = 0
        var presentation = 0
        var transcript = 0
        var composer = 0
        var textLayouts = 0
        var maskImages = 0
        var frames = 0
        var defaults = 0
        /// Message views in the counted windows at the start and the end: a row that came into view meanwhile shows
        /// here. ChatWork counts every view in the process, and rows the list measures ahead are built outside any
        /// window, so a typeset with no change here was measuring ahead or another test's work, not this window's.
        var textViews = (0, 0)
        var ms: [Double] = []
        var description: String {
            let mean = ms.isEmpty ? 0 : ms.reduce(0, +) / Double(ms.count)
            let worst = ms.max() ?? 0
            return String(format: "events=%d model=%d swiftUIGeometry=%d transcript=%d composer=%d textLayouts=%d (message views in this window %d->%d) maskImages=%d windowFrames=%d settingsWrites=%d mean=%.2fms worst=%.2fms",
                          events, model, presentation, transcript, composer, textLayouts, textViews.0, textViews.1,
                          maskImages, frames, defaults, mean, worst)
        }
    }

    /// Counts from now until `finish`.
    @MainActor final class Counter {
        var work = Work()
        private var bag: [AnyCancellable] = []
        private var observers: [NSObjectProtocol] = []
        private let start = (ChatWork.transcript, ChatWork.composer, ChatWork.textLayouts)
        private let presentation: NotchPresentation?
        private let startPublishes: Int
        private let startMasks: Int
        private let windows: [NSWindow]

        static func messageViews(in windows: [NSWindow]) -> Int {
            func walk(_ view: NSView) -> Int { (view is MessageTextView ? 1 : 0) + view.subviews.map(walk).reduce(0, +) }
            return windows.compactMap(\.contentView).map(walk).reduce(0, +)
        }

        init(_ model: NotchModel, presentation: NotchPresentation?, windows: [NSWindow], defaults: UserDefaults) {
            self.presentation = presentation
            self.windows = windows
            work.textViews.0 = Self.messageViews(in: windows)
            startPublishes = presentation?.publishes ?? 0
            startMasks = presentation?.backdrop.maskImageBuilds ?? 0
            model.objectWillChange.sink { [unowned self] _ in self.work.model += 1 }.store(in: &bag)
            for window in windows {
                for name in [NSWindow.didMoveNotification, NSWindow.didResizeNotification] {
                    observers.append(NotificationCenter.default.addObserver(forName: name, object: window, queue: nil) {
                        [unowned self] _ in MainActor.assumeIsolated { self.work.frames += 1 } })
                }
            }
            observers.append(NotificationCenter.default.addObserver(forName: UserDefaults.didChangeNotification,
                                                                    object: defaults, queue: nil) {
                [unowned self] _ in MainActor.assumeIsolated { self.work.defaults += 1 } })
        }

        func finish() -> Work {
            observers.forEach(NotificationCenter.default.removeObserver)
            observers = []
            bag = []
            var out = work
            out.transcript = ChatWork.transcript - start.0
            out.composer = ChatWork.composer - start.1
            out.textLayouts = ChatWork.textLayouts - start.2
            out.presentation = (presentation?.publishes ?? 0) - startPublishes
            out.maskImages = (presentation?.backdrop.maskImageBuilds ?? 0) - startMasks
            out.textViews.1 = Self.messageViews(in: windows)
            return out
        }
    }

    // MARK: - Helpers

    /// One display frame after an event: what AppKit does before it draws, and the commit.
    func frame(_ window: NSWindow) {
        window.contentView?.layoutSubtreeIfNeeded()
        window.displayIfNeeded()
        CATransaction.flush()
    }

    func settle(_ window: NSWindow, _ seconds: TimeInterval = 0.4) {
        let end = Date().addingTimeInterval(seconds)
        while Date() < end {
            window.contentView?.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        }
    }

    /// Runs the main run loop until no message has been typeset for 0.3 s: the transcript list's measuring ahead (a
    /// few rows per pass, 6 ms each, slower under load) and any work queued before has finished, so a counted window
    /// counts only what its own events cause. Fails if that never happens within 5 s.
    func quiet(_ window: NSWindow, file: StaticString = #filePath, line: UInt = #line) {
        let first = ChatWork.textLayouts
        var last = first, still = Date()
        let end = Date().addingTimeInterval(5)
        while Date() < end {
            window.contentView?.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
            if ChatWork.textLayouts != last {
                last = ChatWork.textLayouts
                still = Date()
            } else if Date().timeIntervalSince(still) >= 0.3 {
                return
            }
        }
        XCTFail("the chat never settled: \(ChatWork.textLayouts - first) typesets in 5 s", file: file, line: line)
    }

    /// The pointer event and a frame, timed (the display link and queued work run after, untimed).
    func step(_ window: NSWindow, _ counter: Counter, _ send: () throws -> Void) rethrows {
        let t = Date()
        try send()
        frame(window)
        counter.work.ms.append(Date().timeIntervalSince(t) * 1000)
        counter.work.events += 1
        RunLoop.main.run(until: Date().addingTimeInterval(0.004))
    }

    func event(_ type: NSEvent.EventType, _ window: NSWindow?, at location: NSPoint = .zero) throws -> NSEvent {
        try XCTUnwrap(NSEvent.mouseEvent(with: type, location: location, modifierFlags: [], timestamp: 0,
                                         windowNumber: window?.windowNumber ?? 0, context: nil, eventNumber: 1,
                                         clickCount: 1, pressure: 1))
    }

    func views<T: NSView>(_ type: T.Type, in root: NSView) -> [T] {
        ((root as? T).map { [$0] } ?? []) + root.subviews.flatMap { views(type, in: $0) }
    }

    /// FERB with a 60-entry day in today's chat; the notch torn out to `edge`, or the chat in its window.
    func coordinator(window: Bool, edge: String = "top") throws -> (PanelCoordinator, UserDefaults) {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults("DragWork")
        NotchDocking.save(false, in: defaults)
        defaults.set(edge, forKey: "notch.indicatorEdge")
        if window { defaults.set(ChatHost.window.rawValue, forKey: ChatHost.defaultsKey) }
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        model.conversation = try TranscriptScrollProbeTests.archive(day: Date(), entries: 60)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        coordinator.hoverWait = { _ in }
        addTeardownBlock { coordinator.tearDown() }
        coordinator.updateScreen()
        return (coordinator, defaults)
    }

    /// The open notch held by its pill and pulled out into the upper middle of the screen, as the owner holds it.
    struct HeldNotch {
        let coordinator: PanelCoordinator
        let defaults: UserDefaults
        let control: FerbIndicatorControl
        let window: NSWindow
        let screen: CGRect
        let pointer: () -> CGPoint
        let move: (CGPoint) throws -> Void
    }

    func holdNotch() throws -> HeldNotch {
        let (coordinator, defaults) = try coordinator(window: false)
        coordinator.model.expand(focus: false)
        let control = try XCTUnwrap(coordinator.indicatorControl)
        let window = try XCTUnwrap(control.window)
        settle(window)
        let screen = try XCTUnwrap(window.screen ?? NSScreen.main).frame
        let start = window.convertToScreen(control.convert(control.bounds, to: nil))
        var pointer = CGPoint(x: start.midX, y: start.midY)
        control.pointerLocation = { pointer }
        control.mouseDown(with: try event(.leftMouseDown, window))
        let move: (CGPoint) throws -> Void = { [unowned self] to in
            pointer = to
            control.mouseDragged(with: try self.event(.leftMouseDragged, control.window))
        }
        let target = CGPoint(x: screen.midX, y: screen.midY + 200)
        for i in 1...20 {
            let f = CGFloat(i) / 20
            try move(CGPoint(x: start.midX + (target.x - start.midX) * f, y: start.midY + (target.y - start.midY) * f))
            frame(window)
        }
        settle(window)
        return HeldNotch(coordinator: coordinator, defaults: defaults, control: control, window: window, screen: screen,
                         pointer: { pointer }, move: move)
    }

    /// Where the composer's pixels are, against the frost's outline (the resize border follows the frost), in the
    /// backdrop's coordinates: through the layers, so a layer's own translation counts.
    func composerAgainstFrost(_ coordinator: PanelCoordinator) throws -> CGPoint {
        let backdrop = coordinator.presentation.backdrop
        let composer = try XCTUnwrap(views(ChatTextView.self, in: backdrop).first, "the composer")
        let layer = try XCTUnwrap(composer.layer), root = try XCTUnwrap(backdrop.layer)
        let at = layer.convert(CGPoint(x: layer.bounds.midX, y: layer.bounds.midY), to: root)
        let frost = try XCTUnwrap(views(PanelResizeOverlay.self, in: backdrop).first).frame
        return CGPoint(x: at.x - frost.minX, y: at.y - frost.minY)
    }

    // MARK: - The notch held and dragged (APP-971)

    // decision: APP-970, APP-971, APP-597
    func testHoldingTheOpenNotchAndDraggingItEvaluatesAndRasterizesNothing() throws {
        let held = try holdNotch()
        let coordinator = held.coordinator, backdrop = coordinator.presentation.backdrop
        // while held the outline is a vector mask on the material's layer; the window has no shape image
        XCTAssertNil(backdrop.maskImage, "no display-sized mask image while held")
        let material = try XCTUnwrap(ProgressiveBlur.backdropLayer(in: backdrop.layer)?.superlayer, "the material")
        XCTAssertEqual(material.mask?.sublayers?.count, 3, "card, pill and shoulder outlines")
        let grip = try composerAgainstFrost(coordinator)
        quiet(held.window)
        let counter = Counter(coordinator.model, presentation: coordinator.presentation, windows: [held.window],
                              defaults: held.defaults)
        var surface = coordinator.presentation.surfaceOrigin
        var moved = 0
        for n in 0..<120 {
            var to = held.pointer()
            to.x += n < 60 ? 2.5 : -2.5
            to.y += 0.75
            try step(held.window, counter) { try held.move(to) }
            if coordinator.presentation.surfaceOrigin != surface { moved += 1 }
            surface = coordinator.presentation.surfaceOrigin
            let now = try composerAgainstFrost(coordinator)
            XCTAssertEqual(now.x, grip.x, accuracy: 0.01, "event \(n): the chat moves with its frost")
            XCTAssertEqual(now.y, grip.y, accuracy: 0.01, "event \(n): the chat moves with its frost")
        }
        let work = counter.finish()
        print("DRAG notch held: \(work)")
        XCTAssertGreaterThan(moved, 100, "the surface followed the pointer")
        XCTAssertEqual(work.model, 0, "nothing the chat's views observe changed")
        XCTAssertEqual(work.presentation, 0, "SwiftUI was handed no new geometry: the layers moved")
        XCTAssertEqual(work.transcript, 0, "the transcript was not evaluated")
        XCTAssertEqual(work.composer, 0, "the composer was not evaluated")
        XCTAssertEqual(work.textLayouts, 0, "no message was typeset: \(work)")
        XCTAssertEqual(work.maskImages, 0, "no outline was rasterized")
        XCTAssertEqual(work.frames, 0, "the window stays still; the surface moves inside it")
        XCTAssertNotNil(material.mask, "the vector outline stayed on the material through the drag")

        // the drop: one outline image at the window's own size, SwiftUI draws the new place, nothing jumps
        let drop = Counter(coordinator.model, presentation: coordinator.presentation, windows: [held.window],
                           defaults: held.defaults)
        held.control.mouseUp(with: try event(.leftMouseUp, held.control.window))
        CATransaction.flush()                     // what the release's own event commits (AppKit lays out first)
        let atRelease = try composerAgainstFrost(coordinator)
        frame(held.window)
        let dropped = drop.finish()
        XCTAssertEqual(atRelease.x, grip.x, accuracy: 0.5, "no jump in the release's own frame")
        XCTAssertEqual(atRelease.y, grip.y, accuracy: 0.5, "no jump in the release's own frame")
        XCTAssertEqual(coordinator.presentation.contentShift, .zero, "SwiftUI draws the place itself again")
        XCTAssertNotNil(backdrop.maskImage, "the outline image is back once the drag ends")
        let materialNow = ProgressiveBlur.backdropLayer(in: backdrop.layer)?.superlayer
        XCTAssertNotEqual(materialNow?.mask?.sublayers?.count, 3, "the vector outline is gone: AppKit's image mask again")
        XCTAssertLessThanOrEqual(dropped.maskImages, 2, "the drop draws the outline once at the small window")
        XCTAssertLessThan(backdrop.bounds.width, held.screen.width, "the window is no longer the display's size")
        let after = try composerAgainstFrost(coordinator)
        XCTAssertEqual(after.x, grip.x, accuracy: 0.5, "no jump at the drop")
        XCTAssertEqual(after.y, grip.y, accuracy: 0.5, "no jump at the drop")
    }

    /// A drag that ends on the camera: the join to the notch moves the outline every frame for about 0.4 s; it stays a
    /// vector mask until the notch is whole, then becomes one image.
    // decision: APP-971
    func testDockingAfterADragRasterizesTheOutlineOnce() throws {
        let held = try holdNotch()
        let coordinator = held.coordinator, backdrop = coordinator.presentation.backdrop
        guard let screen = held.window.screen, screen.safeAreaInsets.top > 0,
              let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea else {
            held.control.mouseUp(with: try event(.leftMouseUp, held.control.window))
            throw XCTSkip("Needs a display with a camera housing")
        }
        let camera = CGPoint(x: (left.maxX + right.minX) / 2, y: screen.frame.maxY - 6)
        quiet(held.window)
        let counter = Counter(coordinator.model, presentation: coordinator.presentation, windows: [held.window],
                              defaults: held.defaults)
        let from = held.pointer()
        for i in 1...30 {
            let f = CGFloat(i) / 30
            try step(held.window, counter) {
                try held.move(CGPoint(x: from.x + (camera.x - from.x) * f, y: from.y + (camera.y - from.y) * f))
            }
        }
        held.control.mouseUp(with: try event(.leftMouseUp, held.control.window))
        settle(held.window, 1.2)
        let work = counter.finish()
        print("DRAG notch docked: \(work)")
        XCTAssertTrue(coordinator.model.isNotchDocked, "it docked")
        XCTAssertNotNil(backdrop.maskImage, "docked, the outline is an image again")
        XCTAssertLessThanOrEqual(work.maskImages, 4, "the join rasterized no outline per frame")
        XCTAssertEqual(work.textLayouts, 0, "\(work)")
    }

    /// The open notch resized by its edge: every event resizes the window and the outline with it. The outline is the
    /// vector mask while held, its image drawn once at the release; the chat lays out at each new size (that is the
    /// resize), and no message is typeset again for a height change.
    // decision: APP-971, APP-972, APP-587
    func testResizingTheNotchByItsEdgeRasterizesTheOutlineOnlyAtTheRelease() throws {
        let (coordinator, defaults) = try coordinator(window: false)
        coordinator.model.expand(focus: false)
        let window = try XCTUnwrap(coordinator.indicatorControl?.window)
        settle(window)
        let backdrop = coordinator.presentation.backdrop
        let overlay = try XCTUnwrap(views(PanelResizeOverlay.self, in: backdrop).first)
        var pointer = NSPoint(x: 800, y: 500)
        overlay.pointer = { pointer }
        let press = try event(.leftMouseDown, window,
                              at: overlay.convert(NSPoint(x: overlay.bounds.midX, y: overlay.bounds.maxY - 4), to: nil))
        let height = coordinator.model.chatTranscriptHeight
        overlay.mouseDown(with: press)
        // taller by 40 points and back, then the same again: the rows the first pass brought into view are built then;
        // the second pass is the resize alone
        func pass(_ counter: Counter?) throws {
            for n in 0..<40 {
                pointer.y += n < 20 ? -2 : 2
                if let counter { try step(window, counter) { overlay.mouseDragged(with: press) } } else {
                    overlay.mouseDragged(with: press)
                    frame(window)
                }
                XCTAssertNil(backdrop.maskImage, "event \(n): the vector outline while held")
            }
        }
        try pass(nil)
        quiet(window)
        let counter = Counter(coordinator.model, presentation: coordinator.presentation, windows: [window],
                              defaults: defaults)
        try pass(counter)
        let work = counter.finish()
        print("DRAG notch resized (height): \(work)")
        XCTAssertEqual(coordinator.model.chatTranscriptHeight, height, accuracy: 0.5, "it resized and came back")
        XCTAssertEqual(work.transcript, work.events + 0, accuracy: 2, "the chat lays out once per new size")
        XCTAssertEqual(work.maskImages, 0, "no outline image per event")
        XCTAssertEqual(work.textLayouts, 0, "a taller or shorter chat typesets no message again: \(work)")
        XCTAssertEqual(ProgressiveBlur.backdropLayer(in: backdrop.layer)?.superlayer?.mask?.sublayers?.count, 3,
                       "the outline stayed on the material through every new size")
        let end = Counter(coordinator.model, presentation: coordinator.presentation, windows: [window],
                          defaults: defaults)
        overlay.mouseUp(with: press)
        frame(window)
        XCTAssertNotNil(backdrop.maskImage, "the outline image is back at the release")
        XCTAssertLessThanOrEqual(end.finish().maskImages, 2)
    }

    // MARK: - The window moved by its header (APP-973)

    // decision: APP-973, APP-759
    func testMovingTheWindowByItsHeaderEvaluatesNothingAndSavesItsPlaceOnce() throws {
        let (coordinator, defaults) = try coordinator(window: true)
        coordinator.toggleByShortcut()
        let chat = try XCTUnwrap(coordinator.chatWindow)
        let window = chat.panel
        settle(window)
        let drag = try XCTUnwrap(views(WindowDragArea.DragView.self, in: try XCTUnwrap(window.contentView)).first)
        var pointer = NSPoint(x: 500, y: 500)
        drag.pointer = { pointer }
        let press = try event(.leftMouseDown, window)
        drag.mouseDown(with: press)
        quiet(window)
        let counter = Counter(coordinator.model, presentation: nil, windows: [window], defaults: defaults)
        for n in 0..<120 {
            pointer.x += n < 60 ? 2.5 : -2.5
            pointer.y += 0.75
            try step(window, counter) { drag.mouseDragged(with: press) }
        }
        let work = counter.finish()
        print("DRAG window move: \(work)")
        XCTAssertEqual(work.frames, 120, "the window followed every event")
        XCTAssertEqual(work.model, 0)
        XCTAssertEqual(work.transcript, 0)
        XCTAssertEqual(work.composer, 0)
        XCTAssertEqual(work.textLayouts, 0, "\(work)")
        XCTAssertEqual(work.defaults, 0, "no settings write while held")
        let end = Counter(coordinator.model, presentation: nil, windows: [window], defaults: defaults)
        drag.mouseUp(with: press)
        let ended = end.finish()
        XCTAssertEqual(ended.defaults, 1, "its place is saved once, at the release")
        let display = try XCTUnwrap(window.screen.flatMap {
            ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value })
        XCTAssertEqual(ChatWindowFrames.saved(display: display, in: defaults), window.frame)
    }

    // MARK: - A message given again (APP-972)

    /// A list item's marker and words come with two paragraph styles; the text storage makes them one. The text a row
    /// gives again on each of its evaluations was then never "the same", so the view took it again and typeset all of
    /// it: in a resize of the notch, two long answers each event.
    // decision: APP-972
    func testAnAnswerGivenAgainIsNotTypesetAgain() throws {
        _ = NSApplication.shared
        let answer = (0..<3).map { _ in LongChatScaleTests.answer(2) }.joined(separator: "\n\n")
        let host = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 400, height: 600), styleMask: .borderless,
                            backing: .buffered, defer: true)                 // never ordered front
        let view = MessageTextView()
        view.frame = CGRect(x: 0, y: 0, width: 340, height: 400)
        host.contentView?.addSubview(view)
        view.stage(MarkdownTextAttributes.make(answer, size: 12), generation: 0)
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))               // the staged text goes in
        _ = view.measuredSize(width: 340)
        view.layoutSubtreeIfNeeded()
        let before = ChatWork.textLayouts
        for _ in 0..<10 {
            view.stage(MarkdownTextAttributes.make(answer, size: 12), generation: 0)   // a row evaluated again
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
            _ = view.measuredSize(width: 340)
            view.layoutSubtreeIfNeeded()
        }
        XCTAssertEqual(ChatWork.textLayouts - before, 0, "the same answer is measured once")
        XCTAssertNil(view.pendingText)
        // a changed answer still goes in and is measured
        view.stage(MarkdownTextAttributes.make(answer + "\n\nOne more line.", size: 12), generation: 0)
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        _ = view.measuredSize(width: 340)
        XCTAssertTrue(view.string.hasSuffix("One more line."))
        XCTAssertGreaterThan(ChatWork.textLayouts - before, 0)
    }

    // MARK: - The numbers (FERB_DRAG_PROBE=1; wall-clock, so never part of the suite)

    /// Prints what each hold-and-drag costs per pointer event: the notch held in the open, held where the room below the
    /// pill is shorter than the chat (it shrinks with the pointer), resized by its edge, the window moved and resized.
    func testProbeEveryDrag() throws {
        guard ProcessInfo.processInfo.environment["FERB_DRAG_PROBE"] == "1" else {
            throw XCTSkip("probe: FERB_DRAG_PROBE=1")
        }
        try probeHeldNotch()
        try probeNotchResize()
        try probeWindow()
    }

    func probeHeldNotch() throws {
        let held = try holdNotch()
        let coordinator = held.coordinator
        quiet(held.window)
        let open = Counter(coordinator.model, presentation: coordinator.presentation, windows: [held.window],
                           defaults: held.defaults)
        for n in 0..<120 {
            var to = held.pointer()
            to.x += n < 60 ? 2.5 : -2.5
            to.y += 0.75
            try step(held.window, open) { try held.move(to) }
        }
        print("PROBE notch held: \(open.finish())")
        try held.move(CGPoint(x: held.screen.midX, y: held.screen.midY + 40))
        settle(held.window)
        quiet(held.window)
        let tight = Counter(coordinator.model, presentation: coordinator.presentation, windows: [held.window],
                            defaults: held.defaults)
        for n in 0..<120 {
            var to = held.pointer()
            to.y += n < 60 ? -1 : 1
            try step(held.window, tight) { try held.move(to) }
        }
        print("PROBE notch held where the chat shrinks with the pointer: \(tight.finish())")
        held.control.mouseUp(with: try event(.leftMouseUp, held.control.window))
    }

    func probeNotchResize() throws {
        let (coordinator, defaults) = try coordinator(window: false)
        coordinator.model.expand(focus: false)
        let window = try XCTUnwrap(coordinator.indicatorControl?.window)
        settle(window)
        let overlay = try XCTUnwrap(views(PanelResizeOverlay.self, in: try XCTUnwrap(window.contentView)).first)
        var pointer = NSPoint(x: 800, y: 500)
        overlay.pointer = { pointer }
        for (name, local, delta) in [("width", NSPoint(x: overlay.bounds.maxX - 4, y: overlay.bounds.midY), CGPoint(x: 1, y: 0)),
                                     ("height", NSPoint(x: overlay.bounds.midX, y: overlay.bounds.maxY - 4), CGPoint(x: 0, y: -1))] {
            let press = try event(.leftMouseDown, window, at: overlay.convert(local, to: nil))
            overlay.mouseDown(with: press)
            quiet(window)
            let counter = Counter(coordinator.model, presentation: coordinator.presentation, windows: [window],
                                  defaults: defaults)
            for n in 0..<60 {
                let sign: CGFloat = n < 30 ? 1 : -1
                pointer.x += delta.x * sign
                pointer.y += delta.y * sign
                try step(window, counter) { overlay.mouseDragged(with: press) }
            }
            print("PROBE notch resized (\(name)): \(counter.finish())")
            overlay.mouseUp(with: press)
            settle(window, 0.2)
        }
    }

    func probeWindow() throws {
        let (coordinator, defaults) = try coordinator(window: true)
        coordinator.toggleByShortcut()
        let window = try XCTUnwrap(coordinator.chatWindow).panel
        settle(window)
        let drag = try XCTUnwrap(views(WindowDragArea.DragView.self, in: try XCTUnwrap(window.contentView)).first)
        var pointer = NSPoint(x: 500, y: 500)
        drag.pointer = { pointer }
        let press = try event(.leftMouseDown, window)
        drag.mouseDown(with: press)
        quiet(window)
        let moved = Counter(coordinator.model, presentation: nil, windows: [window], defaults: defaults)
        for n in 0..<120 {
            pointer.x += n < 60 ? 2.5 : -2.5
            try step(window, moved) { drag.mouseDragged(with: press) }
        }
        print("PROBE window moved: \(moved.finish())")
        drag.mouseUp(with: press)
        for axis in ["width", "height"] {
            quiet(window)
            let counter = Counter(coordinator.model, presentation: nil, windows: [window], defaults: defaults)
            var frame = window.frame
            for n in 0..<60 {
                let sign: CGFloat = n < 30 ? 1 : -1
                if axis == "width" { frame.size.width += sign } else { frame.size.height += sign; frame.origin.y -= sign }
                try step(window, counter) { window.setFrame(frame, display: true) }
            }
            print("PROBE window resized (\(axis)): \(counter.finish())")
        }
    }
}
