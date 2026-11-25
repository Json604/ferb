import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI

/// The review of what FERB learned (kb MEM-460, MEM-485, MEM-486), as FERB.app draws it: the list's own buttons on top
/// (Approve all, "Approve 2" while some lines are unticked, Decline all), the line above the items (`showHint`: the
/// sheet; the card's question already says it), then each part's title and line, its groups by topic, and every line
/// with its tick. A tap on a line unticks it (the exception); Approve all gives every ticked line its part's yes and
/// every unticked one its no. Every word is the Mac's. The list is read when it shows; the Mac's reply replaces it.
struct ReviewList: View {
    let model: AppModel
    let ink: CardInk
    var showHint = false
    @Environment(\.busy) private var busySet

    var body: some View {
        let busy = busySet.contains("review")
        let r = model.review
        VStack(alignment: .leading, spacing: 14) {
            if !r.parts.isEmpty {
                if showHint, let h = r.hint { Text(h).type(13.5, .regular, ink.secondary).fixedSize(horizontal: false, vertical: true) }
                ReviewFlow(spacing: 6) {
                    if let label = r.approveLabel {
                        chip(label, strong: true, busy: busy) { model.answerMemoryReviewAll(approve: true) }
                            .accessibilityIdentifier("review:approve")
                    }
                    if let label = r.declineLabel {
                        chip(label, strong: false, busy: busy) { model.answerMemoryReviewAll(approve: false) }
                            .accessibilityIdentifier("review:decline")
                    }
                }
            }
            ForEach(r.parts) { p in part(p, busy) }
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
                    ForEach(g.items) { item in self.item(item, busy) }
                }
            }
        }
    }

    /// One line: its tick, its words and the small note under them; the whole row is the tick's button.
    private func item(_ item: ReviewLayout.Item, _ busy: Bool) -> some View {
        let ticked = model.review.isTicked(item.id)
        return HStack(alignment: .top, spacing: 10) {
            ZStack {
                Circle().fill(ticked ? ink.primary : Color.clear)
                Circle().strokeBorder(ticked ? ink.primary : ink.secondary, lineWidth: 1.5)
                if ticked { Image(systemName: "checkmark").font(.system(size: 10, weight: .bold)).foregroundStyle(ink.card) }
            }
            .frame(width: 20, height: 20).padding(.top, 1)
            VStack(alignment: .leading, spacing: 4) {
                Text(item.text).type(14.5, .regular, ticked ? ink.primary : ink.secondary).fixedSize(horizontal: false, vertical: true)
                if let n = item.note { Text(n).type(12.5, .regular, ink.secondary).fixedSize(horizontal: false, vertical: true) }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ticked ? ink.fill : Color.clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(ticked ? Color.clear : ink.line, lineWidth: 1))
        .contentShape(RoundedRectangle(cornerRadius: 12))
        .pressable(busy ? nil : { model.review.toggle(item.id) })
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(ticked ? [.isButton, .isSelected] : .isButton)
        .accessibilityValue(Text(ticked ? "Ticked" : "Not ticked"))
        .accessibilityIdentifier("review-item:\(item.id)")
    }

    /// A small button: the Mac's words in a capsule, the main one in the card's inverse, the other on a light fill
    /// (Android's SmallButton).
    private func chip(_ label: String, strong: Bool, busy: Bool, _ act: @escaping () -> Void) -> some View {
        Text(label).type(13.5, .medium, (strong ? ink.card : ink.primary).opacity(busy ? 0.5 : 1))
            .lineLimit(1)
            .padding(.horizontal, 14)
            .frame(minHeight: 34)
            .background(strong ? ink.primary : ink.onFerb ? Color.white.opacity(0.14) : ink.line, in: Capsule())
            .pressable(busy ? nil : act)
    }
}

/// The review of what FERB learned in a sheet (MEM-485: the day's note's Review; the Mac's Memory page → To check):
/// the list, scrolling when long; "nothing to check" once all is answered.
struct ReviewSheet: View {
    let model: AppModel
    let insets: PhoneInsets
    let close: () -> Void
    /// False only for a still picture (a scroll view draws nothing there).
    var scrolls = true

    var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: close) {
            SheetTitle(L.s("review_sheet_title"), onClose: close)
            if scrolls {
                ScrollView { content }.frame(maxHeight: 560)
            } else {
                content
            }
        }
    }

    private var content: some View {
        Group {
            if model.review.review == nil {
                Text(L.s("review_sheet_reading")).type(15, .regular, Ink.detail)
            } else if model.review.parts.isEmpty {
                Text(L.s("review_sheet_empty")).type(15, .regular, Ink.detail)
            } else {
                ReviewList(model: model, ink: CardInk(false), showHint: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 12)
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
