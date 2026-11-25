import AppKit
import NotchCore
import SwiftUI

/// Reading the clipboard for a Command-V in the chat box (owner, 2026-11-17; APP-932, APP-933). ClipboardContent
/// decides from the items' types; this reads what it decided.
@MainActor
enum ComposerPaste {
    static func content(_ pasteboard: NSPasteboard) -> ClipboardContent {
        ClipboardContent.of(FileDropSession.itemTypes(pasteboard))
    }

    /// The copied files and folders.
    static func files(_ pasteboard: NSPasteboard) -> [URL] {
        (pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
    }

    /// The copied picture as PNG data: a PNG as it is, any other image type FERB reads made PNG.
    static func png(_ pasteboard: NSPasteboard) -> Data? {
        png(types: pasteboard.types?.map(\.rawValue) ?? [], data: { pasteboard.data(forType: NSPasteboard.PasteboardType($0)) })
    }

    /// The same for one item's `types`, read through `data` (a dragged item, APP-936).
    static func png(types: [String], data: (String) -> Data?) -> Data? {
        if types.contains("public.png"), let png = data("public.png"), NSBitmapImageRep(data: png) != nil { return png }
        for type in ClipboardContent.imageTypes where types.contains(type) {
            guard let bytes = data(type), let image = NSImage(data: bytes),
                  let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { continue }
            if let png = NSBitmapImageRep(cgImage: cgImage).representation(using: .png, properties: [:]) { return png }
        }
        return nil
    }

    /// "Pasted image 2026-11-17 at 10.02.15.png", in the way macOS names a screenshot.
    static func imageName(at date: Date) -> String {
        let format = DateFormatter()
        format.locale = Locale(identifier: "en_US_POSIX")
        format.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        return "Pasted image \(format.string(from: date)).png"
    }
}

/// A long paste held in the composer (APP-935), beside the attached files and the same size as their cards: click to
/// see and edit the whole text (APP-1143), the cross to take it out.
struct HeldPasteCard: View {
    let paste: HeldPaste
    @ObservedObject var model: NotchModel
    @State private var hovered = false

    var body: some View {
        Button { model.toggleShownPaste(paste.id) } label: {
            VStack(alignment: .leading, spacing: 5) {
                Image(systemName: "text.alignleft")
                    .font(.system(size: 20)).foregroundStyle(Palette.secondary)
                    .frame(maxWidth: .infinity, minHeight: 30)
                Text("Pasted text").font(.system(size: 11, weight: .medium)).lineLimit(1)
                Text(paste.size).font(.system(size: 10)).foregroundStyle(Palette.secondary).lineLimit(1)
            }
            .padding(10).frame(width: 116, height: 84)
            .background(.background.opacity(model.shownPaste == paste.id ? 0.95 : 0.7), in: RoundedRectangle(cornerRadius: 10))
            .cardHover($hovered, link: true, cornerRadius: 10, label: "pasted text")
            .contentShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .help("Click to see and edit the text")
        .accessibilityLabel("Pasted text, \(paste.size). Show and edit the text")
        .overlay(alignment: .topTrailing) {
            Button { model.removeHeldPaste(paste.id) } label: {
                Image(systemName: "xmark.circle.fill")
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.secondary, .background)
                    .font(.system(size: 14)).frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .help("Remove pasted text")
            .accessibilityLabel("Remove pasted text")
        }
    }
}

/// The whole text of the chip the owner clicked, in the composer above the field, to read and edit (owner, 2026-11-25,
/// APP-1143): what is in the editor is what is sent. Done and Escape close it with the edits kept; emptied, the chip
/// goes as its cross would (APP-1144).
struct ShownPasteView: View {
    let paste: HeldPaste
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Pasted text · \(paste.size)").font(.system(size: 11, weight: .medium))
                Spacer()
                Button("Done") { model.closeShownPaste() }.buttonStyle(FerbButtonStyle(quiet: true))
            }
            PasteEditor(paste: paste, model: model)
                .id(paste.id)                       // another chip gets its own editor and its own undo
                .background(.background.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
        }
        .padding(.horizontal, 8).padding(.top, 4)
        .frame(height: NotchModel.shownPasteHeight)
    }
}

/// The shown chip's text in the same native text view the composer uses (Undo and Redo, selection, cut, copy, paste,
/// input methods, Command-A), its own undo history, and Return a new line rather than a send (APP-1144).
struct PasteEditor: NSViewRepresentable {
    let paste: HeldPaste
    @ObservedObject var model: NotchModel

    func makeNSView(context: Context) -> NSScrollView {
        let view = PasteEditorTextView()
        Self.configure(view, paste: paste, model: model)
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = view
        return scroll
    }

