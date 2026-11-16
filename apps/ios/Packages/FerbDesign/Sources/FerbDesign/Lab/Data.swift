// Copied from ~/Desktop/ferb-glass-lab Sources/FerbGlass/Data.swift at 23a0fde (LabSource.commit): only its first part,
// FileKind and FileIcon, which the phone draws (MobileScreens.swift's recent files). The rest of Data.swift (file
// cards and chips, facts, tables, charts, callouts, code) is the Mac's and is not carried here.

import SwiftUI

// The pieces FERB uses to show what it found: files, facts, tables, numbers, small charts, links, code.
// All of them are neutral. A chart is ivory on glass; the one row or bar that is the answer is brighter, the rest
// dimmer. Lamp colours stay reserved for state and always come with an icon and words.

// MARK: Files

/// What kind of file something is, read from its name. The four office kinds keep the colours people already know
/// them by; everything else is neutral with its own glyph.
public enum FileKind: Equatable {
    case pdf, document, sheet, slides, image, video, audio, code, archive, text, other

    public init(name: String) {
        switch (name.split(separator: ".").last.map(String.init) ?? "").lowercased() {
        case "pdf": self = .pdf
        case "doc", "docx", "pages", "rtf": self = .document
        case "xls", "xlsx", "csv", "numbers", "tsv": self = .sheet
        case "ppt", "pptx", "key": self = .slides
        case "png", "jpg", "jpeg", "heic", "gif", "webp", "svg", "tiff": self = .image
        case "mp4", "mov", "m4v", "webm": self = .video
        case "mp3", "m4a", "wav", "aiff", "caf": self = .audio
        case "swift", "py", "js", "ts", "json", "html", "css", "sh", "toml", "yaml", "yml", "rs", "go", "c", "h": self = .code
        case "zip", "dmg", "tar", "gz", "pkg": self = .archive
        case "txt", "md": self = .text
        default: self = .other
        }
    }

    /// The badge colour. Darker and duller than the lamps, so a red PDF badge is never mistaken for a failure.
    public var badge: Color {
        switch self {
        case .pdf: return Color(hex: 0xC8493F)
        case .document: return Color(hex: 0x3F78C9)
        case .sheet: return Color(hex: 0x2F8F5B)
        case .slides: return Color(hex: 0xC97A2B)
        default: return Color(hex: 0x5B5852)
        }
    }

    var glyph: String? {
        switch self {
        case .image: return "photo"
        case .video: return "play.fill"
        case .audio: return "waveform"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .archive: return "shippingbox"
        default: return nil
        }
    }
}

/// A sheet of paper with a folded corner, a hint of what is on it, and a badge naming its type.
public struct FileIcon: View {
    public let name: String
    public var width: CGFloat = 34

    public init(name: String, width: CGFloat = 34) {
        self.name = name
        self.width = width
    }

    public var body: some View {
        let kind = FileKind(name: name)
        let height = width * 1.26
        let fold = width * 0.3
        ZStack(alignment: .topLeading) {
            Paper(fold: fold).fill(Color(hex: 0xF7F5F0))
            // The folded corner.
            Path { p in
                p.move(to: CGPoint(x: width - fold, y: 0))
                p.addLine(to: CGPoint(x: width - fold, y: fold - width * 0.06))
                p.addQuadCurve(to: CGPoint(x: width - fold + width * 0.06, y: fold), control: CGPoint(x: width - fold, y: fold))
                p.addLine(to: CGPoint(x: width, y: fold))
                p.closeSubpath()
            }
            .fill(Color(hex: 0xD9D5CC))
            content(kind).frame(width: width, height: height * 0.62).offset(y: height * 0.1)
            Text(label(kind))
                .font(.system(size: max(6, width * 0.21), weight: .bold))
                .foregroundStyle(.white)
                .padding(.horizontal, width * 0.09)
                .frame(height: width * 0.33)
                .background(kind.badge, in: RoundedRectangle(cornerRadius: width * 0.07, style: .continuous))
                .offset(x: -width * 0.1, y: height * 0.6)
        }
        .frame(width: width, height: height)
        .shadow(color: .black.opacity(0.3), radius: width * 0.08, y: width * 0.04)
        .accessibilityLabel(Text(label(kind) + " file"))
    }

    private func label(_ kind: FileKind) -> String {
        let ext = (name.split(separator: ".").last.map(String.init) ?? "").uppercased()
        return name.contains(".") && !ext.isEmpty && ext.count <= 5 ? ext : "FILE"
    }

    @ViewBuilder private func content(_ kind: FileKind) -> some View {
        let ink = Color(hex: 0x1D1D1F)
        if let glyph = kind.glyph {
            Image(systemName: glyph).font(.system(size: width * 0.34, weight: .medium)).foregroundStyle(ink.opacity(0.4))
        } else if kind == .sheet {
            // A grid, as on a spreadsheet.
            VStack(spacing: width * 0.05) {
                ForEach(0..<3, id: \.self) { _ in
                    HStack(spacing: width * 0.05) {
                        ForEach(0..<3, id: \.self) { _ in
                            RoundedRectangle(cornerRadius: 1).fill(ink.opacity(0.2)).frame(width: width * 0.17, height: width * 0.1)
                        }
                    }
                }
            }
        } else if kind == .slides {
            RoundedRectangle(cornerRadius: width * 0.05).strokeBorder(ink.opacity(0.3), lineWidth: max(1, width * 0.035))
                .frame(width: width * 0.6, height: width * 0.4)
        } else {
            // Lines of text.
            VStack(alignment: .leading, spacing: width * 0.065) {
                RoundedRectangle(cornerRadius: 1).fill(ink.opacity(0.75)).frame(width: width * 0.38, height: max(1.5, width * 0.06))
                ForEach(0..<3, id: \.self) { line in
                    RoundedRectangle(cornerRadius: 1).fill(ink.opacity(0.22))
                        .frame(width: width * (line == 2 ? 0.34 : 0.56), height: max(1, width * 0.04))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.leading, width * 0.18)
        }
    }

    private struct Paper: Shape {
        let fold: CGFloat
        func path(in r: CGRect) -> Path {
            let radius = r.width * 0.11
            var p = Path()
            p.move(to: CGPoint(x: radius, y: 0))
            p.addLine(to: CGPoint(x: r.maxX - fold, y: 0))
            p.addLine(to: CGPoint(x: r.maxX, y: fold))
            p.addLine(to: CGPoint(x: r.maxX, y: r.maxY - radius))
            p.addQuadCurve(to: CGPoint(x: r.maxX - radius, y: r.maxY), control: CGPoint(x: r.maxX, y: r.maxY))
            p.addLine(to: CGPoint(x: radius, y: r.maxY))
            p.addQuadCurve(to: CGPoint(x: 0, y: r.maxY - radius), control: CGPoint(x: 0, y: r.maxY))
            p.addLine(to: CGPoint(x: 0, y: radius))
            p.addQuadCurve(to: CGPoint(x: radius, y: 0), control: CGPoint(x: 0, y: 0))
            p.closeSubpath()
            return p
        }
    }
}
