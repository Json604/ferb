import AppKit
import SwiftUI
import NotchCore

private func when(_ date: Date?) -> String { date?.formatted(date: .abbreviated, time: .shortened) ?? "" }
private func words(_ status: String) -> String { status.replacingOccurrences(of: "_", with: " ") }

struct TasksPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        WindowPage {
            HStack {
                WindowSectionLabel(title: "Activity", count: store.tasks.count)
                Spacer()
                if store.stopped {
                    Button("Resume FERB") { Task { await store.resumeEverything() } }
                } else {
                    Button("Stop everything") { Task { await store.stopEverything() } }
                }
                Button { Task { await store.loadTasks() } } label: {
                    Image(systemName: "arrow.clockwise")
                }.help("Refresh tasks").accessibilityLabel("Refresh tasks")
            }.padding(.bottom, 4)
            if store.stopped { Text("FERB is stopped").foregroundStyle(.secondary) }
            if store.tasks.isEmpty { Text("No tasks yet.").foregroundStyle(.secondary) }
            ForEach(store.tasks) { row in WindowCard { TaskHistoryRowView(row: row, store: store) } }
            if store.moreTasks { Button("Show older tasks") { Task { await store.loadTasks(more: true) } } }
        }
    }
}

struct TaskHistoryRowView: View {
    let row: TaskHistoryRow
    @ObservedObject var store: FERBWindowStore
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                // T-1055: the heading is the task's short name; opening the row shows the full instruction
                OpenableHeading(text: row.name.isEmpty ? row.id : row.name, open: $open,
                                canOpen: !row.goal.isEmpty && row.goal != row.name) { $0.bold().lineLimit(2) }
                Spacer()
                Text(words(row.status)).foregroundStyle(.secondary)
            }
            if open { FullInstruction(text: row.goal) }
            Text(when(row.createdAt)).font(.caption).foregroundStyle(.secondary)
            if !row.answer.isEmpty { ExpandableAnswer(text: row.answer, lines: 4).id(row.answer) }
            HStack {
                Button("Inspect") { Task { await store.inspect(row) } }
                if row.isRunning {
                    Button("Stop") { Task { await store.stop(row) } }
                } else {
                    Button("Pass") { Task { await store.setVerdict(row, "pass") } }
                    Button("Fail") { Task { await store.setVerdict(row, "fail") } }
                    if let verdict = row.verdict { Text("Your verdict: \(verdict)").foregroundStyle(.secondary) }
                }
            }
        }
        .padding(.vertical, 4)
    }
}

struct SchedulesPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        WindowPage {
            WindowSectionLabel(title: "Schedules", count: store.schedules.count)
            if store.schedules.isEmpty {
                WindowCard { Text("Ask FERB to do something regularly, like a morning brief every weekday.").foregroundStyle(.secondary) }
            }
            ForEach(store.schedules) { schedule in
                WindowCard { ScheduleCardView(schedule: schedule, store: store) }
            }
            WindowSectionLabel(title: "Reminders", count: store.reminders.count).padding(.top, 14)
            if store.reminders.isEmpty { Text("No reminders waiting.").foregroundStyle(.secondary) }
            ForEach(store.reminders) { reminder in
                WindowCard { ReminderRowView(reminder: reminder, store: store) }
            }
        }
    }
}

/// A heading the owner can open (T-1055): tapping it shows the item's full instruction below; nothing else does.
private struct OpenableHeading<Styled: View>: View {
    let text: String
    @Binding var open: Bool
    var canOpen = true
    let style: (Text) -> Styled

