import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Owner, 2026-11-17: "when i copy some or many files by selecting adn then cmd+c then cmd+v of thsoe fiels shoudlb
/// paste in ferb chat" (APP-932) and "if im pasting text adn if it longer than a certain limit, ot is hoein like this in
/// the box with click to see and cross to cancel" (APP-934). Driven through the real ChatTextView, ChatInput's wiring and
/// NotchModel with uniquely named private pasteboards and temporary folders; the clock is fixed.
@MainActor
final class ComposerPasteTests: XCTestCase {
    private func wrapped(_ text: String) -> String { "\u{1b}[200~" + text + "\u{1b}[201~" }

    private func scratchFolder() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-paste-\(UUID().uuidString)")
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

    /// A model whose received pictures go to a temporary folder, and the composer the owner types into.
    private func composer() throws -> (NotchModel, ChatTextView, ScriptedRPC) {
        let rpc = ScriptedRPC()
        rpc.replies["chat.attach"] = ["session_id": "test", "capabilities": ["input_paths"]]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.receivedFiles = ReceivedFiles(root: try scratchFolder().appendingPathComponent("FERB attachments"))
        model.now = { Date(timeIntervalSince1970: 1_800_000_000) }
        let editor = ChatTextView()
        ChatInput.configure(editor, model: model, coordinator: ChatInput.Coordinator(model))
        return (model, editor, rpc)
    }

    private func sentText(_ rpc: ScriptedRPC, _ model: NotchModel) async throws -> [String: Any] {
        let id = try XCTUnwrap(model.conversation.turns.last?.id)
        await model.submissions[id]?.value
        return try XCTUnwrap(rpc.calls.last { $0.0 == "chat.send" }?.1)
    }

    // MARK: - What the clipboard holds

    // decision: APP-933
    func testFilesWinOverTheirNamesAndAPictureWithNoFileIsAnImage() {
        // Finder: each file with its name as text (and the icon as a picture)
        XCTAssertEqual(ClipboardContent.of([["public.file-url", "public.utf8-plain-text", "public.tiff"],
                                            ["public.file-url", "public.utf8-plain-text"]]), .files)
        XCTAssertEqual(ClipboardContent.of([["public.png"]]), .image)                          // a copied screenshot
        XCTAssertEqual(ClipboardContent.of([["public.tiff", "public.html"]]), .image)          // a browser's Copy Image
        XCTAssertEqual(ClipboardContent.of([["public.tiff", "public.url", "public.utf8-plain-text"]]), .image)
        // A spreadsheet's cells carry a picture of themselves beside their text: the text wins
        XCTAssertEqual(ClipboardContent.of([["public.utf8-plain-text", "public.rtf", "public.tiff"]]), .text)
        XCTAssertEqual(ClipboardContent.of([["public.utf8-plain-text"]]), .text)
        XCTAssertEqual(ClipboardContent.of([["public.url", "public.utf8-plain-text"]]), .text)   // a link
        XCTAssertEqual(ClipboardContent.of([["public.file-url"], ["public.utf8-plain-text"]]), .text)   // not all files
        XCTAssertEqual(ClipboardContent.of([]), .nothing)
    }

    // decision: APP-930, APP-931
    func testTextAndLinksAreStillRefusedAsAFileDrop() {
        // Pasting files and pictures changes nothing about what a drag onto FERB may carry.
        XCTAssertFalse(FileDrop.accepts([["public.utf8-plain-text"]]))
        XCTAssertFalse(FileDrop.accepts([["public.url", "public.utf8-plain-text"]]))
        XCTAssertTrue(FileDrop.accepts([["public.file-url", "public.utf8-plain-text"]]))
    }

    // MARK: - Command-V with files or a picture

