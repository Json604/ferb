import AppKit
import SwiftUI
import NotchCore

private func when(_ date: Date?) -> String { date?.formatted(date: .abbreviated, time: .shortened) ?? "" }
private func words(_ status: String) -> String { status.replacingOccurrences(of: "_", with: " ") }

struct TasksPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        List {
            if store.tasks.isEmpty { Text("No tasks yet.").foregroundStyle(.secondary) }
            ForEach(store.tasks) { row in TaskHistoryRowView(row: row, store: store) }
            if store.moreTasks { Button("Show older tasks") { Task { await store.loadTasks(more: true) } } }
        }
        .listStyle(.inset)
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                if store.stopped {
                    Text("FERB is stopped").foregroundStyle(.secondary)
                    Button("Resume FERB") { Task { await store.resumeEverything() } }
                } else {
                    Button("Stop everything") { Task { await store.stopEverything() } }
                }
                Button {
                    Task { await store.loadTasks() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .help("Refresh tasks")
            }
        }
    }
}

struct TaskHistoryRowView: View {
    let row: TaskHistoryRow
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(row.goal.isEmpty ? row.id : row.goal).bold().lineLimit(2)
                Spacer()
                Text(words(row.status)).foregroundStyle(.secondary)
            }
            Text(when(row.createdAt)).font(.caption).foregroundStyle(.secondary)
            if !row.answer.isEmpty { Text(row.answer).lineLimit(4).textSelection(.enabled) }
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
        List {
            Section("Schedules") {
                if store.schedules.isEmpty {
                    Text("No schedules. Ask FERB in the notch, e.g. “every weekday at 9, …”.").foregroundStyle(.secondary)
                }
                ForEach(store.schedules) { s in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(s.request).bold()
                        Text("\(s.rule) · \(s.state)").foregroundStyle(.secondary)
                        if let next = s.nextRun { Text("Next run: \(when(next))") }
                        if let end = s.until {
                            Text("Ends: " + (CountdownTarget.lastDay(of: end).map { "end of " + $0.formatted(date: .abbreviated, time: .omitted) } ?? when(end)))
                        }
                        if let last = s.lastResult {
                            Text("Last run (\(words(last.status))): \(last.answer)").lineLimit(3).textSelection(.enabled)
                        }
                        if s.failures > 0 { Text("Failed \(s.failures) time(s) in a row").foregroundStyle(.red) }
                        HStack {
                            ForEach(s.actions, id: \.self) { action in
                                Button(action.capitalized) { Task { await store.change(s, action) } }
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
            Section("Reminders") {
                if store.reminders.isEmpty { Text("No reminders waiting.").foregroundStyle(.secondary) }
                ForEach(store.reminders) { r in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(r.message)
                            Text(when(r.at)).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Cancel") { Task { await store.cancel(r) } }
                    }
                }
            }
        }
    }
}

struct NoticedPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        List {
            if store.notices.isEmpty { Text("FERB hasn't noticed anything yet.").foregroundStyle(.secondary) }
            ForEach(store.notices) { n in
                VStack(alignment: .leading, spacing: 4) {
                    Text(n.text).textSelection(.enabled)
                    // Outside text FERB quotes is data, shown as a quote (MP2)
                    ForEach(Array(n.quotes.enumerated()), id: \.offset) { _, quote in
                        Text("“\(quote.text)”").italic().foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Text(when(n.at)).font(.caption).foregroundStyle(.secondary)
                }
                .padding(.vertical, 4)
            }
        }
    }
}

struct MemoryPage: View {
    @ObservedObject var store: FERBWindowStore
    @State private var forgetting: MemoryRow?
    @State private var willDelete: [String] = []

    var body: some View {
        List {
            Section("Always on (core profile, \(store.coreProfile.usedTokens) of \(store.coreProfile.budgetTokens) tokens)") {
                if store.coreProfile.sections.isEmpty { Text("Nothing yet.").foregroundStyle(.secondary) }
                ForEach(store.coreProfile.sections, id: \.title) { section in
                    Text(section.title).font(.headline)
                    ForEach(section.ids, id: \.self) { id in
                        if let item = store.activeMemory.first(where: { $0.id == id }) { row(item) }
                    }
                }
            }
            importSection
            Section("Everything FERB remembers") {
                if store.activeMemory.isEmpty { Text("FERB doesn't know anything about you yet.").foregroundStyle(.secondary) }
                ForEach(store.activeMemory.filter { !$0.inProfile }) { item in row(item) }
            }
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
    /// estimate, Start; progress while it runs, Cancel. The facts arrive as FERB's understanding (decision 6).
    @ViewBuilder private var importSection: some View {
        Section("Import from other assistants") {
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
        }
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
        HStack {
            VStack(alignment: .leading) {
                Text(item.content).textSelection(.enabled)
                Text([item.label, item.about].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
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