    static func configure(_ view: PasteEditorTextView, paste: HeldPaste, model: NotchModel) {
        view.isRichText = false
        view.allowsUndo = true
        view.drawsBackground = false
        view.font = .systemFont(ofSize: 12)
        view.textColor = .white
        view.insertionPointColor = .controlAccentColor
        view.textContainerInset = NSSize(width: 6, height: 6)
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.isVerticallyResizable = true
        view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        view.textContainer?.containerSize.height = .greatestFiniteMagnitude
        view.isHorizontallyResizable = false
        view.autoresizingMask = [.width]
        view.string = paste.text
        view.setSelectedRange(NSRange(location: 0, length: 0))
        let id = paste.id
        view.changed = { [weak model] text in model?.editHeldPaste(id, text: text) }
        view.escape = { [weak model] in model?.closeShownPaste() ?? false }
        view.isEditable = model.canCompose
        view.setAccessibilityLabel("Pasted text")
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? PasteEditorTextView else { return }
        if view.string != paste.text, !view.hasMarkedText() { view.string = paste.text }
        view.isEditable = model.canCompose
        // Opened by the owner's click, or a focus request while open (hover back, a shortcut): the keyboard comes
        // here, not to the composer (ChatInput leaves it while an editor is open).
        if context.coordinator.focusRequest != model.focusRequest {
            context.coordinator.focusRequest = model.focusRequest
            if model.bodyVisible, model.canCompose, model.questionFocus == nil { view.focusWhenKey() }
            else { view.pendingFocus = false }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }
    final class Coordinator { var focusRequest: UUID? }
}

final class PasteEditorTextView: KeyFocusTextView, HandlesOwnReturn {
    /// The text after every change to it (typing, cut, paste, undo and redo, an input method): read from the text
    /// storage, since an undo changes the characters without a text-did-change report.
    var changed: ((String) -> Void)? {
        didSet {
            if let storageObserver { NotificationCenter.default.removeObserver(storageObserver) }
            storageObserver = nil
            guard changed != nil, let textStorage else { return }
            storageObserver = NotificationCenter.default.addObserver(
                forName: NSTextStorage.didProcessEditingNotification, object: textStorage, queue: nil
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self, self.textStorage?.editedMask.contains(.editedCharacters) == true else { return }
                    self.changed?(self.string)
                }
            }
        }
    }
    private var storageObserver: NSObjectProtocol?
    deinit { if let storageObserver { NotificationCenter.default.removeObserver(storageObserver) } }
    /// Escape: closes the editor, edits kept; false when nothing took it.
    var escape: (() -> Bool)?
    var pasteboard: NSPasteboard = .general
    /// Its own history, so an undo in the composer never reaches into a closed editor and back.
    private let history = UndoManager()
    override var undoManager: UndoManager? { history }

    /// Return and Shift-Return both put a new line here; nothing is sent from the editor (APP-1144). Escape closes it
    /// (an input method's own Escape stays the input method's).
    override func keyDown(with event: NSEvent) {
        if (event.keyCode == 36 || event.keyCode == 76), !hasMarkedText() {
            insertNewlineIgnoringFieldEditor(nil)
            return
        }
        if ChatWindowPanel.isPlainEscape(event), !hasMarkedText(), escape?() == true { return }
        super.keyDown(with: event)
    }

    override func cancelOperation(_ sender: Any?) {
        if hasMarkedText() || escape?() != true { super.cancelOperation(sender) }
    }

    /// Text only, without bracketed-paste markers copied out of a terminal (APP-713); a long one goes in whole, since
    /// this is already a held paste.
    override func paste(_ sender: Any?) {
        guard isEditable, let copied = pasteboard.string(forType: .string) else { return }
        let text = PasteMarkers.removed(from: copied)
        guard !text.isEmpty else { return }
        insertText(text, replacementRange: selectedRange())
    }
}

/// A promised file still on its way (APP-936), the same size as the file cards: its name when the source gave one and
/// "Arriving…", the cross to stop waiting for it.
struct ReceivingFileCard: View {
    let file: ReceivingFile
    @ObservedObject var model: NotchModel

    private var title: String { file.name ?? "Dropped file" }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            ProgressView().controlSize(.small).frame(maxWidth: .infinity, minHeight: 30)
            Text(title).font(.system(size: 11, weight: .medium)).lineLimit(1).truncationMode(.middle)
            Text("Arriving…").font(.system(size: 10)).foregroundStyle(Palette.secondary).lineLimit(1)
        }
        .padding(10).frame(width: 116, height: 84)
        .background(.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 10))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(title), arriving")
        .overlay(alignment: .topTrailing) {
            Button { model.cancelReceiving(file.id) } label: {
                Image(systemName: "xmark.circle.fill")
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.secondary, .background)
                    .font(.system(size: 14)).frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .help("Don't attach this file")
            .accessibilityLabel("Don't attach \(title)")
        }
    }
}
