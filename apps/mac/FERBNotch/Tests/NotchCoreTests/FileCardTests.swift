import AppKit
import QuickLookUI
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// File cards and links in the chat (APP-715, APP-716): a click reveals or opens, the name is selectable, nothing
/// opens on hover and FERB's own views hold no Quick Look view.
@MainActor
final class FileCardTests: XCTestCase {
    private var revealed: [[URL]] = []
    private var opened: [URL] = []
    private var pasteboard: NSPasteboard!

    override func setUp() {
        revealed = []
        opened = []
        pasteboard = NSPasteboard.withUniqueName()
    }

    override func tearDown() { pasteboard.releaseGlobally() }

    private func makeModel() -> NotchModel {
        let model = NotchModel(preferences: scratchDefaults())
        model.revealInFinder = { [unowned self] in self.revealed.append($0) }
        model.openURL = { [unowned self] in self.opened.append($0) }
        model.pasteboard = pasteboard
        model.onQuickLook = { _ in XCTFail("a click never opens Quick Look") }
        model.onOpenDocument = { _ in XCTFail("a click never opens the file") }
        return model
    }

    private func scratchFile() throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: folder) }
        let url = folder.appendingPathComponent("report final.md")
        try Data("x".utf8).write(to: url)
        return url
    }

    private func host(_ view: some View, sized: Bool = true) -> NSHostingView<AnyView> {
        let hosting = NSHostingView(rootView: sized ? AnyView(view.frame(width: 360, height: 140)) : AnyView(view))
        hosting.frame = NSRect(x: 0, y: 0, width: 360, height: 140)
        let window = NSWindow(contentRect: hosting.frame, styleMask: [.borderless], backing: .buffered, defer: true)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        return hosting
    }

    private func views(in view: NSView) -> [NSView] { [view] + view.subviews.flatMap { views(in: $0) } }

    private func name(in hosting: NSView, _ text: String) throws -> SelectableNameView {
        try XCTUnwrap(views(in: hosting).compactMap { $0 as? SelectableNameView }.first { $0.string == text })
    }

    // decision: APP-715
    /// Every file card in chat (a reply's file, a task's file chip, a composer attachment) reveals its file in Finder
    /// on a plain click, and no card's view tree holds a QLPreviewView (crash 2026-11-05: a hover popover's
    /// QLPreviewView closed before Quick Look activated and aborted FERBNotch).
    func testAFileCardsClickRevealsItInFinderAndNoCardHoldsAQuickLookView() throws {
        let url = try scratchFile()
        let model = makeModel()
        let cards: [AnyView] = [AnyView(ArtifactRow(url: url, model: model)), AnyView(FileChip(url: url, model: model)),
                                AnyView(ComposerAttachmentCard(url: url, model: model))]
        for card in cards {
            let hosting = host(card)
            try name(in: hosting, "report final.md").finishPress(clickCount: 1)
            XCTAssertFalse(views(in: hosting).contains { $0 is QLPreviewView })
        }
        XCTAssertEqual(revealed, [[url], [url], [url]])
        XCTAssertEqual(model.previewPopoverCount, 0)
    }

    // decision: APP-715
    func testAMissingFileRevealsNothingAndTheCardSaysSo() throws {
        let url = try scratchFile()
        try FileManager.default.removeItem(at: url)
        let model = makeModel()
        let card = FileCardState()
        card.reveal(url, with: model)
        XCTAssertTrue(card.unavailable)
        XCTAssertEqual(revealed, [])
        XCTAssertEqual(FileCardState.unavailableLine, "This file is no longer available.")

        let hosting = host(ArtifactRow(url: url, model: model), sized: false)
        let before = hosting.fittingSize.height
        try name(in: hosting, "report final.md").finishPress(clickCount: 1)
        RunLoop.main.run(mode: .default, before: Date())           // one turn: SwiftUI applies the published change
        hosting.layoutSubtreeIfNeeded()
        XCTAssertGreaterThan(hosting.fittingSize.height, before)    // the line is shown under the card
        XCTAssertEqual(revealed, [])
    }

    // decision: APP-716
    /// Selecting the name (a drag, a double-click) never reveals; the menu copies the selection and the path.
    func testSelectingTheNameDoesNotActAndTheMenuCopiesThePath() throws {
        let url = try scratchFile()
        let model = makeModel()
        let hosting = host(ArtifactRow(url: url, model: model))
        let view = try name(in: hosting, "report final.md")
        view.setSelectedRange(NSRange(location: 0, length: 6))
        view.finishPress(clickCount: 1)                           // a drag that selected "report"
        view.finishPress(clickCount: 2)                           // a double-click
        XCTAssertEqual(revealed, [])
        let event = NSEvent.mouseEvent(with: .rightMouseDown, location: .zero, modifierFlags: [], timestamp: 0,
                                       windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        let menu = try XCTUnwrap(view.menu(for: event))
        XCTAssertEqual(menu.items.filter { !$0.isSeparatorItem }.map(\.title),
                       ["Copy", "Open in default app", "Quick Look", "Reveal in Finder", "Copy path"])
        let copyPath = try XCTUnwrap(menu.items.first { $0.title == "Copy path" })
        _ = copyPath.target?.perform(copyPath.action, with: copyPath)
        XCTAssertEqual(pasteboard.string(forType: .string), url.path)
        view.setSelectedRange(NSRange(location: 0, length: 0))
        view.finishPress(clickCount: 1)
        XCTAssertEqual(revealed, [[url]])
    }

    // decision: APP-715, APP-589
    /// A site in the sites row opens its https page on click and copies its link; anything that isn't a plain host
    /// opens nothing.
    func testASiteOpensOnClickAndOnlyAPlainHostBecomesALink() throws {
        let model = makeModel()
        let hosting = host(VisitedSitesView(sites: VisitedSites(hosts: ["example.com", "github.com"]), model: model))
        let site = try name(in: hosting, "example.com")
        site.finishPress(clickCount: 1)
        XCTAssertEqual(opened, [URL(string: "https://example.com")!])
        XCTAssertFalse(views(in: hosting).contains { $0 is QLPreviewView })
        let event = NSEvent.mouseEvent(with: .rightMouseDown, location: .zero, modifierFlags: [], timestamp: 0,
                                       windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        let copyLink = try XCTUnwrap(site.menu(for: event)?.items.first { $0.title == "Copy link" })
        _ = copyLink.target?.perform(copyLink.action, with: copyLink)
        XCTAssertEqual(pasteboard.string(forType: .string), "https://example.com")

        XCTAssertEqual(VisitedSites.link("localhost:8080")?.absoluteString, "https://localhost:8080")
        for host in ["", "javascript:alert(1)", "evil.com/path", "me@evil.com", "a b.com", "a.com?x=1", "a.com#x"] {
            XCTAssertNil(VisitedSites.link(host), host)
            XCTAssertFalse(model.openSite(host), host)
        }
        XCTAssertEqual(opened.count, 1)
    }
}

/// Paste, undo and redo reach the notch's fields although FERB never activates (APP-717).
@MainActor
final class PanelEditKeysTests: XCTestCase {
    final class AlwaysUndo: UndoManager {
        override var canUndo: Bool { true }
        override var canRedo: Bool { true }
    }

    final class RecordingField: NSTextView {
        var received: [String] = []
        let undo = AlwaysUndo()
        override var undoManager: UndoManager? { undo }
        override func paste(_ sender: Any?) { received.append("paste") }
        @objc func undo(_ sender: Any?) { received.append("undo") }
        @objc func redo(_ sender: Any?) { received.append("redo") }
    }

    private func key(_ characters: String, _ flags: NSEvent.ModifierFlags, code: UInt16) -> NSEvent {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0,
                         context: nil, characters: characters, charactersIgnoringModifiers: characters,
                         isARepeat: false, keyCode: code)!
    }

    private func panel(holding view: NSView) -> NotchPanel {
        let panel = NotchPanel(contentRect: NSRect(x: 0, y: 0, width: 300, height: 100),
                               styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        view.frame = NSRect(x: 0, y: 0, width: 300, height: 100)
        panel.contentView?.addSubview(view)
        XCTAssertTrue(panel.makeFirstResponder(view))
        return panel
    }

    // decision: APP-717
    func testThePanelSendsPasteUndoAndRedoToTheFocusedField() {
        let field = RecordingField()
        let panel = panel(holding: field)
        XCTAssertTrue(panel.performKeyEquivalent(with: key("v", .command, code: 9)))
        XCTAssertTrue(panel.performKeyEquivalent(with: key("z", .command, code: 6)))
        XCTAssertTrue(panel.performKeyEquivalent(with: key("Z", [.command, .shift], code: 6)))
        XCTAssertEqual(field.received, ["paste", "undo", "redo"])
        field.isHidden = true                                            // a hidden field gets nothing (APP-558)
        _ = panel.performKeyEquivalent(with: key("v", .command, code: 9))
        XCTAssertEqual(field.received, ["paste", "undo", "redo"])
        XCTAssertFalse(NotchPanel.canPerform(Selector(("undo:")), undo: UndoManager()))   // nothing to undo: not sent
    }

    // decision: APP-717, APP-596
    /// The composer's paste runs once through the panel: one provenance record, the text inserted once.
    func testCommandVInTheComposerPastesOnceThroughThePanel() {
        let editor = ChatTextView()
        editor.isRichText = false
        editor.pasteboard = NSPasteboard.withUniqueName()
        defer { editor.pasteboard.releaseGlobally() }
        editor.pasteboard.setString("pasted", forType: .string)
        var provenance: [String] = []
        editor.didPaste = { text, _ in provenance.append(text) }
        let panel = panel(holding: editor)
        XCTAssertTrue(panel.performKeyEquivalent(with: key("v", .command, code: 9)))
        XCTAssertEqual(editor.string, "pasted")
        XCTAssertEqual(provenance, ["pasted"])
    }
}
