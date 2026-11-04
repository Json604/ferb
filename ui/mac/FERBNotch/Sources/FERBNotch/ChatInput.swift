import AppKit
import NotchCore
import SwiftUI

/// Own Return handling so AppKit never treats submission as an unhandled command.
struct ChatInput: NSViewRepresentable {
    @ObservedObject var model: NotchModel

    func makeNSView(context: Context) -> NSScrollView {
        let view = ChatTextView()
        Self.configure(view, model: model, coordinator: context.coordinator)
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = false
        scroll.documentView = view
        return scroll
    }

    static func configure(_ view: ChatTextView, model: NotchModel, coordinator: Coordinator) {
        view.isRichText = false
        view.allowsUndo = true
        view.drawsBackground = false
        view.font = .systemFont(ofSize: 14)
        view.textColor = .white
        view.insertionPointColor = .controlAccentColor
        view.textContainerInset = NSSize(width: 0, height: 6)
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.isVerticallyResizable = true
        view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        view.textContainer?.containerSize.height = .greatestFiniteMagnitude
        view.isHorizontallyResizable = false
        view.autoresizingMask = [.width]
        view.delegate = coordinator
        view.submit = { [weak model] in model?.startWork() }
        // Where pasted text sits is recorded before it is inserted, independent of the edit report below.
        view.didPaste = { [weak model, weak view] text, selection in
            guard let model, let view else { return }
            model.pasteProvenance.paste(text, over: selection, in: view.string)
        }
        view.setAccessibilityLabel("Your message")
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? ChatTextView else { return }
        if view.string != model.session.draft {
            let previousSelection = view.selectedRange()
            view.string = model.session.draft
            let end = view.string.utf16.count
            let location = min(previousSelection.location, end)
            view.setSelectedRange(NSRange(location: location, length: min(previousSelection.length, end - location)))
        }
        view.isEditable = model.canCompose
        if context.coordinator.focusRequest != model.focusRequest {
            context.coordinator.focusRequest = model.focusRequest
            // The question field the owner chose to answer keeps focus over the composer.
            if model.bodyVisible, model.canCompose, model.questionFocus == nil, view.window?.isKeyWindow == true {
                view.window?.makeFirstResponder(view)
            }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(model) }
    final class Coordinator: NSObject, NSTextViewDelegate {
        let model: NotchModel
        var focusRequest: UUID?
        init(_ model: NotchModel) { self.model = model }
        /// Every edit, as AppKit is about to apply it, so the paste provenance knows exactly which words were typed.
        func textView(_ textView: NSTextView, shouldChangeTextIn range: NSRange, replacementString: String?) -> Bool {
            guard let replacementString, let view = textView as? ChatTextView, !view.isPasting else { return true }
            // Text from another app (drag, Services) and text put back by undo or redo has no known typed origin.
            let undoing = textView.undoManager.map { $0.isUndoing || $0.isRedoing } ?? false
            model.pasteProvenance.edit(range, with: replacementString, in: textView.string,
                                       pasted: view.isReadingPasteboard || undoing)
            return true
        }
        func textDidChange(_ notification: Notification) {
            guard let view = notification.object as? NSTextView else { return }
            model.session.draft = view.string
            model.onInteraction?()
            model.composerChanged()
        }
    }
}

final class ChatTextView: NSTextView {
    var pasteboard: NSPasteboard = .general
    var submit: (() -> Void)?
    var didPaste: ((String, NSRange) -> Void)?
    /// A Command-V insertion already recorded through `didPaste`.
    private(set) var isPasting = false
    /// Text arriving from a pasteboard other than Command-V: a drop or a Services insertion.
    private(set) var isReadingPasteboard = false
    override func keyDown(with event: NSEvent) {
        if (event.keyCode == 36 || event.keyCode == 76), !hasMarkedText() {
            if event.modifierFlags.contains(.shift) { insertNewlineIgnoringFieldEditor(nil) }
            else { submit?() }
            return
        }
        super.keyDown(with: event)
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask).subtracting([.capsLock, .numericPad])
        if flags == .command, event.charactersIgnoringModifiers?.lowercased() == "v", isEditable {
            paste(nil)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
    override func paste(_ sender: Any?) {
        // Bracketed-paste markers copied out of a terminal never enter the draft (APP-713): clean-up only.
        guard isEditable, let copied = pasteboard.string(forType: .string) else { return }
        let text = PasteMarkers.removed(from: copied)
        guard !text.isEmpty else { return }
        let selection = selectedRange()
        didPaste?(text, selection)
        isPasting = true
        defer { isPasting = false }
        insertText(text, replacementRange: selection)
    }
    override func readSelection(from pboard: NSPasteboard, type: NSPasteboard.PasteboardType) -> Bool {
        isReadingPasteboard = true
        defer { isReadingPasteboard = false }
        return super.readSelection(from: pboard, type: type)
    }
}
