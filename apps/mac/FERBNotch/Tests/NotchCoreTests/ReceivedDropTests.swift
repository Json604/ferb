import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Owner, 2026-11-17: "when i took screenshot and dragged that startight up, it didnt work, it did open ferb nothc but
/// didint pasted in notchchat", and for the sticky window "the ss fix needs to be done in this mode too" (APP-930). The
/// screenshot thumbnail is a promised file; an image dragged out of a page or Preview is a picture with no file. Both go
/// through the one FileDropSession every target (the notch, the pill, the sticky window, the field) reports to, into
/// FERB's own folder (APP-936). Real NotchModel, uniquely named private pasteboards, temporary folders, a fixed clock;
/// the source's write is a stand-in that runs when the test says.
@MainActor
final class ReceivedDropTests: XCTestCase {
    private final class StandInPromise: PromisedFile {
        let name: String?
        private(set) var folder: URL?
        private var arrived: (@MainActor (Result<URL, Error>) -> Void)?
        init(name: String?) { self.name = name }
        func receive(into folder: URL, arrived: @escaping @MainActor (Result<URL, Error>) -> Void) {
            self.folder = folder
            self.arrived = arrived
        }
        /// The source writes the file now.
        @discardableResult
        func write(_ data: Data = Data("shot".utf8)) throws -> URL {
            let url = try XCTUnwrap(folder).appendingPathComponent(name ?? "file")
            try data.write(to: url)
            arrived?(.success(url))
            return url
        }
        func fail() { arrived?(.failure(CocoaError(.fileWriteUnknown))) }
    }

    private func scratchFolder() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-received-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    private func pasteboard(_ write: (NSPasteboard) -> Void) -> NSPasteboard {
        let board = NSPasteboard.withUniqueName()
        board.clearContents()
        write(board)
        addTeardownBlock { board.releaseGlobally() }
        return board
    }

    private func pngData() throws -> Data {
        let rep = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8,
                                                 samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                                 colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        return try XCTUnwrap(rep.representation(using: .png, properties: [:]))
    }

    private func makeModel() throws -> (NotchModel, ScriptedRPC) {
        let rpc = ScriptedRPC()
        rpc.replies["chat.attach"] = ["session_id": "test", "capabilities": ["input_paths"]]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.receivedFiles = ReceivedFiles(root: try scratchFolder().appendingPathComponent("FERB attachments"))
        model.now = { Date(timeIntervalSince1970: 1_800_000_000) }
        return (model, rpc)
    }

    // decision: APP-930, APP-936
    func testAPromisedFileShowsAsArrivingThenWaitsInTheComposerAndGoesWithTheMessage() async throws {
        let (model, rpc) = try makeModel()
        let shot = StandInPromise(name: "Screenshot 2026-11-17 at 10.02.15.png")
        XCTAssertFalse(model.expanded)
        XCTAssertTrue(model.drop(DroppedItems(promises: [shot])))
        XCTAssertTrue(model.bodyVisible, "the drop opens the chat as a file drop does")
        XCTAssertTrue(model.staysOpenOnHoverOut)
        XCTAssertEqual(model.receivingFiles.map(\.name), ["Screenshot 2026-11-17 at 10.02.15.png"])
        XCTAssertEqual(model.attachmentStripHeight, 100, "the arriving card shows in the strip")
        XCTAssertTrue(model.receivedFiles.owns(try XCTUnwrap(shot.folder)))

        // Sending before it is there would leave it behind: FERB says so and sends nothing
        model.session.draft = "What does this show?"
        model.startWork()
        XCTAssertEqual(model.promptError, NotchModel.stillReceiving)
        XCTAssertFalse(rpc.calls.contains { $0.0 == "chat.send" })

        let url = try shot.write()
        XCTAssertTrue(model.receivingFiles.isEmpty)
        XCTAssertEqual(model.session.attachments, [url.standardizedFileURL])
        model.startWork()
        let id = try XCTUnwrap(model.conversation.turns.last?.id)
        await model.submissions[id]?.value
        let sent = try XCTUnwrap(rpc.calls.last { $0.0 == "chat.send" }?.1)
        XCTAssertEqual(sent["input_paths"] as? [String], [url.standardizedFileURL.path])
        XCTAssertTrue(FileManager.default.fileExists(atPath: url.path), "kept for its task")
        model.disconnectDaemon()
    }

