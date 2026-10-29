import AppKit
import SwiftUI
import NotchCore

private struct SelectableTextGenerationKey: EnvironmentKey {
    static let defaultValue = 0
}
extension EnvironmentValues {
    var selectableTextGeneration: Int {
        get { self[SelectableTextGenerationKey.self] }
        set { self[SelectableTextGenerationKey.self] = newValue }
    }
}

/// Text and native layout only: no web view, scripts, remote image loads, or automatic link visits.
struct MarkdownContent: View, Equatable {
    let source: String
    let baseSize: CGFloat
    static func == (lhs: Self, rhs: Self) -> Bool { lhs.source == rhs.source && lhs.baseSize == rhs.baseSize }
    init(_ source: String, size: CGFloat = 12) { self.source = source; self.baseSize = size }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            ForEach(Array(MarkdownDocument(source).blocks.enumerated()), id: \.offset) { _, block in
                content(block)
            }
        }

    }

    @ViewBuilder private func content(_ block: MarkdownDocument.Block) -> some View {
        switch block {
        case .paragraph(let text):
            inline(text)
        case .heading(let text, let level):
            NativeMessageText(text, size: level == 1 ? 17 : (level == 2 ? 15 : 13), markdown: true, bold: true)
        case .listItem(let text, let marker, let level):
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                Text(marker).foregroundStyle(Palette.secondary)
                inline(text)
                    .alignmentGuide(.firstTextBaseline) { _ in NSFont.systemFont(ofSize: baseSize).ascender }
            }.padding(.leading, CGFloat(level * 12))
        case .quote(let text):
            NativeMessageText(text, size: baseSize, secondary: true, markdown: true).padding(.horizontal, 10).padding(.vertical, 5)
                .background(.white.opacity(0.035), in: RoundedRectangle(cornerRadius: 5))
        case .code(let text, let language):
            VStack(alignment: .leading, spacing: 4) {
                if !language.isEmpty { Text(language).font(.system(size: 9)).foregroundStyle(Palette.secondary) }
                ScrollView(.horizontal) {
                    NativeMessageText(text, size: 11, monospaced: true)
                        .fixedSize(horizontal: true, vertical: false)
                }
            }.padding(8).background(.black.opacity(0.25), in: RoundedRectangle(cornerRadius: 7))
        case .table(let headers, let rows, let alignments):
            ScrollView(.horizontal) {
                Grid(alignment: .topLeading, horizontalSpacing: 12, verticalSpacing: 8) {
                    GridRow {
                        ForEach(headers.indices, id: \.self) { column in
                            tableCell(headers[column], alignment: alignments[column], bold: true)
                        }
                    }.padding(.bottom, 3)
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(headers.indices, id: \.self) { column in
                                tableCell(column < row.count ? row[column] : "", alignment: alignments[column])
                            }
                        }
                    }
                }.padding(8)
            }.background(.black.opacity(0.12), in: RoundedRectangle(cornerRadius: 7))
        case .divider: Divider().opacity(0.35)
        }
    }

    private func tableCell(_ value: String, alignment: MarkdownDocument.Alignment, bold: Bool = false) -> some View {
        NativeMessageText(value, size: baseSize, markdown: true, bold: bold,
                          alignment: alignment == .trailing ? .right : (alignment == .center ? .center : .left))
            .frame(width: 145, alignment: alignment == .trailing ? .trailing : (alignment == .center ? .center : .leading))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func inline(_ value: String) -> NativeMessageText {
        NativeMessageText(value, size: baseSize, markdown: true)
    }
}