    var body: some View {
        if canOpen {
            Button { withAnimation(.easeOut(duration: 0.15)) { open.toggle() } } label: {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    style(Text(text)).multilineTextAlignment(.leading)
                    Image(systemName: open ? "chevron.up" : "chevron.down")
                        .font(.system(size: 9, weight: .semibold)).foregroundStyle(.secondary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint(open ? "Hides the full instruction" : "Shows the full instruction")
        } else {
            style(Text(text))
        }
    }
}

/// The item's full instruction, shown only while the owner has it open (T-1055).
private struct FullInstruction: View {
    let text: String
    var body: some View {
        Text(text).font(.system(size: 12)).foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
    }
}

/// A waiting reminder or timer: its short name, when, and (opened) what FERB will tell the owner.
private struct ReminderRowView: View {
    let reminder: ReminderRow
    @ObservedObject var store: FERBWindowStore
    @State private var open = false

    var body: some View {
        HStack(alignment: .center, spacing: 18) {
            Image(systemName: reminder.isTimer ? "timer" : "bell").font(.system(size: 17)).foregroundStyle(.white.opacity(0.4))
            VStack(alignment: .leading, spacing: 7) {
                OpenableHeading(text: reminder.name, open: $open, canOpen: reminder.hasDetail) {
                    $0.font(.system(size: 14, weight: .medium))
                }
                if open { FullInstruction(text: reminder.message) }
                if let date = reminder.at {
                    Label((reminder.isTimer ? "Timer · " : "") + when(date), systemImage: "clock")
                        .font(.system(size: 12)).foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 0)
            Button("Cancel") { Task { await store.cancel(reminder) } }
        }
    }
}

/// A schedule: its short name and rule; opening it shows its full run instruction (T-1055).
private struct ScheduleCardView: View {
    let schedule: ScheduleRow
    @ObservedObject var store: FERBWindowStore
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 14) {
                VStack(alignment: .leading, spacing: 8) {
                    OpenableHeading(text: schedule.name, open: $open,
                                    canOpen: !schedule.request.isEmpty && schedule.request != schedule.name) {
                        $0.font(.system(size: 16, weight: .semibold)).tracking(-0.2)
                    }
                    if open { FullInstruction(text: schedule.request) }
                    Label(schedule.rule, systemImage: "repeat")
                        .font(.system(size: 12)).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
                Text(schedule.state.capitalized).font(.system(size: 10, weight: .medium))
                    .foregroundStyle(.white.opacity(schedule.state == "active" ? 0.8 : 0.45))
                    .padding(.horizontal, 10).padding(.vertical, 6)
                    .background(.white.opacity(0.06), in: Capsule())
            }
            if let last = schedule.lastResult {
                HStack(alignment: .top, spacing: 7) {
                    Image(systemName: last.status == "failed" ? "exclamationmark.circle" : (last.status == "done" ? "checkmark.circle" : "clock"))
                        .padding(.top, 1)
                    ExpandableAnswer(text: words(last.status).capitalized + " · " + last.answer, lines: 3).id(last.answer)
                }
                .font(.system(size: 12)).foregroundStyle(last.status == "failed" ? Color.red.opacity(0.85) : Color.white.opacity(0.5))
            }
            if schedule.failures > 0 {
                Text("Failed \(schedule.failures) time(s) in a row").font(.system(size: 11)).foregroundStyle(.red.opacity(0.85))
            }
            HStack(alignment: .center, spacing: 8) {
                VStack(alignment: .leading, spacing: 5) {
                    if let next = schedule.nextRun {
                        Text("Next · " + when(next)).font(.system(size: 11, weight: .medium)).foregroundStyle(.white.opacity(0.67))
                    }
                    if let end = schedule.until {
                        Text("Ends: " + (CountdownTarget.lastDay(of: end).map { "end of " + $0.formatted(date: .abbreviated, time: .omitted) } ?? when(end)))
                            .font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 8)
                ForEach(schedule.actions, id: \.self) { action in
                    Button(action.capitalized) { Task { await store.change(schedule, action) } }
                }
            }
        }
    }
}

struct NoticedPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        WindowPage {
            if store.notices.isEmpty { Text("FERB hasn't noticed anything yet.").foregroundStyle(.secondary) }
            ForEach(store.notices) { n in
                WindowCard { VStack(alignment: .leading, spacing: 4) {
                    Text(n.text).textSelection(.enabled)
                    // Outside text FERB quotes is data, shown as a quote (MP2)
                    ForEach(Array(n.quotes.enumerated()), id: \.offset) { _, quote in
                        Text("“\(quote.text)”").italic().foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Text(when(n.at)).font(.caption).foregroundStyle(.secondary)
                }
                }
            }
        }
    }
}

struct MemoryPage: View {
    @ObservedObject var store: FERBWindowStore
    var correctMemory: ((String) -> Void)? = nil
    @State private var forgetting: MemoryRow?
    @State private var willDelete: [String] = []

