import AppKit
import SwiftUI
import NotchCore

/// FERB's record cards (ORCH-563..ORCH-565, DMN-554, APP-802, APP-803): what FERB's own records hold, drawn under the reply when
/// the owner asks about FERB's state (reminders, tasks, memory, permissions, logins, about), about what a task did (its
/// task card), and at a task's end (submit, trash, delete, download). Every row comes from the daemon's payload, which
/// code built from FERB's rows; nothing is read from the reply's text. Plain and functional, in the typed cards' look
/// (ResultCards.swift); the owner polishes.

/// The first `limit` rows, the rest behind "Show all N" (a long list stays readable in the notch).
struct CardRows<Row, Content: View>: View {
    let rows: [Row]
    var limit = 6
    var spacing: CGFloat = 8
    @ViewBuilder let content: (Row) -> Content
    @State private var all = false

    var body: some View {
        VStack(alignment: .leading, spacing: spacing) {
            ForEach(Array((all ? rows : Array(rows.prefix(limit))).enumerated()), id: \.offset) { _, row in
                content(row)
            }
            if rows.count > limit {
                Button(all ? "Show fewer" : "Show all \(rows.count)") {
                    withAnimation(.easeOut(duration: 0.15)) { all.toggle() }
                }
                .buttonStyle(.plain).font(.system(size: 10.5, weight: .semibold)).foregroundStyle(Palette.chatBlue)
            }
        }
    }
}

/// A small heading inside a card ("Standing sends", "FERB's own").
private struct CardSection: View {
    let text: String
    var body: some View {
        Text(verbatim: text).font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.secondary)
            .textCase(.uppercase).padding(.top, 2)
    }
}

