import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Owner, 2026-11-05: "When doing drag n drop file/folder, FERB chat should open up", and the files stay in the chat
/// "until i send them manully or attach a message in text and send". D17: a drag over the notch or pill opens the chat
/// even with hover off and a drop-opened chat stays open like a press; D18: files and folders only, refused during the
/// tour. Driven through the real FileDropSession, NotchModel and PanelCoordinator with real pasteboards.
@MainActor
final class FileDropTests: XCTestCase {
    private var pointer = CGPoint.zero

    private func scratchFiles(_ names: [String], folder: String? = nil) throws -> [URL] {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-drop-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        var urls = try names.map { name -> URL in
            let url = root.appendingPathComponent(name)
            try Data("x".utf8).write(to: url)
            return url
        }
        if let folder {
            let url = root.appendingPathComponent(folder, isDirectory: true)
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            urls.append(url)
        }
        return urls
    }

    private func pasteboard(_ write: (NSPasteboard) -> Void) -> NSPasteboard {
        let board = NSPasteboard(name: NSPasteboard.Name("ferb-drop-test-\(UUID().uuidString)"))
        board.clearContents()
        write(board)
        addTeardownBlock { board.releaseGlobally() }
        return board
    }

    private func files(_ urls: [URL]) -> NSPasteboard { pasteboard { $0.writeObjects(urls.map { $0 as NSURL }) } }

    // decision: APP-737, APP-738
    func testOnlyFilesAndFoldersAreADrop() {
        XCTAssertTrue(FileDrop.accepts([["public.file-url"], ["public.file-url", "public.utf8-plain-text"]]))
        XCTAssertFalse(FileDrop.accepts([]))
        XCTAssertFalse(FileDrop.accepts([["public.utf8-plain-text"]]))                  // selected text
        XCTAssertFalse(FileDrop.accepts([["public.url", "public.utf8-plain-text"]]))    // a web link
        XCTAssertFalse(FileDrop.accepts([["public.file-url"], ["public.utf8-plain-text"]]))   // mixed: refused whole
        XCTAssertFalse(FileDrop.accepts([["com.apple.NSFilePromiseItemMetaData", "com.apple.pasteboard.promised-file-url"]]))
        XCTAssertFalse(FileDrop.accepts([["public.file-url", "com.apple.pasteboard.promised-file-url"]]))
        XCTAssertFalse(FileDrop.accepts([["public.file-url", "x.promise"]], promises: ["x.promise"]))
    }

    // decision: APP-737, APP-739, APP-577
    func testADropAttachesEveryFileAtOnceOpensTheChatAndNeverSends() throws {
        let rpc = ScriptedRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        var presentations = 0
        model.onPresentationChange = { _ in presentations += 1 }
        let urls = try scratchFiles(["a.txt", "b.pdf"], folder: "Folder")
        XCTAssertFalse(model.expanded)
        XCTAssertTrue(model.drops.drop(files(urls + [urls[0]])))          // a repeat is kept once
        XCTAssertEqual(presentations, 1, "one presentation change for the whole drop")
        XCTAssertTrue(model.bodyVisible)
        XCTAssertTrue(model.staysOpenOnHoverOut, "a drop-opened chat stays open like a press")
        XCTAssertEqual(model.session.attachments, urls.map(\.standardizedFileURL))
        XCTAssertTrue(model.session.attachments.last!.hasDirectoryPath, "folders are kept")
        XCTAssertTrue(model.drops.drop(files([urls[1]])))
        XCTAssertEqual(model.session.attachments.count, 3, "already attached files aren't added twice")
        XCTAssertFalse(rpc.calls.contains { $0.0 == "chat.send" }, "a drop never sends")
        XCTAssertEqual(model.session.phase, .draft)
        XCTAssertTrue(model.canSend, "files alone can be sent")
    }

