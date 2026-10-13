import SwiftUI
import NotchCore

/// Text and native layout only: no web view, scripts, remote image loads, or automatic link visits.
struct MarkdownContent: View, Equatable {
    let source: String
    init(_ source: String) { self.source = source }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            ForEach(Array(MarkdownDocument(source).blocks.enumerated()), id: \.offset) { _, block in
                content(block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
        .environment(\.openURL, OpenURLAction { url in
            MarkdownDocument.safeLink(url) ? .systemAction : .discarded
        })
    }

    @ViewBuilder private func content(_ block: MarkdownDocument.Block) -> some View {
        switch block {
        case .paragraph(let text): inline(text)
        case .heading(let text, let level):
            inline(text).font(.system(size: level == 1 ? 17 : (level == 2 ? 15 : 13), weight: .semibold))
        case .listItem(let text, let marker, let level):
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                Text(marker).foregroundStyle(Palette.secondary)
                inline(text).frame(maxWidth: .infinity, alignment: .leading)
            }.padding(.leading, CGFloat(level * 12))
        case .quote(let text):
            inline(text).foregroundStyle(Palette.secondary).padding(.horizontal, 10).padding(.vertical, 5)
                .background(.white.opacity(0.035), in: RoundedRectangle(cornerRadius: 5))
        case .code(let text, let language):
            VStack(alignment: .leading, spacing: 4) {
                if !language.isEmpty { Text(language).font(.system(size: 9)).foregroundStyle(Palette.secondary) }
                ScrollView(.horizontal) { Text(text).font(.system(size: 11, design: .monospaced)).fixedSize(horizontal: true, vertical: false) }
            }.padding(8).background(.black.opacity(0.25), in: RoundedRectangle(cornerRadius: 7))
        case .table(let headers, let rows, let alignments):
            ScrollView(.horizontal) {
                Grid(alignment: .topLeading, horizontalSpacing: 12, verticalSpacing: 8) {
                    GridRow {
                        ForEach(headers.indices, id: \.self) { column in
                            tableCell(headers[column], alignment: alignments[column]).fontWeight(.semibold)
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

    private func tableCell(_ value: String, alignment: MarkdownDocument.Alignment) -> some View {
        inline(value).frame(width: 145, alignment: alignment == .trailing ? .trailing : (alignment == .center ? .center : .leading))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func inline(_ value: String) -> Text {
        var attributed = MarkdownDocument.inline(value)
        for run in attributed.runs where run.inlinePresentationIntent?.contains(.code) == true {
            attributed[run.range].font = .system(size: 11, design: .monospaced)
            attributed[run.range].backgroundColor = .white.opacity(0.06)
        }
        return Text(attributed)
    }
}
