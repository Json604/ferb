import AppKit
import SwiftUI
import NotchCore

/// Settings → Stored data (owner STORE-422; agent STORE-423, STORE-424, STORE-425): everything FERB keeps in its
/// database, read only, grouped for a person with every raw table one click away; a row opened in full; a search across
/// the tables; what the planner would read on its next turn; and the owner's deletes (a turn, a memory) behind a
/// confirmation that says what goes and what stays. Functional; the owner polishes it.
@MainActor
final class StoredDataModel: ObservableObject {
    enum Shown: Equatable {
        case nothing
        case table(StoredPage, filtered: Bool)
        case row(StoredFullRow)
        case search(StoredSearch)
        case context(StoredContext)
    }

    @Published private(set) var tables: StoredTables?
    @Published private(set) var shown: Shown = .nothing
    @Published private(set) var loading = false
    @Published private(set) var problem: String?
    @Published private(set) var note: String?
    /// The delete the owner is asked about (its preview), until Yes or Cancel.
    @Published var asking: StoredDeletion?
    /// A memory the owner is asked to forget, until Yes or Cancel.
    @Published var forgetting: String?
    @Published var showInternal = false
    /// Where Back goes from a row: the page or the search it was opened from.
    private var back: Shown = .nothing
    private let rpc: any DaemonRPCTransport
    /// The chat this app holds drops a deleted turn (`NotchModel.removeTurns`).
    var turnDeleted: ((String) -> Void)?
    /// FERB.app's own chat starts a fresh conversation (`NotchModel.startFreshConversation`); nil when it can't here.
    var freshConversation: (() async -> String?)?

    init(rpc: any DaemonRPCTransport) {
        self.rpc = rpc
    }

    private func ask(_ method: String, _ params: [String: Any] = [:]) async -> [String: Any]? {
        loading = true
        defer { loading = false }
        do {
            let reply = try await rpc.call(method, params: params)
            problem = nil
            return reply
        } catch {
            problem = error.localizedDescription
            return nil
        }
    }

    func loadTables() async {
        if let reply = await ask("data.tables"), let tables = StoredTables(reply) { self.tables = tables }
    }

    /// A table's newest rows; `modelCalls` shows only the model calls of the trace (what the AI was given).
    func open(_ table: String, modelCalls: Bool = false) async {
        note = nil
        var params: [String: Any] = ["table": table, "limit": 50]
        if modelCalls { params["filter"] = ["column": "kind", "starts_with": "model."] }
        if let reply = await ask("data.rows", params), let page = StoredPage(reply) {
            shown = .table(page, filtered: modelCalls)
        }
    }

    func loadMore() async {
        guard case let .table(page, filtered) = shown, let next = page.next else { return }
        var params: [String: Any] = ["table": page.table, "limit": 50, "cursor": next]
        if filtered { params["filter"] = ["column": "kind", "starts_with": "model."] }
        if let reply = await ask("data.rows", params), let more = StoredPage(reply) {
            var page = page
            page.append(more)
            shown = .table(page, filtered: filtered)
        }
    }

    func openRow(table: String, rowid: Int?) async {
        guard let rowid else { note = "This table's rows can only be read in its list."; return }
        if case .row = shown {} else { back = shown }
        if let reply = await ask("data.row", ["table": table, "rowid": rowid]), let row = StoredFullRow(reply) {
            shown = .row(row)
        }
    }

    func goBack() { shown = back }

    /// The rest of a cut cell, a piece at a time.
    func more(_ column: String) async {
        guard case var .row(row) = shown, let (text, left) = row.shown(column), left > 0 else { return }
        if let reply = await ask("data.cell", ["table": row.table, "rowid": row.rowid, "column": column,
                                               "offset": text.count, "chars": 100_000]) {
            row.add(piece: reply)
            shown = .row(row)
        }
    }

    func search(_ text: String) async {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        if let reply = await ask("data.search", ["text": text]), let found = StoredSearch(reply) {
            shown = .search(found)
            back = shown
        }
    }

    func openContext() async {
        if let reply = await ask("data.context"), let context = StoredContext(reply) { shown = .context(context) }
    }

    /// What deleting this turn removes and keeps, for the owner's Yes.
    func prepareDelete(_ action: StoredAction) async {
        switch action.kind {
        case "forget": forgetting = action.id
        default:
            if let reply = await ask("data.delete", ["request_id": action.id]) { asking = StoredDeletion(reply) }
        }
    }

    func confirmDelete() async {
        guard let preview = asking else { return }
        asking = nil
        guard let reply = await ask("data.delete", ["request_id": preview.requestID, "confirm": true]),
              let done = StoredDeletion(reply) else { return }
        turnDeleted?(done.requestID)
        note = "Deleted: " + (done.removes.isEmpty ? "nothing was left." : done.removes.joined(separator: "; ")) + "."
        await refreshAfterChange()
    }

    func confirmForget() async {
        guard let id = forgetting else { return }
        forgetting = nil
        guard await ask("memory.forget", ["id": id, "confirm": true]) != nil else { return }
        note = "Forgotten."
        await refreshAfterChange()
    }

    func startFresh() async {
        guard let fresh = freshConversation else { return }
        if let problem = await fresh() { self.problem = problem; return }
        note = "A fresh conversation started: FERB no longer reads the earlier messages. They stay in your chat history."
    }

    private func refreshAfterChange() async {
        await loadTables()
        switch back {
        case let .table(page, filtered): await open(page.table, modelCalls: filtered); back = shown
        case let .search(found): await search(found.text)
        default: shown = .nothing
        }
    }
}

