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
    let note: String?
    let correction: String?
    let lines: [String]
    init(_ source: String, size: CGFloat = 12, note: String? = nil, correction: String? = nil, lines: [String] = []) {
        self.source = source; self.baseSize = size; self.note = note; self.correction = correction; self.lines = lines
    }

    var body: some View {
        NativeMessageText(MessageTextAttributes.joined([
            MarkdownTextAttributes.make(source, size: baseSize),
            MarkdownTextAttributes.annotations(note: note, correction: correction, lines: lines)
        ]))
    }
}

/// Paragraphs, lists, code and tables share one text storage, so native selection
/// and Command-A/Command-C work across the entire message. Parsing stays inert.
enum MarkdownTextAttributes {
    static func annotations(note: String?, correction: String?, lines: [String]) -> NSAttributedString {
        var parts: [NSAttributedString] = []
        for (prefix, value, color) in [("Couldn't confirm: ", note, NSColor(Palette.amber)),
                                       ("Correction: ", correction, NSColor.labelColor)] {
            guard let value else { continue }
            let label = NSMutableAttributedString(attributedString: plain(prefix, size: 11.5, bold: true))
            label.addAttribute(.foregroundColor, value: color, range: NSRange(location: 0, length: label.length))
            label.append(plain(value, size: 11.5, secondary: true))
            parts.append(label)
        }
        parts += lines.map { plain($0, size: 11, secondary: true) }
        return MessageTextAttributes.joined(parts)
    }

    static func make(_ source: String, size: CGFloat) -> NSAttributedString {
        let result = NSMutableAttributedString(string: "")
        for block in MarkdownDocument(source).blocks {
            if result.length > 0 {
                // The smaller blank line keeps the previous nine-point block rhythm.
                result.append(plain("\n", size: size))
                result.append(plain("\n", size: 7))
            }
            result.append(render(block, size: size))
        }
        return result
    }

    private static func plain(_ source: String, size: CGFloat, secondary: Bool = false,
                              markdown: Bool = false, bold: Bool = false, mono: Bool = false) -> NSAttributedString {
        MessageTextAttributes.make(source, size: size, secondary: secondary, markdown: markdown,
                                   bold: bold, monospaced: mono, alignment: .left)
    }

    private static func render(_ block: MarkdownDocument.Block, size: CGFloat) -> NSAttributedString {
        switch block {
        case .paragraph(let text): return plain(text, size: size, markdown: true)
        case .heading(let text, let level):
            return plain(text, size: level == 1 ? 17 : (level == 2 ? 15 : 13), markdown: true, bold: true)
        case .listItem(let text, let marker, let level):
            let result = NSMutableAttributedString(attributedString: plain(marker + "\t", size: size, secondary: true))
            result.append(plain(text, size: size, markdown: true))
            let paragraph = NSMutableParagraphStyle()
            paragraph.firstLineHeadIndent = CGFloat(level * 12)
            paragraph.headIndent = paragraph.firstLineHeadIndent + 20
            paragraph.tabStops = [NSTextTab(textAlignment: .left, location: paragraph.headIndent)]
            result.addAttribute(.paragraphStyle, value: paragraph, range: NSRange(location: 0, length: result.length))
            return result
        case .quote(let text):
            return inset(plain(text, size: size, secondary: true, markdown: true), fill: .white.withAlphaComponent(0.035))
        case .code(let text, let language):
            let result = NSMutableAttributedString(string: "")
            if !language.isEmpty { result.append(plain(language + "\n", size: 9, secondary: true)) }
            result.append(plain(text, size: 11, mono: true))
            return inset(result, fill: .black.withAlphaComponent(0.25))
        case .table(let headers, let rows, let alignments):
            return table(headers: headers, rows: rows, alignments: alignments, size: size)
        case .divider:
            return plain("────────────────────────", size: 9, secondary: true)
        }
    }

    private static func inset(_ text: NSAttributedString, fill: NSColor) -> NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        paragraph.firstLineHeadIndent = 6
        paragraph.headIndent = 6
        paragraph.tailIndent = -6
        let result = NSMutableAttributedString(attributedString: text)
        result.addAttributes([.paragraphStyle: paragraph, .messageBlockFill: fill], range: NSRange(location: 0, length: result.length))
        return result
    }

    private static func table(headers: [String], rows: [[String]], alignments: [MarkdownDocument.Alignment],
                              size: CGFloat) -> NSAttributedString {
        let table = NSTextTable()
        table.numberOfColumns = headers.count
        table.setContentWidth(100, type: .percentageValueType)
        table.layoutAlgorithm = .fixedLayoutAlgorithm
        table.collapsesBorders = true
        let result = NSMutableAttributedString(string: "")
        for (rowIndex, row) in ([headers] + rows).enumerated() {
            for column in headers.indices {
                let block = NSTextTableBlock(table: table, startingRow: rowIndex, rowSpan: 1, startingColumn: column, columnSpan: 1)
                block.setContentWidth(100 / CGFloat(headers.count), type: .percentageValueType)
                block.setWidth(6, type: .absoluteValueType, for: .padding)
                let paragraph = NSMutableParagraphStyle()
                paragraph.textBlocks = [block]
                paragraph.alignment = alignments[column] == .trailing ? .right : (alignments[column] == .center ? .center : .left)
                let value = column < row.count ? row[column] : ""
                let cell = NSMutableAttributedString(attributedString: plain(value + "\n", size: size, markdown: true, bold: rowIndex == 0))
                cell.addAttribute(.paragraphStyle, value: paragraph, range: NSRange(location: 0, length: cell.length))
                result.append(cell)
            }
        }
        result.addAttribute(.messageBlockFill, value: NSColor.black.withAlphaComponent(0.12),
                            range: NSRange(location: 0, length: result.length))
        return result
    }
}
