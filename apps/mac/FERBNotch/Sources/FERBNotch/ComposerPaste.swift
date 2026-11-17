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
/// see the whole text, the cross to take it out.
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
        .help("Click to see the whole text")
        .accessibilityLabel("Pasted text, \(paste.size). Show the whole text")
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

/// The whole text of the chip the owner clicked, in the composer above the field: read and select, nothing to edit.
struct ShownPasteView: View {
    let paste: HeldPaste
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Pasted text · \(paste.size)").font(.system(size: 11, weight: .medium))
                Spacer()
                Button("Done") { model.toggleShownPaste(paste.id) }.buttonStyle(FerbButtonStyle(quiet: true))
            }
            ScrollView {
                Text(paste.text)
                    .font(.system(size: 12)).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(.background.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
        }
        .padding(.horizontal, 8).padding(.top, 4)
        .frame(height: NotchModel.shownPasteHeight)
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
