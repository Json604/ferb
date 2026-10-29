import AppKit
import SwiftUI

/// Popovers need the same unmanaged Space as the notch, while retaining their own
/// window bounds so neither the chat mask nor its screen-edge placement clips them.
struct TaskPreviewWindow: NSViewRepresentable {
    let model: NotchModel

    func makeNSView(context: Context) -> Attachment {
        let view = Attachment()
        view.changed = { [weak model] id, window, attached in
            model?.onPreviewWindowChange?(id, window, attached)
            model?.setPreviewPopover(id, presented: attached)
        }
        return view
    }
    func updateNSView(_ nsView: Attachment, context: Context) {}
    static func dismantleNSView(_ nsView: Attachment, coordinator: ()) { nsView.detach() }

    final class Attachment: NSView {
        let id = UUID()
        var changed: ((UUID, NSWindow, Bool) -> Void)?
        private weak var attachedWindow: NSWindow?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard window !== attachedWindow else { return }
            detach()
            if let window {
                attachedWindow = window
                changed?(id, window, true)
            }
        }
        func detach() {
            if let attachedWindow { changed?(id, attachedWindow, false) }
            attachedWindow = nil
        }
    }
}