    // decision: APP-930, APP-936
    func testAFileThatCannotBeReceivedIsSaidNeverLostSilently() throws {
        let (model, _) = try makeModel()
        let mail = StandInPromise(name: nil)
        model.drop(DroppedItems(promises: [mail]))
        XCTAssertEqual(model.receivingFiles.map(\.name), [nil])
        mail.fail()
        XCTAssertTrue(model.receivingFiles.isEmpty)
        XCTAssertTrue(model.session.attachments.isEmpty)
        XCTAssertEqual(model.promptError, NotchModel.receiveFailed)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: model.receivedFiles.root.path), [],
                       "its empty folder is gone")

        // A promise the pasteboard didn't hand over is said the same way
        model.promptError = nil
        XCTAssertTrue(model.drop(DroppedItems(unreadable: 1)))
        XCTAssertEqual(model.promptError, NotchModel.receiveFailed)
    }

    // decision: APP-936
    func testTheArrivingCardsCrossDropsTheFileWhenItComes() throws {
        let (model, _) = try makeModel()
        let shot = StandInPromise(name: "Screenshot.png")
        model.drop(DroppedItems(promises: [shot]))
        let arriving = try XCTUnwrap(model.receivingFiles.first)
        model.cancelReceiving(arriving.id)
        XCTAssertTrue(model.receivingFiles.isEmpty)
        let url = try shot.write()
        XCTAssertTrue(model.session.attachments.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path))
        XCTAssertNil(model.promptError)
    }

    // decision: APP-930, APP-931, APP-936
    func testAPictureWithNoFileAndAFileDroppedTogetherBothAttach() throws {
        let (model, _) = try makeModel()
        let folder = try scratchFolder()
        let notes = folder.appendingPathComponent("notes.txt")
        try Data("x".utf8).write(to: notes)
        let png = try pngData()
        let board = pasteboard { board in
            let file = NSPasteboardItem()
            file.setString(notes.absoluteString, forType: .fileURL)
            let picture = NSPasteboardItem()                          // an image dragged out of a page
            picture.setData(try! NSImage(data: png)!.tiffRepresentation!, forType: .tiff)
            picture.setString("https://example.com/cat.png", forType: .URL)
            board.writeObjects([file, picture])
        }
        XCTAssertEqual(model.drops.entered(.chat, board), .copy)
        XCTAssertTrue(model.drops.drop(board))
        XCTAssertEqual(model.session.attachments.count, 2)
        XCTAssertEqual(model.session.attachments.first, notes.standardizedFileURL)
        let picture = try XCTUnwrap(model.session.attachments.last)
        XCTAssertTrue(model.receivedFiles.owns(picture))
        XCTAssertEqual(picture.lastPathComponent, ComposerPaste.imageName(at: Date(timeIntervalSince1970: 1_800_000_000)))
        XCTAssertEqual(try Data(contentsOf: picture).prefix(4), Data([0x89, 0x50, 0x4E, 0x47]))
        model.removeAttachment(picture)
        XCTAssertFalse(FileManager.default.fileExists(atPath: picture.deletingLastPathComponent().path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: notes.path), "the owner's own file is never deleted")
        model.removeAttachment(notes.standardizedFileURL)
        XCTAssertTrue(FileManager.default.fileExists(atPath: notes.path))
    }

    // decision: APP-930, APP-931
    func testAPromiseOnAPasteboardIsReadForTheDrop() throws {
        final class Source: NSObject, NSFilePromiseProviderDelegate {
            func filePromiseProvider(_ provider: NSFilePromiseProvider, fileNameForType fileType: String) -> String { "a.png" }
            func filePromiseProvider(_ provider: NSFilePromiseProvider, writePromiseTo url: URL,
                                     completionHandler: @escaping (Error?) -> Void) { completionHandler(nil) }
        }
        let source = Source()
        let provider = NSFilePromiseProvider(fileType: "public.png", delegate: source)
        let board = pasteboard { $0.writeObjects([provider]) }
        XCTAssertTrue(FileDropSession.takes(board))
        let items = try XCTUnwrap(FileDropSession.items(on: board))
        XCTAssertEqual(items.promises.count, 1, "AppKit's promise receiver is read off the pasteboard")
        XCTAssertEqual(items.unreadable, 0)
        XCTAssertTrue(items.files.isEmpty)
        withExtendedLifetime(source) {}
    }

    // decision: APP-936
    func testTheStartSweepKeepsWhatAStillRunningTaskMayUse() throws {
        let received = ReceivedFiles(root: try scratchFolder().appendingPathComponent("FERB attachments"))
        let fresh = try received.save(Data("a".utf8), named: "fresh.png")
        let old = try received.save(Data("b".utf8), named: "old.png")
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        try FileManager.default.setAttributes([.modificationDate: now.addingTimeInterval(-60)],
                                              ofItemAtPath: fresh.deletingLastPathComponent().path)
        try FileManager.default.setAttributes([.modificationDate: now.addingTimeInterval(-ReceivedFiles.keptFor - 60)],
                                              ofItemAtPath: old.deletingLastPathComponent().path)
        received.sweep(now: now)
        XCTAssertTrue(FileManager.default.fileExists(atPath: fresh.path), "a day-old file may belong to a running task")
        XCTAssertFalse(FileManager.default.fileExists(atPath: old.deletingLastPathComponent().path))
        XCTAssertEqual(ReceivedFiles.keptFor, 7 * 24 * 60 * 60)
    }
}
