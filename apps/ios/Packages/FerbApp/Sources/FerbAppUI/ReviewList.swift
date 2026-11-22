import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI

/// The review of what an import brought in (kb MEM-460), drawn in its question's card in place of the question's
/// detail: each part's title and line, its groups, each item with its note and its buttons (its own, or the part's
/// answers), a group's own buttons for all of its items when it has them, and the part's buttons for all of its items.
/// Every word is the Mac's; "Show all N" and "Show less" are the app's. The list is read when the card shows; a
/// button's answer brings the review back as it stands (answered items leave it); a failure leaves it as it was.
struct ReviewList: View {
    let model: AppModel
    let ink: CardInk
    @Environment(\.busy) private var busySet

    var body: some View {
        let busy = busySet.contains("review")
        let parts = model.review.parts
        VStack(alignment: .leading, spacing: 14) {
            ForEach(Array(parts.enumerated()), id: \.element.id) { i, p in
                if i > 0 { Rectangle().fill(ink.line).frame(height: 1) }
                part(p, busy)
            }
        }
        .padding(.top, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .onAppear { model.loadMemoryReview() }
    }

    private func part(_ p: ReviewLayout.Part, _ busy: Bool) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(p.title).type(15.5, .semibold, ink.primary).fixedSize(horizontal: false, vertical: true)
                if let h = p.hint { Text(h).type(13, .regular, ink.secondary).fixedSize(horizontal: false, vertical: true) }
            }
            ForEach(p.groups) { g in
                VStack(alignment: .leading, spacing: 6) {
                    if let t = g.title { Text(t).type(12.5, .medium, ink.secondary).padding(.top, 4) }
                    ForEach(g.items) { item in self.item(item, p, busy) }
                    // the group's own buttons for all of its items (Keep all / Remove all, MEM-464)
                    if !g.all.isEmpty {
                        allButtons(g.all, part: p.id, ids: g.ids, busy: busy, tag: g.id)
                    }
                    if let toggle = g.toggle {
                        Text(toggle).type(13.5, .medium, ink.primary).padding(.vertical, 4)
                            .pressable({ model.review.toggle(g.id) })
                            .accessibilityIdentifier("review-toggle:\(g.id)")
                    }
                }
            }
            // the part's, apart from the last group's own so each reads as its own
            if !p.all.isEmpty { allButtons(p.all, part: p.id, ids: p.ids, busy: busy, tag: p.id).padding(.top, 8) }
        }
    }

    private func item(_ item: ReviewLayout.Item, _ p: ReviewLayout.Part, _ busy: Bool) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 4) {
                Text(item.text).type(14.5, .regular, ink.primary).fixedSize(horizontal: false, vertical: true)
                if let n = item.note { Text(n).type(12.5, .regular, ink.secondary).fixedSize(horizontal: false, vertical: true) }
            }
            ReviewFlow(spacing: 6) {
                ForEach(Array(item.answers.enumerated()), id: \.offset) { _, b in
                    chip(b.label, strong: false, busy: busy) {
                        model.answerMemoryReview(part: p.id, itemIds: [item.id], answer: b.answer)
                    }
                    .accessibilityLabel(Text("\(b.label): \(item.text)"))
                    .accessibilityIdentifier("review-item:\(item.id):\(b.answer)")
                }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ink.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    /// Buttons answering every item of a part or a group (`ids`), the first in the card's inverse (Android's AllButtons).
    private func allButtons(_ buttons: [ReviewButton], part: String, ids: [String], busy: Bool, tag: String) -> some View {
        ReviewFlow(spacing: 6) {
            ForEach(Array(buttons.enumerated()), id: \.offset) { j, b in
                chip(b.label, strong: j == 0, busy: busy) {
                    model.answerMemoryReview(part: part, itemIds: ids, answer: b.answer)
                }
                .accessibilityIdentifier("review-all:\(tag):\(b.answer)")
            }
        }
        .padding(.top, 2)
    }

    /// A small button: the Mac's words in a capsule, the first of a part's "all" buttons in the card's inverse, the rest
    /// on a light fill (Android's SmallButton).
    private func chip(_ label: String, strong: Bool, busy: Bool, _ act: @escaping () -> Void) -> some View {
        Text(label).type(13.5, .medium, (strong ? ink.card : ink.primary).opacity(busy ? 0.5 : 1))
            .lineLimit(1)
            .padding(.horizontal, 12)
            .frame(minHeight: 32)
            .background(strong ? ink.primary : ink.onFerb ? Color.white.opacity(0.14) : ink.line, in: Capsule())
            .pressable(busy ? nil : act)
    }
}

/// Buttons left to right, wrapping to the next line.
struct ReviewFlow: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(subviews, width: proposal.width ?? .infinity)
        return CGSize(width: proposal.width ?? rows.map(\.width).max() ?? 0, height: rows.last.map { $0.y + $0.height } ?? 0)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for row in arrange(subviews, width: bounds.width) {
            var x = bounds.minX
            for i in row.items {
                let size = subviews[i].sizeThatFits(.unspecified)
                subviews[i].place(at: CGPoint(x: x, y: bounds.minY + row.y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
        }
    }

    private struct Row { var items: [Int] = []; var y: CGFloat = 0; var width: CGFloat = 0; var height: CGFloat = 0 }

    private func arrange(_ subviews: Subviews, width: CGFloat) -> [Row] {
        var rows = [Row()]
        for i in subviews.indices {
            let size = subviews[i].sizeThatFits(.unspecified)
            if !rows[rows.count - 1].items.isEmpty && rows[rows.count - 1].width + spacing + size.width > width {
                let last = rows[rows.count - 1]
                rows.append(Row(y: last.y + last.height + spacing))
            }
            rows[rows.count - 1].width += (rows[rows.count - 1].items.isEmpty ? 0 : spacing) + size.width
            rows[rows.count - 1].height = max(rows[rows.count - 1].height, size.height)
            rows[rows.count - 1].items.append(i)
        }
        return rows.filter { !$0.items.isEmpty }
    }
}