    // decision: APP-737, APP-738
    func testTextLinksPromisesTheTourAndDictationAreRefused() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults(), microphone: HeldMicrophone())
        let urls = try scratchFiles(["a.txt"])
        let text = pasteboard { $0.setString("hello", forType: .string) }
        XCTAssertEqual(model.drops.entered(.handle, text), [])
        XCTAssertFalse(model.drops.drop(text))
        let promise = pasteboard { board in
            let item = NSPasteboardItem()
            item.setString("x", forType: NSPasteboard.PasteboardType("com.apple.pasteboard.promised-file-url"))
            board.writeObjects([item])
        }
        XCTAssertEqual(model.drops.entered(.handle, promise), [])
        XCTAssertEqual(model.drops.entered(.handle, files(urls)), .copy)
        model.drops.exited(.handle)

        model.onIntroAction = { _ in }                                    // the tour runs
        XCTAssertEqual(model.drops.entered(.handle, files(urls)), [])
        XCTAssertFalse(model.drops.drop(files(urls)))
        model.onIntroAction = nil

        model.startDictation()
        XCTAssertEqual(model.session.phase, .listening)
        XCTAssertFalse(model.drops.drop(files(urls)))
        XCTAssertTrue(model.session.attachments.isEmpty)
    }

    // decision: APP-737, APP-739, APP-577
    func testFilesWaitInTheComposerWhileATaskRunsAndSendWithoutWords() throws {
        let rpc = ScriptedRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.expand()
        model.session.draft = "first task"
        model.startWork()
        XCTAssertEqual(model.session.phase, .working)
        let urls = try scratchFiles(["a.txt"])
        XCTAssertTrue(model.drops.drop(files(urls)))
        XCTAssertEqual(model.session.attachments, urls.map(\.standardizedFileURL))
        XCTAssertTrue(model.bodyVisible)
        let turns = model.conversation.turns.count
        model.startWork()                                                 // sent by the owner, no words
        XCTAssertEqual(model.conversation.turns.count, turns + 1)
        XCTAssertEqual(model.conversation.turns.last?.sources, urls.map(\.standardizedFileURL))
        XCTAssertTrue(model.session.attachments.isEmpty)
    }

    // decision: APP-739
    func testTheSpringOpenedChatWaitsForThePointerToLeaveAndFoldsWhenTheDragEnds() async throws {
        let session = FileDropSession()
        var open = false, folds = 0, waits = 0
        session.accepts = { true }
        session.isOpen = { open }
        session.onSpring = { open = true }
        session.onFold = { open = false; folds += 1 }
        session.wait = { _ in waits += 1 }
        session.pointerOver = { waits < 4 }          // still over FERB for the first few looks (the chat opening)
        let urls = try scratchFiles(["a.txt"])
        _ = session.entered(.handle, files(urls))
        await session.springTask?.value
        XCTAssertTrue(open)
        session.exited(.handle)                       // the handle moved into the opening chat
        await session.leaveTask?.value
        XCTAssertEqual(folds, 1)
        XCTAssertGreaterThanOrEqual(waits, 4, "it folded while the pointer was still over FERB")

        // The button comes up over FERB with nothing dropped: the drag is over, the chat it opened folds.
        session.pointerOver = { true }
        _ = session.entered(.edgeRow, files(urls))
        await session.springTask?.value
        XCTAssertTrue(open)
        session.dragEnded()
        XCTAssertFalse(open)
        XCTAssertEqual(folds, 2)
    }

    // MARK: Through the coordinator

    private struct Harness {
        let model: NotchModel
        let coordinator: PanelCoordinator
    }

    private func harness(hover: Bool) throws -> Harness {
        _ = NSApplication.shared
        guard let screen = NSScreen.screens.first else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        model.setHoverEnabled(hover)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        coordinator.updateScreen()
        coordinator.hoverWait = { _ in }
        pointer = CGPoint(x: screen.frame.midX, y: screen.frame.minY + 20)
        coordinator.pointerLocation = { [unowned self] in self.pointer }
        model.drops.wait = { _ in }
        return Harness(model: model, coordinator: coordinator)
    }

    // decision: APP-737, APP-705
    func testWithHoverOffADropOnTheClosedHandleOpensAndAttaches() throws {
        let h = try harness(hover: false)
        defer { h.coordinator.tearDown() }
        let urls = try scratchFiles(["a.txt", "b.txt"])
        XCTAssertEqual(h.model.drops.entered(.handle, files(urls)), .copy)
        XCTAssertTrue(h.model.drops.drop(files(urls)))
        XCTAssertTrue(h.model.bodyVisible)
        XCTAssertEqual(h.model.session.attachments.count, 2)
        XCTAssertTrue(h.model.staysOpenOnHoverOut)
    }

    // decision: APP-737, APP-739
    func testAHeldDragSpringsTheChatOpenAndLeavingWithoutADropFoldsIt() async throws {
        for hover in [false, true] {
            let h = try harness(hover: hover)
            defer { h.coordinator.tearDown() }
            let urls = try scratchFiles(["a.txt"])
            _ = h.model.drops.entered(.edgeRow, files(urls))
            await h.model.drops.springTask?.value
            XCTAssertTrue(h.model.bodyVisible, "hover \(hover): the held drag didn't open the chat")
            // Into the open chat, then away: nothing dropped, so the chat it opened folds.
            _ = h.model.drops.entered(.chat, files(urls))
            h.model.drops.exited(.edgeRow)
            XCTAssertTrue(h.model.bodyVisible)
            h.model.drops.exited(.chat)
            await h.model.drops.leaveTask?.value
            XCTAssertFalse(h.model.expanded, "hover \(hover): the spring-opened chat stayed after the drag left")
            XCTAssertTrue(h.model.session.attachments.isEmpty)

            // Leaving before the spring fires opens nothing.
            h.model.drops.wait = { _ in try await Task.sleep(for: .seconds(5)) }
            _ = h.model.drops.entered(.handle, files(urls))
            let spring = h.model.drops.springTask
            h.model.drops.exited(.handle)
            await spring?.value
            XCTAssertFalse(h.model.expanded)
        }
    }

    // decision: APP-737, APP-739
    func testADropWhileFERBYieldsBringsItBack() throws {
        let h = try harness(hover: false)
        defer { h.coordinator.tearDown() }
        h.model.expand()
        h.model.beginYield(.foreignOverlay)
        XCTAssertTrue(h.model.isYielded)
        let urls = try scratchFiles(["a.txt"])
        XCTAssertTrue(h.model.drops.drop(files(urls)))
        XCTAssertFalse(h.model.isYielded, "the drop didn't bring FERB back")
        XCTAssertTrue(h.model.bodyVisible)
    }
}

