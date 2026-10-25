import SwiftUI
import NotchCore

private func when(_ date: Date?) -> String { date?.formatted(date: .abbreviated, time: .shortened) ?? "" }
private func words(_ status: String) -> String { status.replacingOccurrences(of: "_", with: " ") }

struct TasksPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Tasks").font(.title2)
                Spacer()
                if store.stopped {
                    Text("FERB is stopped").foregroundStyle(.secondary)
                    Button("Resume FERB") { Task { await store.resumeEverything() } }
                } else {
                    Button("Stop everything") { Task { await store.stopEverything() } }
                }
                Button("Refresh") { Task { await store.loadTasks() } }
            }
            List {
                if store.tasks.isEmpty { Text("No tasks yet.").foregroundStyle(.secondary) }
                ForEach(store.tasks) { row in TaskHistoryRowView(row: row, store: store) }
                if store.moreTasks { Button("Show older tasks") { Task { await store.loadTasks(more: true) } } }
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
            Section("Waiting for you to confirm") {
                if store.proposedMemory.isEmpty { Text("Nothing waiting.").foregroundStyle(.secondary) }
                ForEach(store.proposedMemory) { item in
                    HStack {
                        Text(item.content).textSelection(.enabled)
                        Spacer()
                        Button("Confirm") { Task { await store.confirm(item) } }
                        Button("Not right") { Task { await store.retract(item) } }
                    }
                }
            }
            Section("What FERB knows") {
                if store.activeMemory.isEmpty { Text("FERB doesn't know anything about you yet.").foregroundStyle(.secondary) }
                ForEach(store.activeMemory) { item in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(item.content).textSelection(.enabled)
                            Text("\(item.kind) · \(item.subject)").font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Forget…") {
                            Task {
                                if let list = await store.forgetPreview(item) { willDelete = list; forgetting = item }
                            }
                        }
                    }
                }
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
}
