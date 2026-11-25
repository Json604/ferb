import SwiftUI
import NotchCore

/// FERB's review of what it learned about the owner (kb MEM-460..457, MEM-485, MEM-486), drawn the same in the chat's
/// card and on the Memory page's To check: the list's own buttons on top (Approve all, "Approve 2" while some lines are
/// unticked, Decline all), the line above the items, then each part's question and the line under it, its groups by
/// topic, and every line with its tick. A tap on a line unticks it (the exception); Approve all gives every ticked line
/// its part's yes and every unticked one its no. Every line shows: Approve all answers only what he can see. The reply
/// is the new list (answered lines leave it).
struct MemoryReviewList: View {
    @ObservedObject var model: MemoryReviewModel
    /// The chat's card is narrower and smaller than the Memory page; its question already says how to untick.
    var compact = true

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 12 : 16) {
            if !model.review.parts.isEmpty {
                if !compact && !model.review.hint.isEmpty {
                    Text(model.review.hint).font(.system(size: 11.5)).foregroundStyle(Palette.secondary)
                }
                AllButtons(model: model)
            }
            ForEach(model.review.parts) { part in
                PartView(model: model, part: part, compact: compact)
            }
            if let problem = model.problem {
                Text(problem).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .task { await model.load() }
    }

    /// Approve all (the card's main button) and Decline all, for the whole list.
    private struct AllButtons: View {
        @ObservedObject var model: MemoryReviewModel

        var body: some View {
            HStack(spacing: 6) {
                if let approve = model.approveLabel {
                    Button(approve) { Task { await model.answerAll(approve: true) } }
                        .buttonStyle(FerbButtonStyle(primary: true))
                        .accessibilityIdentifier("review-approve")
                }
                if model.review.all.count > 1 {
                    Button(model.review.all[1].label) { Task { await model.answerAll(approve: false) } }
                        .buttonStyle(FerbButtonStyle(quiet: true))
                        .accessibilityIdentifier("review-decline")
                }
            }
            .disabled(model.sendingAll)
        }
    }

    private struct PartView: View {
        @ObservedObject var model: MemoryReviewModel
        let part: MemoryReview.Part
        let compact: Bool

        var body: some View {
            VStack(alignment: .leading, spacing: compact ? 7 : 9) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(part.title).font(.system(size: compact ? 12.5 : 14, weight: .semibold))
                        .foregroundStyle(Palette.ink)
                    if !part.hint.isEmpty {
                        Text(part.hint).font(.system(size: compact ? 10.5 : 11.5)).foregroundStyle(Palette.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                ForEach(Array(part.groups.enumerated()), id: \.offset) { _, group in
                    VStack(alignment: .leading, spacing: compact ? 6 : 8) {
                        if !group.title.isEmpty {
                            Text(group.title.uppercased()).tracking(1.1)
                                .font(.system(size: 9.5, weight: .semibold)).foregroundStyle(Palette.secondary)
                        }
                        ForEach(group.items) { item in
                            ItemRow(model: model, item: item, compact: compact)
                        }
                    }
                }
            }
        }
    }

    /// One line: its tick, its words and the small note under them. The whole row is the tick's button.
    private struct ItemRow: View {
        @ObservedObject var model: MemoryReviewModel
        let item: MemoryReview.Item
        let compact: Bool

        var body: some View {
            let ticked = model.isTicked(item.id)
            Button { model.toggle(item.id) } label: {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: ticked ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: compact ? 13 : 14))
                        .foregroundStyle(ticked ? Palette.ink : Palette.secondary)
                        .padding(.top, 1)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(item.text).font(.system(size: compact ? 12 : 13))
                            .foregroundStyle(ticked ? Palette.ink : Palette.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .multilineTextAlignment(.leading)
                        if !item.note.isEmpty {
                            Text(item.note).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                        }
                    }
                    Spacer(minLength: 0)
                }
                .padding(compact ? 8 : 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.white.opacity(ticked ? 0.045 : 0.02),
                            in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 10))
            }
            .buttonStyle(.plain)
            .pointingHand("button")
            .disabled(model.sendingAll)
            .accessibilityLabel(Text(item.text))
            .accessibilityValue(Text(ticked ? "Ticked" : "Not ticked"))
            .accessibilityAddTraits(ticked ? .isSelected : [])
        }
    }
}

/// The Memory page's copy of the review (MEM-460), shown while anything is left to answer; an answer there reloads the
/// page (an approved item joins Always on).
struct MemoryReviewSection: View {
    @ObservedObject var model: MemoryReviewModel
    var changed: () async -> Void = {}

    var body: some View {
        Group {
            if model.review.count > 0 {
                WindowSectionLabel(title: "To check", count: model.review.count)
                WindowCard { MemoryReviewList(model: model, compact: false) }
            }
        }
        .task { await model.load() }
        .onChange(of: model.review) { _, _ in Task { await changed() } }
    }
}