    // decision: APP-932, APP-933
    func testCommandVWithCopiedFilesAttachesThemAndTypesNoNames() throws {
        let (model, editor, rpc) = try composer()
        let folder = try scratchFolder()
        let urls = try ["notes.txt", "plan.pdf"].map { name -> URL in
            let url = folder.appendingPathComponent(name)
            try Data("x".utf8).write(to: url)
            return url
        }
        editor.insertText("Compare these", replacementRange: editor.selectedRange())
        editor.pasteboard = pasteboard { board in
            board.writeObjects(urls.map { $0 as NSURL })
            // Finder puts the names beside the files as text
            board.pasteboardItems?.first?.setString("notes.txt\nplan.pdf", forType: .string)
        }
        editor.paste(nil)
        XCTAssertEqual(model.session.attachments, urls.map(\.standardizedFileURL))
        XCTAssertEqual(model.session.draft, "Compare these", "the names are not typed")
        XCTAssertFalse(rpc.calls.contains { $0.0 == "chat.send" }, "a paste never sends")
        XCTAssertTrue(model.canSend)
    }

    // decision: APP-932, APP-933
    func testCommandVWithAPictureAttachesItAsAPNGFERBOwns() async throws {
        let (model, editor, rpc) = try composer()
        let png = try pngData()
        editor.pasteboard = pasteboard { $0.setData(png, forType: .png) }
        editor.paste(nil)
        let attached = try XCTUnwrap(model.session.attachments.first)
        XCTAssertEqual(model.session.attachments.count, 1)
        XCTAssertTrue(model.receivedFiles.owns(attached))
        XCTAssertEqual(attached.lastPathComponent, ComposerPaste.imageName(at: Date(timeIntervalSince1970: 1_800_000_000)))
        XCTAssertTrue(attached.lastPathComponent.hasPrefix("Pasted image "))
        XCTAssertEqual(try Data(contentsOf: attached), png)
        XCTAssertEqual(model.session.draft, "")

        // A TIFF (a browser's Copy Image) is made PNG
        let tiff = try XCTUnwrap(NSImage(data: png)?.tiffRepresentation)
        editor.pasteboard = pasteboard { $0.setData(tiff, forType: .tiff) }
        editor.paste(nil)
        let second = try XCTUnwrap(model.session.attachments.last)
        XCTAssertEqual(model.session.attachments.count, 2)
        XCTAssertNotNil(NSBitmapImageRep(data: try Data(contentsOf: second)))
        XCTAssertEqual(try Data(contentsOf: second).prefix(4), Data([0x89, 0x50, 0x4E, 0x47]))

        // Taken out before sending: deleted with its folder
        model.removeAttachment(second)
        XCTAssertFalse(FileManager.default.fileExists(atPath: second.deletingLastPathComponent().path))

        // Sent: the daemon gets its path, and it stays for the task until FERB.app next starts
        editor.insertText("What is this?", replacementRange: editor.selectedRange())
        model.startWork()
        let params = try await sentText(rpc, model)
        XCTAssertEqual(params["input_paths"] as? [String], [attached.path])
        model.removeAttachment(attached)                                 // a recalled copy taken out again
        XCTAssertTrue(FileManager.default.fileExists(atPath: attached.path), "a sent picture is kept for its task")
        let arrived = try XCTUnwrap(FileManager.default.attributesOfItem(atPath: attached.deletingLastPathComponent().path)[.modificationDate] as? Date)
        ReceivedFiles(root: model.receivedFiles.root).sweep(now: arrived.addingTimeInterval(3_600))   // a start soon after: kept
        XCTAssertTrue(FileManager.default.fileExists(atPath: attached.path))
        ReceivedFiles(root: model.receivedFiles.root).sweep(now: .distantFuture)   // a start a week on
        XCTAssertFalse(FileManager.default.fileExists(atPath: attached.path))
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: model.receivedFiles.root.path), [])
        model.disconnectDaemon()
    }

    // decision: APP-933, APP-930
    func testFilesAndPicturesWaitOutTheTourAndPasteAsTextThere() throws {
        let (model, editor, _) = try composer()
        model.onIntroAction = { _ in }                                    // the tour runs: no files (D18)
        editor.pasteboard = pasteboard { board in
            board.setData(try! self.pngData(), forType: .png)
            board.setString("caption", forType: .string)
        }
        editor.paste(nil)
        XCTAssertTrue(model.session.attachments.isEmpty)
        XCTAssertEqual(model.session.draft, "caption")
        XCTAssertFalse(FileManager.default.fileExists(atPath: model.receivedFiles.root.path),
                       "nothing written that isn't attached")
    }

    // MARK: - Long pastes

    // decision: APP-934, APP-935
    func testTheLimitIsTwoThousandCharactersOrThirtyLines() {
        XCTAssertEqual(LongPaste.limit.characters, 2_000)
        XCTAssertEqual(LongPaste.limit.lines, 30)
        XCTAssertFalse(LongPaste.isLong(String(repeating: "a", count: 2_000)))
        XCTAssertTrue(LongPaste.isLong(String(repeating: "a", count: 2_001)))
        let thirty = Array(repeating: "line", count: 30).joined(separator: "\n")
        XCTAssertFalse(LongPaste.isLong(thirty))
        XCTAssertFalse(LongPaste.isLong(thirty + "\n"), "a final line break starts no line")
        XCTAssertTrue(LongPaste.isLong(thirty + "\nline"))
        XCTAssertEqual(HeldPaste(id: UUID(), text: thirty + "\nline", location: 0).size, "31 lines")
        XCTAssertEqual(HeldPaste(id: UUID(), text: String(repeating: "a", count: 2_400), location: 0).size,
                       "\(2_400.formatted()) characters")
    }

    // decision: APP-934, APP-935, ORCH-518
    func testALongPasteIsAChipAndGoesOutWholeAtItsPlaceMarkedAsAPaste() async throws {
        let (model, editor, rpc) = try composer()
        let log = (1...240).map { "line \($0)" }.joined(separator: "\n")
        editor.insertText("Why does this fail? ", replacementRange: editor.selectedRange())
        editor.pasteboard = pasteboard { $0.setString(log, forType: .string) }
        editor.paste(nil)
        XCTAssertEqual(model.session.draft, "Why does this fail? ", "the field keeps only what the owner typed")
        XCTAssertEqual(model.heldPastes.map(\.text), [log])
        XCTAssertEqual(model.heldPastes.first?.size, "240 lines")
        XCTAssertTrue(model.canSend)

        // Typed words before and after it stay the owner's; a second chip goes after the first
        editor.insertText("Thanks.", replacementRange: editor.selectedRange())
        editor.setSelectedRange(NSRange(location: 0, length: 0))
        editor.insertText("Hi. ", replacementRange: editor.selectedRange())
        let second = String(repeating: "x", count: 2_100)
        editor.setSelectedRange(NSRange(location: (editor.string as NSString).length, length: 0))
        editor.pasteboard = pasteboard { $0.setString(second, forType: .string) }
        editor.paste(nil)
        XCTAssertEqual(model.heldPastes.count, 2)

        // Clicking a chip shows its whole text; the strip grows by the shown text
        let first = try XCTUnwrap(model.heldPastes.first)
        model.toggleShownPaste(first.id)
        XCTAssertEqual(model.shownHeldPaste?.text, log)
        XCTAssertEqual(model.attachmentStripHeight, 100 + NotchModel.shownPasteHeight)

        model.startWork()
        let params = try await sentText(rpc, model)
        XCTAssertEqual(params["text"] as? String,
                       "Hi. Why does this fail? " + wrapped(log) + "Thanks." + wrapped(second))
        let turn = try XCTUnwrap(model.conversation.turns.last)
        XCTAssertEqual(turn.request, "Hi. Why does this fail? " + log + "Thanks." + second, "the bubble holds the whole text")
        XCTAssertTrue(model.heldPastes.isEmpty)
        XCTAssertNil(model.shownPaste)
        XCTAssertEqual(model.attachmentStripHeight, 0)
        model.disconnectDaemon()
    }

    // decision: APP-934, APP-935
    func testTheCrossTakesAChipOutAndAChipAloneCanBeSent() async throws {
        let (model, editor, rpc) = try composer()
        let long = String(repeating: "y", count: 3_000)
        editor.pasteboard = pasteboard { $0.setString(long, forType: .string) }
        editor.paste(nil)
        editor.paste(nil)
        XCTAssertEqual(model.heldPastes.count, 2)
        model.removeHeldPaste(model.heldPastes[0].id)
        XCTAssertEqual(model.heldPastes.count, 1)
        XCTAssertEqual(model.session.draft, "")
        XCTAssertTrue(model.canSend, "a chip alone can be sent")
        model.startWork()
        let params = try await sentText(rpc, model)
        XCTAssertEqual(params["text"] as? String, wrapped(long))
        model.disconnectDaemon()
    }

    // decision: APP-596, APP-935
    func testTypedCommandWordsBesideAChipAreSentNotRun() async throws {
        let (model, editor, rpc) = try composer()
        editor.pasteboard = pasteboard { $0.setString(String(repeating: "z\n", count: 40), forType: .string) }
        editor.paste(nil)
        editor.insertText("turn hover off", replacementRange: editor.selectedRange())
        model.startWork()
        _ = try await sentText(rpc, model)
        XCTAssertNil(model.localSettingReply, "with pasted text in the message it is a message, not a notch command")
        model.disconnectDaemon()
    }

    // decision: APP-713, ORCH-518
    func testAShortPasteStillGoesIntoTheFieldMarked() {
        let rpc = ScriptedRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        let editor = ChatTextView()
        ChatInput.configure(editor, model: model, coordinator: ChatInput.Coordinator(model))
        let short = (1...30).map { "row \($0)" }.joined(separator: "\n")
        editor.pasteboard = pasteboard { $0.setString("[200~" + short + "[201~", forType: .string) }
        editor.paste(nil)
        XCTAssertEqual(model.session.draft, short, "leaked markers are still removed")
        XCTAssertTrue(model.heldPastes.isEmpty)
        XCTAssertEqual(PastedRequest(text: model.session.draft, provenance: model.pasteProvenance).markedText, wrapped(short))
    }

    // MARK: - Editing a chip's text (owner, 2026-11-25: "this click to expand text is good BUT also let add and remove
    // text in this too", APP-1143)

    /// A composer holding `text` as a chip after "Look: ", its editor open, as clicking the chip opens it.
    private func openEditor(_ text: String) throws -> (NotchModel, ChatTextView, PasteEditorTextView, ScriptedRPC) {
        let (model, composer, rpc) = try composer()
        composer.insertText("Look: ", replacementRange: composer.selectedRange())
        composer.pasteboard = pasteboard { $0.setString(text, forType: .string) }
        composer.paste(nil)
        let paste = try XCTUnwrap(model.heldPastes.first)
        model.toggleShownPaste(paste.id)
        let editor = PasteEditorTextView()
        PasteEditor.configure(editor, paste: try XCTUnwrap(model.shownHeldPaste), model: model)
        return (model, composer, editor, rpc)
    }

    private func key(_ code: UInt16, _ characters: String, _ flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0,
                                       context: nil, characters: characters, charactersIgnoringModifiers: characters,
                                       isARepeat: false, keyCode: code))
    }

    // decision: APP-1143, APP-1144, ORCH-518
    func testEditsInAChipsEditorAreExactlyWhatIsSentStillAsAPaste() async throws {
        let log = (1...240).map { "line \($0)" }.joined(separator: "\n")
        let (model, composer, editor, rpc) = try openEditor(log)
        XCTAssertEqual(editor.string, log, "the editor shows the whole text")
        // Remove the first line and add words at the end, as the owner would
        editor.setSelectedRange(NSRange(location: 0, length: ("line 1\n" as NSString).length))
        editor.insertText("", replacementRange: editor.selectedRange())
        editor.setSelectedRange(NSRange(location: (editor.string as NSString).length, length: 0))
        editor.insertText("\nend of log", replacementRange: editor.selectedRange())
        let edited = (2...240).map { "line \($0)" }.joined(separator: "\n") + "\nend of log"
        XCTAssertEqual(model.heldPastes.map(\.text), [edited])
        XCTAssertEqual(model.shownHeldPaste?.size, "240 lines", "the size line follows the edits")

        // Escape closes the editor and keeps the edits; the composer's own text is untouched
        editor.keyDown(with: try key(53, "\u{1b}"))
        XCTAssertNil(model.shownPaste)
        XCTAssertEqual(model.heldPastes.map(\.text), [edited])
        XCTAssertEqual(model.session.draft, "Look: ")

        composer.insertText("thanks", replacementRange: composer.selectedRange())
        model.startWork()
        let params = try await sentText(rpc, model)
        XCTAssertEqual(params["text"] as? String, "Look: " + wrapped(edited) + "thanks")
        XCTAssertEqual(model.conversation.turns.last?.request, "Look: " + edited + "thanks")
        model.disconnectDaemon()
    }

    // decision: APP-1143
    func testTheChipsLineCountFollowsItsEditsAndDoneKeepsThem() throws {
        let (model, _, editor, _) = try openEditor((1...419).map { "row \($0)" }.joined(separator: "\n"))
        XCTAssertEqual(model.heldPastes.first?.size, "419 lines")
        // Keep only the first 100 lines
        let keep = ((1...100).map { "row \($0)" }.joined(separator: "\n") as NSString).length
        editor.setSelectedRange(NSRange(location: keep, length: (editor.string as NSString).length - keep))
        editor.insertText("", replacementRange: editor.selectedRange())
        XCTAssertEqual(model.shownHeldPaste?.size, "100 lines")
        model.closeShownPaste()                                  // Done
        XCTAssertNil(model.shownPaste)
        XCTAssertEqual(model.heldPastes.first?.size, "100 lines")
        XCTAssertEqual(model.attachmentStripHeight, 100, "the strip is back to the chips alone")
        // Opened again, the editor has the edited text
        model.toggleShownPaste(try XCTUnwrap(model.heldPastes.first?.id))
        let again = PasteEditorTextView()
        PasteEditor.configure(again, paste: try XCTUnwrap(model.shownHeldPaste), model: model)
        XCTAssertEqual(again.string, (1...100).map { "row \($0)" }.joined(separator: "\n"))
    }

    // decision: APP-1143, APP-1144
    func testAChipEmptiedInItsEditorIsTakenOutWhenTheEditorCloses() throws {
        let (model, composer, editor, _) = try openEditor(String(repeating: "q", count: 2_500))
        editor.selectAll(nil)
        editor.insertText("", replacementRange: editor.selectedRange())
        XCTAssertEqual(model.heldPastes.count, 1, "emptied while open, it stays, so the owner can type a new text")
        composer.selectAll(nil)
        composer.insertText("", replacementRange: composer.selectedRange())
        XCTAssertFalse(model.canSend, "an emptied chip carries nothing to send")
        model.closeShownPaste()
        XCTAssertTrue(model.heldPastes.isEmpty, "closed empty, it goes as its cross would")
        XCTAssertNil(model.shownPaste)
        XCTAssertEqual(model.attachmentStripHeight, 0)

        // Emptied and left by clicking another chip: the same
        let (other, _, first, _) = try openEditor(String(repeating: "a", count: 2_500))
        let keptID = try XCTUnwrap(other.heldPastes.first?.id)
        other.holdPaste(String(repeating: "b", count: 2_500), at: 0)
        first.selectAll(nil)
        first.insertText("", replacementRange: first.selectedRange())
        other.toggleShownPaste(try XCTUnwrap(other.heldPastes.last?.id))
        XCTAssertFalse(other.heldPastes.contains { $0.id == keptID })
        XCTAssertEqual(other.heldPastes.count, 1)
    }

    // decision: APP-1144, APP-554
    func testReturnInAChipsEditorIsANewLineAndSendsNothing() throws {
        let (model, _, editor, rpc) = try openEditor((1...40).map { "x\($0)" }.joined(separator: "\n"))
        editor.setSelectedRange(NSRange(location: 2, length: 0))           // after "x1"
        editor.keyDown(with: try key(36, "\r"))
        editor.keyDown(with: try key(76, "\u{3}"))                         // the keypad's Enter
        XCTAssertTrue(editor.string.hasPrefix("x1\n\n\nx2"))
        XCTAssertTrue(model.heldPastes.first?.text.hasPrefix("x1\n\n\nx2") == true)
        XCTAssertTrue(model.conversation.turns.isEmpty)
        XCTAssertFalse(rpc.calls.contains { $0.0 == "chat.send" })
        XCTAssertNotNil(model.shownPaste, "the editor stays open")
        // The panels' Return-to-send monitor passes it by, as it does the composer
        XCTAssertTrue((editor as NSResponder) is HandlesOwnReturn)
        XCTAssertTrue((ChatTextView() as NSResponder) is HandlesOwnReturn)
        XCTAssertFalse((NSTextView() as NSResponder) is HandlesOwnReturn)
    }

    // decision: APP-1144
    func testAChipsEditorHasItsOwnUndo() throws {
        let (model, composer, editor, _) = try openEditor(String(repeating: "u", count: 2_100))
        XCTAssertNotNil(editor.undoManager, "its own history, not the window's the composer uses")
        XCTAssertNil(composer.undoManager)
        editor.undoManager?.groupsByEvent = false
        editor.undoManager?.beginUndoGrouping()
        editor.insertText("!", replacementRange: NSRange(location: 0, length: 0))
        editor.undoManager?.endUndoGrouping()
        XCTAssertEqual(model.heldPastes.first?.text.first, "!")
        editor.undoManager?.undo()
        XCTAssertEqual(model.heldPastes.first?.text, String(repeating: "u", count: 2_100), "undo reaches what is sent")
        editor.undoManager?.redo()
        XCTAssertEqual(model.heldPastes.first?.text.first, "!", "and redo")
    }

    // MARK: - Where a chip sits as the owner edits

    // decision: APP-935
    func testAHeldPasteFollowsTheEditsAroundIt() {
        var provenance = PasteProvenance()
        provenance.edit(NSRange(location: 0, length: 0), with: "Fix: please", in: "")
        provenance.hold("LONG", at: 5, in: "Fix: please")                      // pasted between "Fix: " and "please"
        provenance.edit(NSRange(location: 0, length: 0), with: "Hi. ", in: "Fix: please")   // typed before it
        provenance.edit(NSRange(location: 9, length: 0), with: "now ", in: "Hi. Fix: please")  // typed at its place
        let draft = "Hi. Fix: now please"
        XCTAssertEqual(provenance.expanded(draft).text, "Hi. Fix: LONGnow please")
        XCTAssertEqual(PastedRequest(text: provenance.expanded(draft).text, provenance: provenance.expanded(draft).provenance).markedText,
                       "Hi. Fix: " + wrapped("LONG") + "now please")
        // Text around it removed: it sits where that text was; the field emptied keeps the chip
        provenance.edit(NSRange(location: 4, length: 9), with: "", in: draft)
        XCTAssertEqual(provenance.expanded("Hi. please").text, "Hi. LONGplease")
        provenance.edit(NSRange(location: 0, length: 10), with: "", in: "Hi. please")
        XCTAssertEqual(provenance.held.count, 1)
        XCTAssertEqual(provenance.expanded("").text, "LONG")
        // An unreported change (dictation) at its place goes after it, as typing there does; one before it moves it
        provenance.follow("thanks")
        XCTAssertEqual(provenance.expanded("thanks").text, "LONGthanks")
        provenance.follow("Read: thanks")
        XCTAssertEqual(provenance.expanded("Read: thanks").text, "LONGRead: thanks")
        provenance.hold("SECOND", at: 6, in: "Read: thanks")
        XCTAssertEqual(provenance.expanded("Read: thanks").text, "LONGRead: SECONDthanks")
        // Two held at one place keep the order they were pasted in
        var two = PasteProvenance()
        two.hold("A", at: 0, in: "")
        two.hold("B", at: 0, in: "")
        XCTAssertEqual(two.expanded("").text, "AB")
    }
}
