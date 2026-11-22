import SwiftUI
import NotchCore

/// FERB's review of what an import brought in (kb MEM-460..457), drawn the same in the chat's question card and on the
/// Memory page: each part's question and the line under it, its groups, each item with its buttons, and the part's
/// buttons for all of them at once. An answered item leaves the list at once (the daemon's reply is the new list).
struct MemoryReviewList: View {
    @ObservedObject var model: MemoryReviewModel
    /// The chat's card is narrower and smaller than the Memory page.
    var compact = true

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 12 : 16) {
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
                    GroupView(model: model, part: part, group: group, compact: compact)
                }
                if part.ids.count > 1 && !part.all.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(part.all, id: \.self) { button in
                            Button(button.label) {
                                Task { await model.answer(part, ids: part.ids, with: button, all: true) }
                            }
                            .buttonStyle(FerbButtonStyle(primary: button == part.all.first))
                            .disabled(model.isSending(part, id: nil))
                        }
                    }
                    // apart from a group's own Keep all / Remove all just above (as both phones draw it)
                    .padding(.top, part.groups.last.map { $0.all.isEmpty } == false ? 8 : 2)
                }
            }
        }
    }

    private struct GroupView: View {
        @ObservedObject var model: MemoryReviewModel
        let part: MemoryReview.Part
        let group: MemoryReview.Group
        let compact: Bool
        @State private var open = false

        var body: some View {
            let shown = open ? group.items : Array(group.items.prefix(MemoryReview.shownAtFirst))
            VStack(alignment: .leading, spacing: compact ? 6 : 8) {
                if !group.title.isEmpty {
                    Text(group.title.uppercased()).tracking(1.1)
                        .font(.system(size: 9.5, weight: .semibold)).foregroundStyle(Palette.secondary)
                }
                ForEach(shown) { item in
                    ItemRow(model: model, part: part, item: item, compact: compact)
                }
                if !group.all.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(group.all, id: \.self) { button in
                            Button(button.label) {
                                Task { await model.answer(part, ids: group.ids, with: button, all: true) }
                            }
                            .buttonStyle(FerbButtonStyle(primary: button == group.all.first))
                            .disabled(model.isSending(part, id: nil))
                        }
                    }
                }
                if group.items.count > MemoryReview.shownAtFirst {
                    Button(open ? MemoryReview.showLess : MemoryReview.showAll(group.items.count)) { open.toggle() }
                        .buttonStyle(.plain).font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.white.opacity(0.8))
                        .pointingHand("button")
                }
            }
        }
    }

    private struct ItemRow: View {
        @ObservedObject var model: MemoryReviewModel
        let part: MemoryReview.Part
        let item: MemoryReview.Item
        let compact: Bool

        var body: some View {
            VStack(alignment: .leading, spacing: 5) {
                Text(item.text).font(.system(size: compact ? 12 : 13)).foregroundStyle(Palette.ink)
                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                if !item.note.isEmpty {
                    Text(item.note).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                }
                HStack(spacing: 6) {
                    ForEach(item.buttons(in: part), id: \.self) { button in
                        Button(button.label) {
                            Task { await model.answer(part, ids: [item.id], with: button) }
                        }
                        .buttonStyle(FerbButtonStyle(quiet: true))
                        .disabled(model.isSending(part, id: item.id))
                    }
                }
            }
            .padding(compact ? 8 : 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
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
