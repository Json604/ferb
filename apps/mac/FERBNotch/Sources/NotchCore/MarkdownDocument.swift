import Foundation

/// Native, inert Markdown presentation: block layout plus Foundation inline parsing.
/// HTML stays text; images never create network requests or executable web content.
public struct MarkdownDocument {
    public enum Alignment: Equatable { case leading, center, trailing }
    public enum Block: Equatable {
        case paragraph(String)
        case heading(String, Int)
        case listItem(String, String, Int)
        case quote(String)
        case code(String, String)
        case table([String], [[String]], [Alignment])
        case divider
    }
    private static let linkDetector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
    private static let listExpression = try? NSRegularExpression(pattern: "^(\\s*)([-+*]|[0-9]+[.)])\\s+(.+)$")
    public let blocks: [Block]

    public init(_ source: String) {
        let lines = source.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        var parsed: [Block] = []
        var paragraph: [String] = []
        func flush() {
            if !paragraph.isEmpty { parsed.append(.paragraph(paragraph.joined(separator: "\n"))); paragraph = [] }
        }
        var index = 0
        while index < lines.count {
            let line = lines[index]
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty { flush(); index += 1; continue }
            if let fence = Self.fence(trimmed) {
                flush()
                let language = String(trimmed.dropFirst(fence.count)).trimmingCharacters(in: .whitespaces)
                var code: [String] = []
                index += 1
                while index < lines.count {
                    let closing = lines[index].trimmingCharacters(in: .whitespaces)
                    if closing.hasPrefix(fence), closing.dropFirst(fence.count).allSatisfy({ $0 == fence.first! || $0.isWhitespace }) {
                        index += 1; break
                    }
                    code.append(lines[index]); index += 1
                }
                parsed.append(.code(code.joined(separator: "\n"), language)); continue
            }
            if index + 1 < lines.count, line.contains("|"),
               let alignment = Self.tableAlignment(lines[index + 1]), alignment.count == Self.cells(line).count {
                flush()
                let header = Self.cells(line)
                var rows: [[String]] = []
                index += 2
                while index < lines.count && lines[index].contains("|") && !lines[index].trimmingCharacters(in: .whitespaces).isEmpty {
                    let cells = Self.cells(lines[index])
                    rows.append(cells + Array(repeating: "", count: max(0, header.count - cells.count)))
                    index += 1
                }
                parsed.append(.table(header, rows, alignment)); continue
            }
            let hashes = trimmed.prefix(while: { $0 == "#" }).count
            if (1...6).contains(hashes), trimmed.dropFirst(hashes).first?.isWhitespace == true {
                flush(); parsed.append(.heading(String(trimmed.dropFirst(hashes)).trimmingCharacters(in: .whitespaces), hashes))
            } else if let item = Self.listItem(line) {
                flush(); parsed.append(item)
            } else if trimmed.hasPrefix(">") {
                flush(); parsed.append(.quote(String(trimmed.dropFirst()).trimmingCharacters(in: .whitespaces)))
            } else if ["---", "***", "___"].contains(trimmed) {
                flush(); parsed.append(.divider)
            } else { paragraph.append(line) }
            index += 1
        }
        flush()
        blocks = parsed
    }

    public static func inline(_ source: String) -> AttributedString {
        var value = (try? AttributedString(markdown: source, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(source)
        let plain = String(value.characters)
        if let detector = linkDetector {
            for match in detector.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)) {
                guard let url = match.url, safeLink(url), let range = Range(match.range, in: plain),
                      let lower = AttributedString.Index(range.lowerBound, within: value),
                      let upper = AttributedString.Index(range.upperBound, within: value) else { continue }
                let attributedRange = lower..<upper
                if value[attributedRange].link == nil, value[attributedRange].inlinePresentationIntent?.contains(.code) != true {
                    value[attributedRange].link = url
                }
            }
        }
        for run in value.runs {
            if let url = run.link, !safeLink(url) { value[run.range].link = nil }
        }
        return value
    }

    public static func safeLink(_ url: URL) -> Bool {
        ["https", "http", "mailto"].contains(url.scheme?.lowercased() ?? "")
    }

    private static func fence(_ line: String) -> String? {
        guard let character = line.first, character == "`" || character == "~" else { return nil }
        let count = line.prefix(while: { $0 == character }).count
        return count >= 3 ? String(repeating: String(character), count: count) : nil
    }

    private static func listItem(_ line: String) -> Block? {
        guard let expression = listExpression,
              let match = expression.firstMatch(in: line, range: NSRange(line.startIndex..., in: line)) else { return nil }
        let text = line as NSString
        let marker = text.substring(with: match.range(at: 2))
        return .listItem(text.substring(with: match.range(at: 3)), ["-", "+", "*"].contains(marker) ? "•" : marker,
                         text.substring(with: match.range(at: 1)).count / 2)
    }

    private static func tableAlignment(_ line: String) -> [Alignment]? {
        let cells = cells(line)
        guard !cells.isEmpty, cells.allSatisfy({ cell in
            let stripped = cell.trimmingCharacters(in: CharacterSet(charactersIn: ":"))
            return stripped.count >= 3 && stripped.allSatisfy { $0 == "-" }
        }) else { return nil }
        return cells.map { $0.hasSuffix(":") ? ($0.hasPrefix(":") ? .center : .trailing) : .leading }
    }

    private static func cells(_ line: String) -> [String] {
        let characters = Array(line)
        var values: [String] = [], cell = "", index = 0, codeFence = 0
        while index < characters.count {
            let character = characters[index]
            if character == "\\", index + 1 < characters.count {
                cell.append(character); cell.append(characters[index + 1]); index += 2; continue
            }
            if character == "`" {
                var count = 1
                while index + count < characters.count && characters[index + count] == "`" { count += 1 }
                if codeFence == 0 { codeFence = count } else if codeFence == count { codeFence = 0 }
                cell += String(repeating: "`", count: count); index += count; continue
            }
            if character == "|", codeFence == 0 { values.append(cell.trimmingCharacters(in: .whitespaces)); cell = "" }
            else { cell.append(character) }
            index += 1
        }
        values.append(cell.trimmingCharacters(in: .whitespaces))
        if values.first == "" { values.removeFirst() }
        if values.last == "" { values.removeLast() }
        return values
    }
}