/// A secondary line under a row's title.
private struct CardDetail: View {
    let text: String
    var body: some View {
        Text(verbatim: text).font(.system(size: 10.5)).foregroundStyle(Palette.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}

enum RecordWords {
    /// "1 reminder", "3 reminders"; `none` for zero.
    static func count(_ n: Int, _ one: String, _ many: String? = nil, none: String) -> String {
        n == 0 ? none : n == 1 ? "1 \(one)" : "\(n) \(many ?? one + "s")"
    }

    /// A permission id as words: "screen_recording" → "Screen recording".
    static func id(_ id: String) -> String {
        let words = id.replacingOccurrences(of: "_", with: " ")
        return words.prefix(1).uppercased() + words.dropFirst()
    }
}

// MARK: - Reminders and timers

/// The pending reminders and timers (ORCH-563): each one's name, kind, when it is due and, today, the time left.
struct RemindersListCard: View {
    let card: ResultCard
    @Environment(\.cardsLive) private var live

    var body: some View {
        let rows = card.reminders ?? []
        let timers = rows.filter(\.isTimer).count, reminders = rows.count - timers
        CardSurface {
            VStack(alignment: .leading, spacing: 8) {
                CardMeta(icon: "bell", text: rows.isEmpty ? "No reminders or timers"
                         : [RecordWords.count(reminders, "reminder", none: ""), RecordWords.count(timers, "timer", none: "")]
                            .filter { !$0.isEmpty }.joined(separator: ", "))
                CardRows(rows: rows) { row in
                    VStack(alignment: .leading, spacing: 4) {
                        CardTitle(text: row.name)
                        FlowRow(spacing: 5) {
                            CardChip(icon: row.isTimer ? "timer" : "alarm", text: row.isTimer ? "Timer" : "Reminder").fixedSize()
                            if let at = row.atDate {
                                CardChip(icon: "calendar", text: CardTime.day(at)).fixedSize()
                                CardChip(icon: "clock", text: CardTime.time(at)).fixedSize()
                                if live {
                                    TimelineView(.periodic(from: Date(), by: 30)) { context in
                                        CardChip(icon: "hourglass", text: Countdown.text(to: at, now: context.date))
                                    }.fixedSize()
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// MARK: - Tasks

/// The owner's tasks (ORCH-563): every running task and this chat's ended ones, each as its task card says it.
struct TasksListCard: View {
    let card: ResultCard
    @ObservedObject var model: NotchModel

    var body: some View {
        let rows = card.tasks ?? []
        CardSurface {
            VStack(alignment: .leading, spacing: 10) {
                CardMeta(icon: "checklist", text: RecordWords.count(rows.count, "task", none: "No tasks in this chat yet"))
                CardRows(rows: rows, limit: 5, spacing: 10) { row in
                    VStack(alignment: .leading, spacing: 10) {
                        Divider().opacity(0.25)
                        TaskRecordDetails(card: row, model: model)
                    }
                }
            }
        }
    }
}

/// One task as FERB's records hold it (ORCH-564): its name, its status as code set it, whether it was checked, code's
/// couldn't-confirm note, the questions the owner answered for it and what it did (its record cards, one line each).
struct TaskRecordCard: View {
    let card: ResultCard
    @ObservedObject var model: NotchModel

    var body: some View {
        CardSurface {
            VStack(alignment: .leading, spacing: 7) {
                CardMeta(icon: "list.bullet.clipboard", text: "Task")
                TaskRecordDetails(card: card, model: model)
            }
        }
    }
}

struct TaskRecordDetails: View {
    let card: ResultCard
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            CardTitle(text: card.name ?? "Task")
            FlowRow(spacing: 5) {
                CardChip(icon: Self.icon(card.status), text: ResultCard.statusSaid(card.status),
                         tint: Self.tint(card.status)).fixedSize()
                if card.verified == true { CardChip(icon: "checkmark.seal", text: "Checked").fixedSize() }
            }
            if let note = card.note { CardDetail(text: "Couldn't confirm: " + note) }
            if let questions = card.questions, !questions.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    CardSection(text: "You answered")
                    ForEach(Array(questions.enumerated()), id: \.offset) { _, asked in
                        (Text(verbatim: asked.question + " ").foregroundStyle(Palette.secondary)
                         + Text(verbatim: asked.answer).fontWeight(.semibold).foregroundStyle(Palette.ink))
                            .font(.system(size: 10.5)).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            let done = (card.records ?? []).compactMap(\.recordLine)
            if !done.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    CardSection(text: "What it did")
                    ForEach(Array(done.enumerated()), id: \.offset) { _, line in
                        Label { Text(verbatim: line.text).fixedSize(horizontal: false, vertical: true) } icon: {
                            Image(systemName: line.icon).foregroundStyle(Palette.secondary)
                        }
                        .font(.system(size: 10.5)).foregroundStyle(Palette.ink)
                    }
                }
            }
        }
    }

    static func icon(_ status: String?) -> String {
        switch status ?? "" {
        case "running": return "circle.dotted"
        case "paused": return "pause.circle"
        case "stopping", "cancelled": return "stop.circle"
        case "done": return "checkmark.circle"
        case "done_unverified": return "checkmark.circle.trianglebadge.exclamationmark"
        case "failed": return "xmark.circle"
        case "needs_user": return "questionmark.circle"
        default: return "circle"
        }
    }

    static func tint(_ status: String?) -> Color {
        switch status ?? "" {
        case "done": return Palette.green
        case "done_unverified", "running", "needs_user": return Palette.amber
        case "failed", "interrupted": return Palette.redSoft
        default: return Palette.ink
        }
    }
}

// MARK: - Memory

/// What FERB remembers now (ORCH-563), and the forgets on record: when and how many, never their contents.
struct MemoryListCard: View {
    let card: ResultCard

    var body: some View {
        let items = card.memoryItems ?? [], forgets = card.forgotten ?? []
        let gone = forgets.reduce(0) { $0 + $1.items }
        CardSurface {
            VStack(alignment: .leading, spacing: 8) {
                CardMeta(icon: "brain", text: "FERB remembers " + RecordWords.count(items.count, "thing", none: "nothing"))
                CardRows(rows: items, limit: 8) { item in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(verbatim: item.text).font(.system(size: 12)).foregroundStyle(Palette.ink)
                            .fixedSize(horizontal: false, vertical: true)
                        FlowRow(spacing: 5) {
                            if let said = item.saidByOwner {
                                CardChip(icon: said ? "person" : "sparkles", text: said ? "You said" : "Inferred").fixedSize()
                            }
                            if let kind = item.kind, !kind.isEmpty { CardChip(icon: "tag", text: kind).fixedSize() }
                        }
                    }
                }
                if gone > 0 {
                    CardDetail(text: "Forgot " + RecordWords.count(gone, "item", none: "") + " (their contents aren't kept)"
                               + (forgets.last?.atDate.map { ", last on " + CardTime.day($0) } ?? "") + ".")
                }
            }
        }
    }
}

// MARK: - Permissions

/// What lets FERB act without asking (ORCH-565): the standing send scopes in force, the destinations it knows and
/// whether each was approved, and whether routine questions are off for now.
struct PermissionsCard: View {
    let card: ResultCard

    var body: some View {
        let sends = card.standingSends ?? [], known = card.knownDestinations ?? []
        CardSurface {
            VStack(alignment: .leading, spacing: 8) {
                CardMeta(icon: "lock.shield", text: "What FERB may do without asking")
                CardSection(text: "Standing sends")
                if sends.isEmpty { CardDetail(text: "None in force.") }
                CardRows(rows: sends, limit: 4) { send in
                    VStack(alignment: .leading, spacing: 3) {
                        CardTitle(text: Self.capability(send.capability))
                        if !send.recipients.isEmpty { CardDetail(text: "To " + send.recipients.joined(separator: ", ")) }
                        FlowRow(spacing: 5) {
                            if let site = send.site { CardChip(icon: "globe", text: site).fixedSize() }
                            if let account = send.account ?? send.identity { CardChip(icon: "person.crop.circle", text: account).fixedSize() }
                            CardChip(icon: "text.bubble", text: send.anyText ? "Any text" : "Set text").fixedSize()
                        }
                        if let held = send.heldBy { CardDetail(text: "For task " + String(held.prefix(8))) }
                    }
                }
                CardSection(text: "Known destinations")
                if known.isEmpty { CardDetail(text: "None yet.") }
                CardRows(rows: known, limit: 5, spacing: 5) { place in
                    HStack(spacing: 6) {
                        Text(verbatim: place.value).font(.system(size: 11.5, weight: .medium)).foregroundStyle(Palette.ink)
                            .lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: 4)
                        CardChip(icon: place.approved ? "checkmark.seal" : "quote.bubble", text: Self.how(place.how)).fixedSize()
                    }
                }
                CardSection(text: "Routine questions")
                if let until = LiveProgress.time(card.questionsOffUntil) {
                    CardDetail(text: "Off until " + CardTime.day(until) + ", " + CardTime.time(until) + ".")
                } else {
                    CardDetail(text: "On: FERB asks them as usual.")
                }
            }
        }
    }

    static func capability(_ capability: String?) -> String {
        switch capability ?? "" {
        case "send": return "Send messages"
        case "reply": return "Reply"
        case "post": return "Post"
        case "mailbox_send": return "Send email"
        case "call": return "Call"
        case "": return "Send"
        default: return RecordWords.id(capability ?? "")
        }
    }

    static func how(_ how: String?) -> String {
        switch how ?? "" {
        case "named": return "You named it"
        case "answered": return "You chose it"
        case "confirmed": return "You confirmed it"
        default: return "Known"
        }
    }
}

// MARK: - Logins

/// The logins FERB can fill (ORCH-565), by name and site only: never a value, a username or an account.
struct LoginsCard: View {
    let card: ResultCard

    var body: some View {
        let owner = card.ownerLogins ?? [], mine = card.ferbLogins ?? []
        let saved = owner.filter { $0.name != nil }
        CardSurface {
            VStack(alignment: .leading, spacing: 8) {
                CardMeta(icon: "key", text: RecordWords.count(saved.count, "saved login", none: "No logins are saved in FERB"))
                ForEach(Array(owner.compactMap(\.problem).enumerated()), id: \.offset) { _, problem in
                    CardDetail(text: problem.prefix(1).uppercased() + problem.dropFirst() + ".")
                }
                CardRows(rows: saved, limit: 6, spacing: 6) { login in
                    VStack(alignment: .leading, spacing: 3) {
                        CardTitle(text: login.name ?? "")
                        FlowRow(spacing: 5) {
                            if let place = login.place { CardChip(icon: "globe", text: place).fixedSize() }
                            if let kind = login.kind, kind != "secret" { CardChip(icon: "creditcard", text: kind).fixedSize() }
                            if login.unusable { CardChip(icon: "exclamationmark.triangle", text: "Can't be used", tint: Palette.redSoft).fixedSize() }
                        }
                    }
                }
                if !mine.isEmpty {
                    CardSection(text: "FERB's own")
                    CardRows(rows: mine, limit: 4, spacing: 6) { login in
                        VStack(alignment: .leading, spacing: 3) {
                            CardTitle(text: login.name)
                            if !login.sites.isEmpty {
                                FlowRow(spacing: 5) {
                                    ForEach(login.sites, id: \.self) { CardChip(icon: "globe", text: $0).fixedSize() }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// MARK: - About

/// FERB itself (ORCH-565): its version, the model each role uses and its permissions' state.
struct AboutCard: View {
    let card: ResultCard

    var body: some View {
        let models = card.models ?? [], permissions = card.permissionRows ?? []
        let missing = permissions.filter { !$0.granted }
        CardSurface {
            VStack(alignment: .leading, spacing: 8) {
                CardMeta(icon: "info.circle", text: "About FERB")
                CardTitle(text: "FERB " + (card.version ?? ""))
                if !models.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        ForEach(models, id: \.role) { row in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Text(verbatim: RecordWords.id(row.role)).font(.system(size: 10.5, weight: .semibold))
                                    .foregroundStyle(Palette.secondary).frame(width: 58, alignment: .leading)
                                Text(verbatim: row.model).font(.system(size: 11.5)).foregroundStyle(Palette.ink)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                }
                if !permissions.isEmpty {
                    CardDetail(text: "\(permissions.count - missing.count) of \(permissions.count) permissions granted")
                    if !missing.isEmpty {
                        FlowRow(spacing: 5) {
                            ForEach(missing, id: \.id) {
                                CardChip(icon: "exclamationmark.circle", text: RecordWords.id($0.id), tint: Palette.amber).fixedSize()
                            }
                        }
                    }
                }
            }
        }
    }
}

// MARK: - Submits, trash, deletes, downloads

/// Something a task did that its records show (ORCH-561): a form submitted on a site, a file moved to the Trash or
/// deleted permanently, a file downloaded (shown in Finder from its chip while it exists).
struct DoneRecordCard: View {
    let card: ResultCard
    @ObservedObject var model: NotchModel

    var body: some View {
        CardSurface {
            VStack(alignment: .leading, spacing: 7) {
                HStack(alignment: .center, spacing: 10) {
                    Image(systemName: Self.icon(card.type)).font(.system(size: 14)).foregroundStyle(Palette.ink)
                        .frame(width: 34, height: 34)
                        .background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    VStack(alignment: .leading, spacing: 3) {
                        CardTitle(text: title)
                        CardDetail(text: Self.verb(card.type) + (folder.map { " · " + $0 } ?? ""))
                    }
                }
                if card.type == "download", let url = card.fileURL, FileManager.default.fileExists(atPath: url.path) {
                    FileChip(url: url, model: model)
                }
            }
        }
        .accessibilityElement(children: .contain)
    }

    private var title: String {
        card.type == "submit" ? (card.site ?? "A site") : (card.name ?? card.fileURL?.lastPathComponent ?? "A file")
    }

    /// The folder the file was in, with the home folder as "~".
    private var folder: String? {
        guard let url = card.fileURL else { return nil }
        let parent = url.deletingLastPathComponent().path
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return parent.hasPrefix(home) ? "~" + parent.dropFirst(home.count) : parent
    }

    static func icon(_ type: String) -> String {
        ["submit": "checkmark.rectangle", "trash": "trash", "delete": "xmark.bin", "download": "arrow.down.circle"][type]
            ?? "checkmark"
    }

    static func verb(_ type: String) -> String {
        ["submit": "Submitted a form", "trash": "Moved to the Trash", "delete": "Deleted permanently",
         "download": "Downloaded"][type] ?? "Done"
    }
}