struct StoredDataSettingsPage: View {
    @ObservedObject var model: StoredDataModel
    @State private var query = ""
    @State private var confirmFresh = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                TextField("Find words anywhere in FERB's data", text: $query)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { Task { await model.search(query) } }
                Button("Search") { Task { await model.search(query) } }.disabled(query.isEmpty)
                if model.freshConversation != nil {
                    Button("Start a fresh conversation…") { confirmFresh = true }
                }
            }
            if let problem = model.problem { Text(problem).foregroundStyle(.red).textSelection(.enabled) }
            if let note = model.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
            HStack(alignment: .top, spacing: 14) {
                sidebar.frame(width: 210)
                detail.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
        .padding(.horizontal, 30).padding(.bottom, 20)
        .task { await model.loadTables() }
        .alert("Delete this turn?", isPresented: Binding(get: { model.asking != nil }, set: { if !$0 { model.asking = nil } }),
               presenting: model.asking) { _ in
            Button("Delete", role: .destructive) { Task { await model.confirmDelete() } }
            Button("Cancel", role: .cancel) { model.asking = nil }
        } message: { preview in
            Text(preview.question)
        }
        .alert("Forget this memory?", isPresented: Binding(get: { model.forgetting != nil },
                                                           set: { if !$0 { model.forgetting = nil } })) {
            Button("Forget", role: .destructive) { Task { await model.confirmForget() } }
            Button("Cancel", role: .cancel) { model.forgetting = nil }
        } message: {
            Text("FERB deletes it and where it came from, and won't use it again. This can't be undone.")
        }
        .alert("Start a fresh conversation?", isPresented: $confirmFresh) {
            Button("Start fresh") { Task { await model.startFresh() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("FERB stops reading your earlier messages when it plans, and your chat shows where. They stay in " +
                 "your chat history and here; delete a turn to remove it. What FERB remembers about you (Memory) stays.")
        }
    }

    private var sidebar: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                if let tables = model.tables {
                    ForEach(tables.groups) { group in
                        WindowSectionLabel(title: group.title)
                        if group.id == "model_input" {
                            link("Next turn: what FERB would read now") { await model.openContext() }
                            link("Model calls (each with what it was given)") { await model.open("spans", modelCalls: true) }
                        }
                        ForEach(tables.tables(in: group.id, internal: model.showInternal)) { table in
                            link("\(table.name)  \(table.rows)") { await model.open(table.name) }
                        }
                    }
                    Toggle("Show internal tables (\(tables.hiddenCount))", isOn: $model.showInternal)
                        .font(.caption).padding(.top, 10)
                } else {
                    ProgressView()
                }
            }
        }
    }

    private func link(_ title: String, _ action: @escaping () async -> Void) -> some View {
        Button { Task { await action() } } label: {
            Text(title).font(.system(size: 12)).frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain).padding(.vertical, 3)
    }

    @ViewBuilder private var detail: some View {
        switch model.shown {
        case .nothing:
            Text("Choose a table, or search. Everything FERB stores is here as it is stored; secrets were removed when " +
                 "they were saved.").foregroundStyle(.secondary)
        case let .table(page, _):
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(page.table) · \(page.total) rows, newest first").bold()
                    if !page.note.isEmpty { Text(page.note).font(.caption).foregroundStyle(.secondary) }
                    Text(page.columns.joined(separator: " · ")).font(.caption.monospaced()).foregroundStyle(.secondary)
                    ForEach(page.rows) { row in
                        Button { Task { await model.openRow(table: page.table, rowid: row.rowid) } } label: {
                            Text(page.summary(row)).font(.system(size: 12, design: .monospaced)).lineLimit(2)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }.buttonStyle(.plain)
                        Divider()
                    }
                    if page.next != nil { Button("Load more") { Task { await model.loadMore() } } }
                }
            }
        case let .row(row):
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Button("Back") { model.goBack() }
                        Text("\(row.table) · row \(row.rowid)").bold()
                        Spacer()
                        if let action = row.action {
                            Button(action.title, role: .destructive) { Task { await model.prepareDelete(action) } }
                        }
                    }
                    if !row.note.isEmpty { Text(row.note).font(.caption).foregroundStyle(.secondary) }
                    ForEach(Array(row.columns.enumerated()), id: \.offset) { index, column in
                        VStack(alignment: .leading, spacing: 3) {
                            Text(column).font(.caption.bold()).foregroundStyle(.secondary)
                            Text(verbatim: cellText(row.cells[index]))
                                .font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                            if let left = row.shown(column)?.left, left > 0 {
                                Button("Show more (\(left) more characters)") { Task { await model.more(column) } }
                            }
                        }
                    }
                }
            }
        case let .search(found):
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    Text("“\(found.text)”: \(found.hits.count) found" + (found.stopped ? " (the search stopped early; not every table was read)" : ""))
                        .bold()
                    ForEach(found.hits) { hit in
                        Button { Task { await model.openRow(table: hit.table, rowid: hit.rowid) } } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("\(hit.table) · \(hit.column)").font(.caption.bold())
                                Text(hit.snippet).font(.system(size: 12, design: .monospaced)).lineLimit(3)
                            }.frame(maxWidth: .infinity, alignment: .leading)
                        }.buttonStyle(.plain)
                        Divider()
                    }
                }
            }
        case let .context(context):
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    Text("The conversation FERB would read on its next turn (\(context.conversation))").bold()
                    if context.blocks.isEmpty { Text("Nothing: a fresh conversation.").foregroundStyle(.secondary) }
                    ForEach(context.blocks) { block in
                        Text(verbatim: block.text).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                        if !block.origins.isEmpty {
                            Text("from: " + block.origins.joined(separator: ", ")).font(.caption).foregroundStyle(.secondary)
                        }
                        Divider()
                    }
                }
            }
        }
    }

    private func cellText(_ cell: StoredCell) -> String {
        if case let .text(t) = cell { return StoredData.readable(t) }
        return cell.short
    }
}
