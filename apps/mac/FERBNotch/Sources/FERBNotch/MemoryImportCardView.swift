import SwiftUI
import NotchCore

/// The memory import's live card (MEM-455, MEM-456; owner 2026-11-22: "does ferb not show the progress on memory
/// transfer it is gettigb??? i think it should show in a good ass UI"): at the end of today's chat and at the top of
/// the Memory page while an import goes on, in the daemon's words. Calm: the bar is the only thing that moves, gliding
/// to each new part (at once under Reduce Motion); no spinner. It leaves as the import's one notice comes in.
struct MemoryImportCardView: View {
    let card: MemoryImportCard
    /// The Memory page draws it on its own card background; the chat on FERB's card surface.
    var onPage = false

    /// The transcript's row id for the card (one at a time).
    static let anchor = "memory-import"

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "tray.and.arrow.down")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(Palette.secondary)
                    .accessibilityHidden(true)
                Text(card.title)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(Palette.secondary)
                    .lineLimit(2).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 12).padding(.top, 10).padding(.bottom, 7)
            ForEach(Array(card.rows.enumerated()), id: \.element.id) { index, row in
                if index > 0 { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
                MemoryImportRowView(row: row, named: card.namesRows)
                    .padding(.horizontal, 12)
                    .padding(.top, index > 0 ? 9 : 0).padding(.bottom, 10)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background {
            if !onPage {
                RoundedRectangle(cornerRadius: 15, style: .continuous).fill(.white.opacity(0.10))
            }
        }
        .accessibilityElement(children: .contain)
    }

    /// A height for the card before it is measured (the transcript list's estimate, APP-815).
    static func estimate(_ card: MemoryImportCard) -> CGFloat {
        card.rows.reduce(CGFloat(32)) { sum, row in
            sum + 26 + (row.namesRow(card) ? 16 : 0) + (row.total > 0 ? 14 : 0) + (row.foundLine.isEmpty ? 0 : 16)
                + (row.detail.isEmpty ? 0 : 30) + (row.note.isEmpty ? 0 : 30)
        }
    }
}

private extension MemoryImportCard.Row {
    func namesRow(_ card: MemoryImportCard) -> Bool { card.namesRows && !name.isEmpty }
}

/// One source on the card: its name (with more than one), the stage line, the bar with "33 of 40", what was found so
/// far, a quieter detail, and a gap or a try that didn't go through in amber with what happens next.
struct MemoryImportRowView: View {
    let row: MemoryImportCard.Row
    let named: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            if named && !row.name.isEmpty {
                Text(row.name).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Palette.ink)
            }
            Text(row.line)
                .font(.system(size: 12.5)).foregroundStyle(Palette.ink)
                .fixedSize(horizontal: false, vertical: true)
            if let fraction = row.fraction {
                HStack(spacing: 9) {
                    MemoryImportBar(fraction: fraction)
                    Text(row.count)
                        .font(.system(size: 10.5).monospacedDigit()).foregroundStyle(Palette.secondary)
                        .fixedSize()
                }
                .padding(.top, 2)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(row.line)
                .accessibilityValue(row.count)
            }
            if !row.foundLine.isEmpty {
                Text(row.foundLine)
                    .font(.system(size: 11).monospacedDigit()).foregroundStyle(Palette.secondary)
                    .contentTransition(.numericText())
            }
            if !row.detail.isEmpty {
                Text(row.detail)
                    .font(.system(size: 11)).foregroundStyle(Palette.secondary.opacity(0.85))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !row.note.isEmpty {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Image(systemName: "exclamationmark.circle").font(.system(size: 10)).foregroundStyle(Palette.amber)
                        .accessibilityHidden(true)
                    Text(row.note)
                        .font(.system(size: 11)).foregroundStyle(Palette.amber)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.top, 1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The bar: a slim capsule track, its fill gliding to each new fraction (easing out, so it settles softly) or set at
/// once under Reduce Motion.
struct MemoryImportBar: View {
    let fraction: Double
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule().fill(.white.opacity(0.10))
                Capsule()
                    .fill(LinearGradient(colors: [.white.opacity(0.62), .white.opacity(0.92)],
                                         startPoint: .leading, endPoint: .trailing))
                    .frame(width: max(fraction > 0 ? 5 : 0, proxy.size.width * fraction))
                    .shadow(color: .white.opacity(0.18), radius: 3)
            }
        }
        .frame(height: 5)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.7), value: fraction)
    }
}

/// The owner's two switches for bringing in what other assistants know (MEM-459), in Settings → Privacy and on the
/// Memory page: each says what on and off do; on starts it (the websites ask which browser first, in the chat), off
/// stops it. FERB's answer to the last one turned shows under them.
struct MemoryImportSwitches: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(MemoryImportSwitch.allCases) { which in
                VStack(alignment: .leading, spacing: 4) {
                    KnownToggle(title: which.title, isOn: store.importSwitch(which),
                                known: store.settingsKnown && store.settings.has(which.setting))
                    Text(which.detail).font(.system(size: 12)).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            if let said = store.importSwitchSaid {
                Text(said).font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
