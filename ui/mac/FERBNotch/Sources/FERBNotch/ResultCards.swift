import AppKit
import SwiftUI
import NotchCore

/// Typed cards (step 8e, owner 2026-10-31). The look follows Shapeshift's intent cards (MIT, Copyright (c) 2026
/// Shapeshift contributors; ThirdPartyNotices.txt): a title in semibold, soft capsule chips for the filled fields, a
/// hero number for amounts and countdowns, an initials avatar for a person; links show as the sites row (APP-700). Rebuilt in
/// SwiftUI for the dark notch; Shapeshift's text classifier is not used: the daemon's `type` decides each card.
/// Functional first; the owner polishes.

/// The cards under (or, for a reminder, above) FERB's text: one surface each, then the links, then the files.
struct ResultCardsView: View {
    let cards: [ResultCard]
    let turnID: UUID
    @ObservedObject var model: NotchModel

    var body: some View {
        let below = CardLayout.below(cards), links = CardLayout.links(cards), files = CardLayout.files(cards)
        if !below.isEmpty || !links.isEmpty || !files.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(below.enumerated()), id: \.offset) { _, card in
                    ResultCardView(card: card, turnID: turnID, model: model)
                }
                if !links.isEmpty { LinkCardsRow(cards: links, model: model) }
                if !files.isEmpty {
                    FlowRow(spacing: 6) {
                        ForEach(Array(files.enumerated()), id: \.offset) { _, card in
                            if let url = card.fileURL { FileChip(url: url, model: model) }
                        }
                    }
                }
            }
            .padding(.trailing, 28)
        }
    }
}

/// One card by its type.
struct ResultCardView: View {
    let card: ResultCard
    let turnID: UUID
    @ObservedObject var model: NotchModel

    var body: some View {
        switch card.type {
        case "reminder": ReminderCard(card: card)
        case "event":
            if card.isTimer { TimerSetCard(card: card, model: model) } else { EventCard(card: card) }
        case "note": NoteCard(card: card, model: model)
        case "payment": ExpenseRow(card: card)
        case "send": ContactCard(card: card)
        default: EmptyView()
        }
    }
}

/// The surface every card sits on: the notch's bubble tint with a hairline, rounded like the turn card.
struct CardSurface<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        content
            .padding(.horizontal, 12).padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(.white.opacity(0.06)))
    }
}

/// Shapeshift's `Chip`: a soft capsule with an icon, for a filled field.
struct CardChip: View {
    let icon: String?
    let text: String
    var tint: Color = Palette.ink

    var body: some View {
        HStack(spacing: 4) {
            if let icon { Image(systemName: icon).font(.system(size: 9.5)).foregroundStyle(Palette.secondary) }
            Text(verbatim: text).lineLimit(1)
        }
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(tint)
        .padding(.horizontal, 8).frame(height: 22)
        .background(.white.opacity(0.08), in: Capsule())
    }
}

/// The card's title (Shapeshift's 17 pt / 550 weight, at notch scale).
private struct CardTitle: View {
    let text: String
    var body: some View {
        Text(verbatim: text).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(Palette.ink)
            .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
    }
}

/// A small label above a title: what kind of card this is.
private struct CardMeta: View {
    let icon: String
    let text: String
    var body: some View {
        Label(text, systemImage: icon).font(.system(size: 10, weight: .medium)).foregroundStyle(Palette.secondary)
    }
}

/// When, as Shapeshift's `formatWhen` says it: Today, Tomorrow, a weekday within the week, else a short date.
enum CardTime {
    static func day(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: now),
                                           to: calendar.startOfDay(for: date)).day ?? 0
        if days == 0 { return "Today" }
        if days == 1 { return "Tomorrow" }
        if days > 1 && days < 7 { return date.formatted(.dateTime.weekday(.wide)) }
        return date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day())
    }

    static func time(_ date: Date) -> String { date.formatted(date: .omitted, time: .shortened) }
}

/// A scheduled run came due: its title, when it was due, its rule, and a countdown to the schedule's recorded end when
/// it has one (ORCH-320), else to its next run (a deadline only in the request's words gets none).
struct ReminderCard: View {
    let card: ResultCard
    var framed = true
    /// For the arrival's and the timer's buttons (Done, Stop, Repeat); nil where nothing acts (a measurement).
    var model: NotchModel? = nil

    var body: some View {
        Group {
            if card.isTimer { TimerDoneView(card: card, model: model, framed: framed) }   // APP-762: the timer's end
            else if framed { CardSurface { details } }
            else { arrivalDetails }
        }
    }