@MainActor
final class FileDragRoutingTests: XCTestCase {
    /// macOS 27.2 never routes a drag to a window in FERB's overlay space (probed: no draggingEntered at any alpha or
    /// level), so during a file drag the drop windows leave it and nothing puts them back until the drag ends.
    // decision: APP-742, APP-731
    func testHeldOutWindowsStayOutOfTheOverlaySpaceUntilTheDragEnds() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let window = NotchPanel(contentRect: CGRect(x: 0, y: 0, width: 2, height: 2),
                                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        window.orderFrontRegardless()
        let space = NotchOverlaySpace()
        defer {
            space.stop()
            window.orderOut(nil)
        }
        space.attach([window])
        guard space.isAttached else { throw XCTSkip("No SkyLight overlay spaces here: FERB keeps ordinary windows (APP-563)") }
        space.holdOut([window])
        XCTAssertEqual(space.membership(of: window)?.member, false)
        window.orderOut(nil)                    // the closed chat or a folding handle, ordered in again mid-drag
        // (whether it lands on the ordinary Spaces needs a running app's window server placement: a live check)
        window.orderFrontRegardless()
        XCTAssertFalse(space.ensure(window), "a held-out window was put back during the drag")
        XCTAssertEqual(space.membership(of: window)?.member, false)
        space.endHoldOut()
        XCTAssertEqual(space.membership(of: window)?.member, true)
    }

    // decision: APP-742, APP-739
    func testANewFileDragOnTheDragPasteboardStartsDragModeUntilTheButtonIsUp() async throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        model.setHoverEnabled(false)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        defer { coordinator.tearDown() }
        coordinator.updateScreen()
        let board = NSPasteboard(name: NSPasteboard.Name("ferb-drag-test-\(UUID().uuidString)"))
        defer { board.releaseGlobally() }
        coordinator.dragPasteboard = { board }
        var buttonDown = true
        coordinator.leftButtonDown = { buttonDown }
        coordinator.fileDragWait = { _ in await Task.yield() }
        let away = CGPoint(x: NSScreen.screens[0].frame.midX, y: NSScreen.screens[0].frame.minY + 20)
        coordinator.pointerLocation = { away }
        model.drops.wait = { _ in }

        board.clearContents()
        board.setString("text", forType: .string)
        coordinator.noticeDragPasteboard()
        XCTAssertFalse(coordinator.fileDragActive, "a text drag isn't a file drag")

        let url = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-drag-\(UUID().uuidString).txt")
        try Data("x".utf8).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        board.clearContents()
        board.writeObjects([url as NSURL])
        coordinator.noticeDragPasteboard()
        XCTAssertTrue(coordinator.fileDragActive)

        // Held over the handle, the chat springs open; the button comes up with no target told: the chat folds.
        _ = model.drops.entered(.handle, board)
        await model.drops.springTask?.value
        XCTAssertTrue(model.bodyVisible)
        buttonDown = false
        await coordinator.fileDragWatch?.value
        XCTAssertFalse(coordinator.fileDragActive)
        await model.drops.leaveTask?.value
        XCTAssertFalse(model.expanded, "the spring-opened chat stayed after the drag ended")

        coordinator.noticeDragPasteboard()
        XCTAssertFalse(coordinator.fileDragActive, "the same drag pasteboard is no new drag")
    }
}

/// A microphone that stays listening (dictation holds the composer) until cancelled.
@MainActor
private final class HeldMicrophone: VoiceCapturing {
    func start(update: @escaping (String) -> Void, level: @escaping ([Double], TimeInterval) -> Void,
               completion: @escaping (Result<String, Error>) -> Void) {}
    func finish() {}
    func cancel() {}
}
