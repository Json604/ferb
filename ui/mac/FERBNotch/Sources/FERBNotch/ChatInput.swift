import AppKit
import SwiftUI

/// Own Return handling so AppKit never treats submission as an unhandled command.
struct ChatInput: NSViewRepresentable {
    @ObservedObject var model: NotchModel

    func makeNSView(context: Context) -> NSScrollView {
        let view = ChatTextView()
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
        view.delegate = context.coordinator
        view.submit = { [weak model] in model?.startWork() }
        view.didPaste = { [weak model, weak view] text in
            guard let model, let view else { return }
            model.pastedTexts.append(text)
            model.lastProvenanceDraft = (view.string as NSString).replacingCharacters(in: view.selectedRange(), with: text)
        }
        view.setAccessibilityLabel("Your message")
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = false
        scroll.documentView = view
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? ChatTextView else { return }
        if view.string != model.session.draft { view.string = model.session.draft; view.setSelectedRange(NSRange(location: view.string.utf16.count, length: 0)) }
        view.isEditable = model.canCompose
        if context.coordinator.focusRequest != model.focusRequest {
            context.coordinator.focusRequest = model.focusRequest
            if model.bodyVisible, model.canCompose, view.window?.isKeyWindow == true {
                view.window?.makeFirstResponder(view)
            }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(model) }
    final class Coordinator: NSObject, NSTextViewDelegate {
        let model: NotchModel
        var focusRequest: UUID?
        init(_ model: NotchModel) { self.model = model }
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
    var didPaste: ((String) -> Void)?
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
        guard isEditable, let text = pasteboard.string(forType: .string) else { return }
        didPaste?(text)
        insertText(text, replacementRange: selectedRange())
    }
}
