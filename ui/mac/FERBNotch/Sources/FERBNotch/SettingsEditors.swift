import SwiftUI
import NotchCore

/// Keep unsaved words until the daemon accepts them, including across category changes.
@MainActor
final class SettingsListDraft: ObservableObject {
    @Published var text = ""
    @Published private(set) var saving = false
    @Published private(set) var problem: String?
    var canAdd: Bool { !saving && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    func add(to items: [String], key: String, store: FERBWindowStore) async {
        guard canAdd else { return }
        let submitted = text
        let value = submitted.trimmingCharacters(in: .whitespacesAndNewlines)
        if await save(items.contains(value) ? items : items + [value], key: key, store: store), text == submitted {
            text = ""
        }
    }

    @discardableResult
    func save(_ items: [String], key: String, store: FERBWindowStore) async -> Bool {
        guard !saving else { return false }
        saving = true
        problem = nil
        defer { saving = false }
        let saved = await store.set(key, .strings(items))
        if !saved { problem = store.problem ?? "Couldn't save. Try again." }
        return saved
    }
}

struct StringListEditor: View {
    let key: String
    let placeholder: String
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var draft: SettingsListDraft
    private var items: [String] { store.settings.strings(key) }

    var body: some View {
        ForEach(items, id: \.self) { item in
            HStack {
                Text(item).textSelection(.enabled)
                Spacer()
                Button("Remove") { Task { await draft.save(items.filter { $0 != item }, key: key, store: store) } }
                    .disabled(draft.saving)
            }
        }
        HStack {
            TextField(placeholder, text: $draft.text).onSubmit(add)
            Button(draft.saving ? "Saving…" : "Add", action: add).disabled(!draft.canAdd)
        }
        if let problem = draft.problem { Text(problem).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
    }

    private func add() { Task { await draft.add(to: items, key: key, store: store) } }
}

@MainActor
final class SettingsDaysDraft: ObservableObject {
    @Published var text = ""
    @Published private(set) var saving = false
    @Published private(set) var problem: String?

    func reset(to days: Int) {
        text = String(days)
        problem = nil
    }

    func commit(key: String, store: FERBWindowStore) async {
        guard !saving else { return }
        guard let days = Int(text.trimmingCharacters(in: .whitespacesAndNewlines)), days >= 1 else {
            problem = "Enter a whole number of days, at least 1."
            return
        }
        problem = nil
        guard days != store.settings.int(key) else { reset(to: days); return }
        let submitted = text
        saving = true
        defer { saving = false }
        if await store.set(key, .int(days)) {
            if text == submitted { reset(to: store.settings.int(key)) }
        } else { problem = store.problem ?? "Couldn't save. Try again." }
    }
}

struct DaysField: View {
    let title: String
    let key: String
    @ObservedObject var store: FERBWindowStore
    @StateObject private var draft = SettingsDaysDraft()
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            HStack(spacing: 10) {
                Text(title)
                Spacer(minLength: 16)
                TextField(title, text: $draft.text)
                    .labelsHidden().multilineTextAlignment(.trailing).monospacedDigit()
                    .frame(width: 96).focused($focused).disabled(draft.saving)
                    .accessibilityLabel("\(title), days")
                    .onSubmit(commit)
                    .onExitCommand { draft.reset(to: store.settings.int(key)); focused = false }
                Text("days").foregroundStyle(.secondary).frame(width: 32, alignment: .leading)
            }
            if let problem = draft.problem {
                Text(problem).font(.caption).foregroundStyle(.red)
            } else if draft.saving {
                Text("Saving…").font(.caption).foregroundStyle(.secondary)
            }
        }
        .onAppear { draft.reset(to: store.settings.int(key)) }
        .onChange(of: focused) { _, active in if !active { commit() } }
        .onChange(of: store.settings.int(key)) { _, days in
            if !focused && !draft.saving && draft.problem == nil { draft.reset(to: days) }
        }
    }

    private func commit() { Task { await draft.commit(key: key, store: store) } }
}