    var body: some View {
        WindowPage {
            Text("Correct a memory by telling FERB in chat. “Not right” stops using it and keeps its history; “Forget” deletes it permanently.")
                .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            WindowSectionLabel(title: "Always on")
            Text("Core profile · \(store.coreProfile.usedTokens) of \(store.coreProfile.budgetTokens) tokens")
                .font(.system(size: 11)).foregroundStyle(.secondary)
            WindowCard { VStack(alignment: .leading, spacing: 12) {
                if store.coreProfile.sections.isEmpty { Text("Nothing yet.").foregroundStyle(.secondary) }
                ForEach(store.coreProfile.sections, id: \.title) { section in
                    Text(section.title).font(.headline)
                    ForEach(section.ids, id: \.self) { id in
                        if let item = store.activeMemory.first(where: { $0.id == id }) { row(item) }
                    }
                }
            } }
            importSection
            if let source = store.memoryFilter {
                Section {
                    HStack {
                        Text("Showing what FERB imported from \(MemoryImportSource(rawValue: source)?.title.replacingOccurrences(of: "…", with: "") ?? source)")
                        Spacer()
                        Button("Show everything") { store.memoryFilter = nil; Task { await store.loadMemory() } }
                    }
                }
            }
            WindowSectionLabel(title: "Everything FERB remembers").padding(.top, 12)
            if store.activeMemory.isEmpty { Text("FERB doesn't know anything about you yet.").foregroundStyle(.secondary) }
            ForEach(store.activeMemory.filter { !$0.inProfile }) { item in WindowCard { row(item) } }
        }
        .alert("Forget this for good?", isPresented: Binding(get: { forgetting != nil },
                                                               set: { if !$0 { forgetting = nil } }),
               presenting: forgetting) { item in
            Button("Forget", role: .destructive) { Task { await store.forget(item); forgetting = nil } }
            Button("Cancel", role: .cancel) { forgetting = nil }
        } message: { _ in
            Text(willDelete.isEmpty ? "FERB deletes this memory." : "FERB deletes: " + willDelete.joined(separator: "; "))
        }
    }

    /// Import from other assistants: pick a source (an export file or folder for the account exports), read the
    /// estimate, Start; progress while it runs, Cancel. The facts arrive as FERB's understanding (DMN-6).
    @ViewBuilder private var importSection: some View {
        WindowCard { VStack(alignment: .leading, spacing: 12) {
            WindowSectionLabel(title: "Import from other assistants")
            HStack {
                Menu("Import from…") {
                    ForEach(MemoryImportSource.allCases) { source in
                        Button(source.title) { Task { await choose(source) } }
                    }
                }
                .disabled(store.importStatus?.running ?? false)
                Spacer()
            }
            if let estimate = store.importEstimate {
                Text(estimate.line).font(.caption)
                HStack {
                    Button("Start import") { Task { await store.startImport() } }
                    Button("Cancel") { store.importEstimate = nil }
                }
            }
            if let status = store.importStatus {
                HStack {
                    Text(status.line).font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    if status.running { Button("Cancel import") { Task { await store.cancelImport() } } }
                }
            }
        } }
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                if store.importStatus?.running ?? false { await store.loadImportStatus() }
            }
        }
    }

    private func choose(_ source: MemoryImportSource) async {
        guard source.needsFile else { await store.estimateImport(source); return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.message = "Choose the \(source.title.replacingOccurrences(of: "…", with: "")) file or its unzipped folder"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        await store.estimateImport(source, path: url.path)
    }

    /// One memory: its words, where it came from, what it is about; Always on / Not always on, Not right (stops using
    /// it, kept as history), Forget… (asks with the daemon's list first). Correct it by telling FERB.
    @ViewBuilder private func row(_ item: MemoryRow) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 5) {
                Text(item.content).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                Text([item.label, item.about].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let correctMemory {
                Button("Correct in chat…") {
                    correctMemory(item.content)
                }
                .buttonStyle(.plain).font(.system(size: 12, weight: .medium))
            }
            HStack(spacing: 8) {
                Button(item.inProfile ? "Not always on" : "Always on") {
                    Task { await store.setInProfile(item, !item.inProfile) }
                }
                Button("Not right") { Task { await store.retract(item) } }
                Button("Forget…") {
                    Task {
                        if let list = await store.forgetPreview(item) { willDelete = list; forgetting = item }
                    }
                }
            }
        }
    }
}

private struct AnswerHeight: PreferenceKey {
    static var defaultValue: [Bool: CGFloat] = [:]
    static func reduce(value: inout [Bool: CGFloat], nextValue: () -> [Bool: CGFloat]) {
        value.merge(nextValue(), uniquingKeysWith: { _, new in new })
    }
}

/// Keep the compact preview, with an in-place way to read the entire result.
struct ExpandableAnswer: View {
    let text: String
    let lines: Int
    @State private var expanded = false
    @State private var truncated = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(text).lineLimit(expanded ? nil : lines).textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .background(GeometryReader { proxy in
                    Color.clear.preference(key: AnswerHeight.self, value: [false: proxy.size.height])
                })
                .background(alignment: .topLeading) {
                    Text(text).fixedSize(horizontal: false, vertical: true)
                        .background(GeometryReader { proxy in
                            Color.clear.preference(key: AnswerHeight.self, value: [true: proxy.size.height])
                        })
                        .hidden().accessibilityHidden(true)
                }
            if truncated || expanded {
                Button(expanded ? "Show less" : "Show full answer") { expanded.toggle() }
                    .buttonStyle(.plain).font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.white.opacity(0.8))
            }
        }
        .onPreferenceChange(AnswerHeight.self) { heights in
            if !expanded, let full = heights[true], let visible = heights[false] { truncated = full > visible + 1 }
        }
    }
}