    /// What kind of card this is: a timer says it's a timer (its row's kind, T-1055); a schedule's run or a reminder
    /// says Reminder. The title is the short name, never the instruction.
    private var meta: (icon: String, text: String) { card.isTimer ? ("timer", "Timer") : ("bell.fill", "Reminder") }

    /// The arrival fills the notch (APP-762, as the timer's alert does): a tinted icon, the title large, when it was
    /// due, the Repeats badge and the countdown when recorded (APP-627), and Done, which dismisses it (the chat keeps
    /// its message, APP-622).
    private var arrivalDetails: some View {
        HStack(alignment: .top, spacing: 16) {
            Image(systemName: meta.icon)
                .font(.system(size: 17, weight: .semibold)).foregroundStyle(Palette.chatBlue)
                .frame(width: 40, height: 40)
                .background(Palette.chatBlue.opacity(0.18), in: Circle())
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    CardMeta(icon: meta.icon, text: meta.text)
                    Spacer()
                    if card.repeatRule != nil { RepeatsBadge(rule: card.repeatRule) }
                }
                HStack(alignment: .top, spacing: 20) {
                    VStack(alignment: .leading, spacing: 10) {
                        Text(card.title ?? "Scheduled reminder")
                            .font(.system(size: 24, weight: .semibold)).tracking(-0.5)
                            .foregroundStyle(Palette.ink).fixedSize(horizontal: false, vertical: true)
                        if let due = card.dueDate {
                            Text("Due " + CardTime.day(due) + ", " + CardTime.time(due))
                                .font(.system(size: 12, weight: .medium)).foregroundStyle(Palette.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        Button("Done") { model?.stopTimerAlert() }
                            .buttonStyle(FerbButtonStyle(primary: true))
                            .accessibilityLabel("Dismiss the reminder")
                            .padding(.top, 2)
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    if let target = card.countdown {
                        CountdownLine(target: target, prominent: true).frame(width: 132, alignment: .trailing)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, minHeight: 112, alignment: .topLeading)
    }

    private var details: some View {
            VStack(alignment: .leading, spacing: 7) {
                CardMeta(icon: meta.icon, text: meta.text)
                CardTitle(text: card.title ?? "Scheduled reminder")
                HStack(spacing: 5) {
                    if let due = card.dueDate {
                        CardChip(icon: "clock", text: "Due " + CardTime.day(due) + ", " + CardTime.time(due))
                    }
                    if card.repeatRule != nil { RepeatsBadge(rule: card.repeatRule) }
                }
                if let target = card.countdown { CountdownLine(target: target) }
            }
    }
}

/// The hero countdown of a reminder or event card: to the schedule's end ("until it ends, last day Mon 3 Nov") or to
/// its next run.
private struct CountdownLine: View {
    let target: CountdownTarget
    var prominent = false

    var body: some View {
        let at = target.date
        TimelineView(.periodic(from: Date(), by: 30)) { context in
            let left = Countdown.text(to: at, now: context.date)
            let layout = prominent ? AnyLayout(VStackLayout(alignment: .trailing, spacing: 7))
                : AnyLayout(HStackLayout(alignment: .firstTextBaseline, spacing: 6))
            layout {
                Text(verbatim: left == "now" ? "Now" : String(left.dropFirst(3)))
                    .font(.system(size: prominent ? 29 : 22, weight: .semibold)).monospacedDigit()
                    .tracking(prominent ? -0.7 : 0)
                    .foregroundStyle(Palette.ink)
                Text(Self.caption(target, due: left == "now"))
                    .font(.system(size: 11, weight: .medium)).foregroundStyle(Palette.secondary)
                    .multilineTextAlignment(prominent ? .trailing : .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityLabel(Self.caption(target, due: false) + ", " + Countdown.text(to: at, now: Date()))
    }

    static func caption(_ target: CountdownTarget, due: Bool) -> String {
        switch target {
        case .end(let end):
            if due { return "ended" }
            if let day = CountdownTarget.lastDay(of: end) { return "until it ends, last day " + CardTime.day(day) }
            return "until it ends, " + CardTime.day(end) + " " + CardTime.time(end)
        case .nextRun(let next):
            return due ? "next run due" : "until the next one, " + CardTime.day(next) + " " + CardTime.time(next)
        }
    }
}

/// "Repeats" with FERB's words for the rule in its tooltip.
private struct RepeatsBadge: View {
    let rule: String?
    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: "repeat").font(.system(size: 9, weight: .semibold))
            Text("Repeats")
        }
        .font(.system(size: 10, weight: .semibold))
        .foregroundStyle(.white)
        .padding(.horizontal, 7).frame(height: 20)
        .background(Palette.chatBlue, in: Capsule())
        .help(rule ?? "")
        .accessibilityLabel("Repeats " + (rule ?? ""))
    }
}

/// A reminder or schedule just set: what, its day and time chips, "Repeats" with its rule.
struct EventCard: View {
    let card: ResultCard

    var body: some View {
        CardSurface {
            VStack(alignment: .leading, spacing: 7) {
                CardMeta(icon: card.isTimer ? "timer" : card.kind == "reminder" ? "alarm" : "calendar",
                         text: card.isTimer ? "Timer set" : card.kind == "reminder" ? "Reminder set" : "Scheduled")
                CardTitle(text: card.what ?? "")
                HStack(spacing: 5) {
                    if let when = card.whenDate {
                        CardChip(icon: "calendar", text: CardTime.day(when))
                        CardChip(icon: "clock", text: CardTime.time(when))
                    }
                    if card.repeatRule != nil { RepeatsBadge(rule: card.repeatRule) }
                }
                if let rule = card.repeatRule {
                    Text(verbatim: rule.prefix(1).capitalized + rule.dropFirst() + (card.watch.map { ", " + $0 } ?? ""))
                        .font(.system(size: 10.5)).foregroundStyle(Palette.secondary)
                } else if let watch = card.watch {
                    Text(verbatim: watch.prefix(1).capitalized + watch.dropFirst())
                        .font(.system(size: 10.5)).foregroundStyle(Palette.secondary)
                }
                if let target = card.countdown { CountdownLine(target: target) }
            }
        }
    }
}

/// A remembered fact (Shapeshift's NoteCard): its words; tapping offers Correct and Forget. Correct edits the item in
/// the owner's words (`memory.edit`); Forget shows exactly what goes (`memory.forget` without confirm) and deletes it
/// only on the owner's second tap.
struct NoteCard: View {
    let card: ResultCard
    @ObservedObject var model: NotchModel
    @State private var open = false
    @State private var editing: String?
    @State private var forgetting: [String]?
    @State private var busy = false

    var body: some View {
        CardSurface {
            VStack(alignment: .leading, spacing: 7) {
                HStack(spacing: 6) {
                    CardMeta(icon: "brain", text: Self.label(card.state))
                    Spacer(minLength: 0)
                    if Self.actionable(card.state) {
                        Button { withAnimation(.easeOut(duration: 0.15)) { open.toggle() } } label: {
                            Image(systemName: "ellipsis").font(.system(size: 11, weight: .semibold))
                                .frame(width: 20, height: 16)
                        }
                        .buttonStyle(.plain).foregroundStyle(Palette.secondary)
                        .help("Correct or forget this").accessibilityLabel("Correct or forget this")
                    }
                }
                Text(verbatim: card.text ?? "")
                    .font(.system(size: 12.5)).foregroundStyle(card.state == "forgotten" ? Palette.secondary : Palette.ink)
                    .strikethrough(card.state == "forgotten")
                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                if open && Self.actionable(card.state) { actions }
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { if Self.actionable(card.state) && editing == nil { open = true } }
    }

    @ViewBuilder private var actions: some View {
        if let words = editing {
            VStack(alignment: .leading, spacing: 6) {
                TextField("In your words…", text: Binding(get: { words }, set: { editing = $0 }), axis: .vertical)
                    .textFieldStyle(.plain).font(.system(size: 12)).padding(7)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
                    .onSubmit(save)
                HStack(spacing: 6) {
                    Button("Save", action: save).buttonStyle(FerbButtonStyle(primary: true))
                        .disabled(busy || words.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    Button("Cancel") { editing = nil }.buttonStyle(FerbButtonStyle(quiet: true))
                }
            }
        } else if let going = forgetting {
            VStack(alignment: .leading, spacing: 5) {
                Text("Forget this? FERB deletes:").font(.system(size: 11, weight: .semibold))
                ForEach(Array(going.enumerated()), id: \.offset) { _, line in
                    Text(verbatim: "• " + line).font(.system(size: 10.5)).foregroundStyle(Palette.secondary)
                }
                HStack(spacing: 6) {
                    Button("Forget") {
                        busy = true
                        model.forgetMemory(itemID: card.itemID ?? "") { _ in busy = false; forgetting = nil; open = false }
                    }
                    .buttonStyle(FerbButtonStyle(primary: true)).disabled(busy)
                    Button("Keep it") { forgetting = nil }.buttonStyle(FerbButtonStyle(quiet: true))
                }
            }
        } else {
            HStack(spacing: 6) {
                Button("Correct") { editing = card.text ?? "" }.buttonStyle(FerbButtonStyle())
                Button("Forget") {
                    busy = true
                    model.forgetPlan(itemID: card.itemID ?? "") { plan in busy = false; forgetting = plan }
                }
                .buttonStyle(FerbButtonStyle(quiet: true)).disabled(busy)
            }
        }
    }

    private func save() {
        guard let words = editing?.trimmingCharacters(in: .whitespacesAndNewlines), !words.isEmpty else { return }
        busy = true
        model.correctMemory(itemID: card.itemID ?? "", text: words) { ok in
            busy = false
            if ok { editing = nil; open = false }
        }
    }

    static func actionable(_ state: String?) -> Bool { state != "forgotten" }

    static func label(_ state: String?) -> String {
        switch state {
        case "proposed": return "Saved in my words · waits for your Keep"
        case "corrected": return "Remembered · corrected"
        case "forgotten": return "Forgotten"
        default: return "Remembered"
        }
    }
}

/// A payment the owner approved that ran (Shapeshift's ExpenseRow): the merchant and site on the left, the amount as
/// the page showed it on the right, and the name of the owner's saved card that paid (ORCH-321). Never a card digit:
/// FERB's facts carry none.
struct ExpenseRow: View {
    let card: ResultCard

    var body: some View {
        CardSurface {
            HStack(alignment: .center, spacing: 10) {
                Image(systemName: "creditcard.fill").font(.system(size: 15)).foregroundStyle(Palette.ink)
                    .frame(width: 34, height: 34)
                    .background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                VStack(alignment: .leading, spacing: 4) {
                    CardTitle(text: card.merchant ?? card.site ?? "Payment")
                    FlowRow(spacing: 5) {
                        CardChip(icon: "checkmark.seal", text: "You approved").fixedSize()
                        if let site = card.site, site != card.merchant { CardChip(icon: "globe", text: site).fixedSize() }
                        if let saved = card.savedCard { CardChip(icon: "creditcard", text: saved).fixedSize() }
                    }
                }
                .layoutPriority(1)
                Spacer(minLength: 8)
                Text(verbatim: card.amount ?? "Amount not read")
                    .font(.system(size: card.amount == nil ? 11 : 20, weight: .semibold)).monospacedDigit()
                    .foregroundStyle(card.amount == nil ? Palette.secondary : Palette.ink)
                    .lineLimit(1).minimumScaleFactor(0.6).fixedSize()
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// A send that ran (Shapeshift's ContactCard): an initials avatar, the recipient, and where it went.
struct ContactCard: View {
    let card: ResultCard

    var body: some View {
        CardSurface {
            HStack(alignment: .top, spacing: 10) {
                Text(card.initials).font(.system(size: 13, weight: .semibold)).foregroundStyle(Palette.ink)
                    .frame(width: 34, height: 34).background(.white.opacity(0.12), in: Circle())
                VStack(alignment: .leading, spacing: 5) {
                    CardTitle(text: card.recipient ?? place)
                    HStack(spacing: 6) {
                        Image(systemName: Self.icon(card.kind)).font(.system(size: 10.5)).foregroundStyle(Palette.secondary)
                        Text(verbatim: Self.verb(card.kind) + (card.recipient == nil ? "" : " · " + place))
                            .font(.system(size: 11)).foregroundStyle(Palette.secondary).lineLimit(1)
                    }
                    if let account = card.account {
                        Text(verbatim: "From " + account).font(.system(size: 10.5)).foregroundStyle(.tertiary)
                    }
                }
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// The app's name for a Mac app's bundle id (looked up on this Mac, never fetched), else the site.
    private var place: String {
        if let bundle = card.app {
            if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle) {
                return FileManager.default.displayName(atPath: url.path).replacingOccurrences(of: ".app", with: "")
            }
            return bundle
        }
        return card.site ?? ""
    }

    static func icon(_ kind: String?) -> String {
        ["call": "phone.fill", "mailbox_send": "envelope.fill", "post": "square.and.pencil",
         "reply": "arrowshape.turn.up.left.fill"][kind ?? ""] ?? "paperplane.fill"
    }

    static func verb(_ kind: String?) -> String {
        ["call": "Called", "mailbox_send": "Emailed", "post": "Posted", "reply": "Replied"][kind ?? ""] ?? "Sent"
    }
}

/// The sites a task contacted, one card each from the daemon: shown as the sites row (APP-700), the first site and
/// "+N" opening the list of all, in chat and in the notch. Nothing is fetched.
struct LinkCardsRow: View {
    let cards: [ResultCard]
    @ObservedObject var model: NotchModel

    static func sites(_ cards: [ResultCard]) -> VisitedSites { VisitedSites(hosts: CardLayout.links(cards).compactMap(\.host)) }

    var body: some View {
        let sites = Self.sites(cards)
        if !sites.all.isEmpty { VisitedSitesView(sites: sites, model: model, leading: 0) }
    }
}

/// A file the task made: its icon and name; a click shows it in Finder, selected. The name is selectable, and the
/// context menu and drag-out are every file card's own (FileCardMenu, APP-715).
struct FileChip: View {
    let url: URL
    @ObservedObject var model: NotchModel
    @StateObject private var card = FileCardState()
    @State private var hovered = false
    func reveal() { card.reveal(url, with: model) }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Button(action: reveal) {
                HStack(spacing: 6) {
                    Image(nsImage: NSWorkspace.shared.icon(forFile: url.path)).resizable().scaledToFit()
                        .frame(width: 16, height: 16)
                    SelectableName(text: url.lastPathComponent, size: 11, weight: .medium, click: reveal,
                                   actions: CardAction.file(url, model: model, card: card))
                }
                .foregroundStyle(Palette.ink)
                .padding(.horizontal, 9).frame(height: 28)
                .cardHover($hovered, link: true, label: "file")
                .contentShape(RoundedRectangle(cornerRadius: 9))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Show \(url.lastPathComponent) in Finder")
            .modifier(FileCardMenu(url: url, model: model, card: card))
            if card.unavailable { FileUnavailableLine() }
        }
    }
}

/// Chips laid out left to right, wrapping to the next line.
struct FlowRow: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(subviews, width: proposal.width ?? .infinity)
        let height = rows.last.map { $0.y + $0.height } ?? 0
        let width = rows.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for row in arrange(subviews, width: bounds.width) {
            var x = bounds.minX
            for index in row.items {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: bounds.minY + row.y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
        }
    }

    private struct Row { var items: [Int] = []; var y: CGFloat = 0; var width: CGFloat = 0; var height: CGFloat = 0 }

    private func arrange(_ subviews: Subviews, width: CGFloat) -> [Row] {
        var rows: [Row] = [Row()]
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            if !rows[rows.count - 1].items.isEmpty && rows[rows.count - 1].width + spacing + size.width > width {
                let last = rows[rows.count - 1]
                rows.append(Row(y: last.y + last.height + spacing))
            }
            var row = rows[rows.count - 1]
            row.width += (row.items.isEmpty ? 0 : spacing) + size.width
            row.height = max(row.height, size.height)
            row.items.append(index)
            rows[rows.count - 1] = row
        }
        return rows.filter { !$0.items.isEmpty }
    }
}

// MARK: - Memory actions from a note card

@MainActor
extension NotchModel {
    /// Correct: the item's words become the owner's (`memory.edit`, journaled as the owner's edit).
    func correctMemory(itemID: String, text: String, done: @escaping (Bool) -> Void) {
        Task {
            do {
                _ = try await rpc.call("memory.edit", params: ["id": itemID, "content": text])
                conversation.settleNote(itemID: itemID, text: text)
                saveConversation()
                done(true)
            } catch {
                promptError = error.localizedDescription
                done(false)
            }
        }
    }

    /// What a forget would delete, exactly (`memory.forget` without `confirm` deletes nothing).
    func forgetPlan(itemID: String, done: @escaping ([String]?) -> Void) {
        Task {
            do {
                let plan = try await rpc.call("memory.forget", params: ["id": itemID, "confirm": false])
                done(plan["will_delete"] as? [String] ?? [])
            } catch {
                promptError = error.localizedDescription
                done(nil)
            }
        }
    }

    /// Forget, after the owner saw what goes.
    func forgetMemory(itemID: String, done: @escaping (Bool) -> Void) {
        Task {
            do {
                _ = try await rpc.call("memory.forget", params: ["id": itemID, "confirm": true])
                conversation.settleNote(itemID: itemID, forgotten: true)
                saveConversation()
                done(true)
            } catch {
                promptError = error.localizedDescription
                done(false)
            }
        }
    }
}
