import SwiftUI

// MARK: The memory import's live card (kb MEM-456; the owner's words, MEM-455)

// Not from the lab: while FERB brings in what the owner said in other assistants (Claude Code, Codex, ChatGPT, Claude),
// one card at the end of today's chat says how far it is, from the moment the import starts until its notice arrives.
// The sibling of PartsCard: FERB's surface, the same radius, padding, caption and inks. Every word is the Mac's (the
// daemon composes them); this draws only what it is handed.

/// One source on the card, in the Mac's words (the phone library's ImportRow, as plain values).
public struct MemoryImportRow: Equatable, Sendable, Identifiable {
    /// Stable while the row lives: the bar moves only within one key.
    public let key: String
    /// The source's name, shown only when the card has more than one row.
    public let name: String
    public let line: String
    public let detail: String
    public let done: Int
    /// No bar while 0.
    public let total: Int
    /// Beside the bar ("33 of 40").
    public let count: String
    public let foundLine: String
    /// A gap, or a try that didn't go through, with what happens next: set apart in amber.
    public let note: String

    public var id: String { key }

    public init(key: String, name: String = "", line: String, detail: String = "", done: Int = 0, total: Int = 0, count: String = "",
                foundLine: String = "", note: String = "") {
        self.key = key; self.name = name; self.line = line; self.detail = detail; self.done = done; self.total = total; self.count = count
        self.foundLine = foundLine; self.note = note
    }

    /// How full the bar is, 0 to 1.
    public var fraction: Double { total > 0 ? min(1, max(0, Double(done) / Double(total))) : 0 }
}

/// The card: its title as the caption, then each source: its name (only with more than one), its line, a slim bar with
/// its count while there is a total, what was found so far, a quieter detail, and a note set apart in amber. Nothing in
/// it moves but the bar's fill, which eases to its new length when `done` changes (snaps under Reduce Motion).
public struct MemoryImportCard: View {
    let title: String
    let rows: [MemoryImportRow]
    @Environment(\.onFerb) private var onFerb

    public init(title: String, rows: [MemoryImportRow]) {
        self.title = title
        self.rows = rows
    }

    public var body: some View {
        let ink = CardInk(onFerb)
        VStack(alignment: .leading, spacing: 0) {
            // PartsCard's caption
            Text(title).type(13, .semibold, ink.secondary).lineLimit(2)
                .padding(.horizontal, 14).padding(.top, 6).padding(.bottom, 2)
                .accessibilityAddTraits(.isHeader)
            ForEach(Array(rows.enumerated()), id: \.element.key) { i, r in
                if i > 0 { Rectangle().fill(ink.line).frame(height: 1).padding(.horizontal, 14) }
                MemoryImportRowView(row: r, named: rows.count > 1, ink: ink)
            }
        }
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ink.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
}

private struct MemoryImportRowView: View {
    let row: MemoryImportRow
    let named: Bool
    let ink: CardInk

    // Android's ImportCard.kt numbers (one design on both phones, PHN-416)
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if named, !row.name.isEmpty {
                Text(row.name).type(13, .semibold, ink.secondary).lineLimit(1)
            }
            Text(row.line).type(15, .semibold, ink.primary).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
            if row.total > 0 {
                HStack(spacing: 10) {
                    ImportBar(fraction: row.fraction, onFerb: ink.onFerb).accessibilityHidden(true)
                    // figures of one width: the count keeps its place as it climbs
                    if !row.count.isEmpty {
                        Text(row.count).type(13, .medium, ink.secondary).monospacedDigit().lineLimit(1).fixedSize()
                    }
                }
                .padding(.top, 6).padding(.bottom, 2)
            }
            if !row.foundLine.isEmpty {
                Text(row.foundLine).type(13.5, .medium, ink.secondary)
            }
            if !row.detail.isEmpty {
                Text(row.detail).type(13.5, .regular, ink.faint).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
            }
            if !row.note.isEmpty {
                // set apart in the amber of the "Not done" lines, on a faint wash of it, behind a small dot
                HStack(alignment: .top, spacing: 8) {
                    Circle().fill(ink.amberText).frame(width: 6, height: 6).padding(.top, 6)
                    Text(row.note).type(13.5, .medium, ink.amberText).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 10).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Ink.amber.opacity(ink.onFerb ? 0.13 : 0.16), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .padding(.top, 4)
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

/// A slim capsule track with a soft fill from the leading edge.
struct ImportBar: View {
    let fraction: Double
    let onFerb: Bool
    @Environment(\.accessibilityReduceMotion) private var reduce

    var body: some View {
        ZStack(alignment: .leading) {
            Capsule().fill(onFerb ? Color.white.opacity(0.14) : Ink.black.opacity(0.07))
            BarFill(fraction: fraction).fill(onFerb ? Color.white.opacity(0.9) : Ink.black.opacity(0.82))
        }
        .frame(height: 6)
        .frame(maxWidth: .infinity)
        // the fill eases to its new length; under Reduce Motion it snaps (nothing moves, kb PHN-624)
        .animation(reduce ? nil : FerbMotion.progress.animation, value: fraction)
    }
}

/// The bar's fill: a capsule `fraction` of the width (at least round), animatable by its fraction so it grows smoothly.
struct BarFill: Shape {
    var fraction: Double

    var animatableData: Double {
        get { fraction }
        set { fraction = newValue }
    }

    func path(in rect: CGRect) -> Path {
        let f = min(1, max(0, fraction))
        guard f > 0 else { return Path() }
        let w = max(rect.height, rect.width * f)
        return Capsule(style: .continuous).path(in: CGRect(x: rect.minX, y: rect.minY, width: w, height: rect.height))
    }
}
