import AppKit
import SwiftUI
import NotchCore

/// The FERB window's sections (L34 Phase E). The notch stays the quick interface; this window holds the detail.
enum FERBSection: Hashable, CaseIterable, Identifiable {
    case tasks, schedules, noticed, memory, settings

    var id: FERBSection { self }

    var title: String {
        switch self {
        case .tasks: return "Tasks"
        case .schedules: return "Schedules & Reminders"
        case .noticed: return "Noticed"
        case .memory: return "Memory"
        case .settings: return "Settings"
        }
    }

    var symbol: String {
        switch self {
        case .tasks: return "checklist"
        case .schedules: return "calendar"
        case .noticed: return "eye"
        case .memory: return "brain"
        case .settings: return "gearshape"
        }
    }
}

/// The FERB window's data (L34 Phase E): each page reads the daemon when it opens, and each button is one RPC. Nothing
/// here decides anything; a refusal is shown as FERB's own words.
@MainActor
final class FERBWindowStore: ObservableObject {
    /// Seconds the app waits: a runtime's tiny check call, a diagnostic file's health check, the full self-test.
    static let runtimeWait = 60
    static let diagnosticWait = 60
    static let healthWait = 120

    let rpc: DaemonRPCTransport
    let records = TaskRecordFiles()
    /// Opens a file the owner asked to see (Inspect); tests replace it.
    var open: (URL) -> Void = { NSWorkspace.shared.open($0) }

    @Published var tasks: [TaskHistoryRow] = []
    @Published var moreTasks = false
    @Published var schedules: [ScheduleRow] = []
    @Published var reminders: [ReminderRow] = []
    @Published var notices: [NoticeRow] = []
    @Published var activeMemory: [MemoryRow] = []
    @Published var coreProfile = CoreProfileView()
    @Published var importStatus: MemoryImportStatus?
    @Published var importEstimate: MemoryImportEstimate?
    /// The Memory page shows only facts imported from this source (a memory import notice's page card); nil: all.
    @Published var memoryFilter: String?
    // Step 12c: Settings → About and Settings → Data.
    @Published var about: AboutDocument?
    @Published var restoreRows: [RestoreRow] = []
    @Published var restoreLine: String?
    @Published var savedSecrets: [SavedSecretRow] = []
    @Published var settings = SettingsSnapshot()
    @Published var banners: [String: Bool] = [:]
    @Published var health: [HealthLine] = []
    @Published var stopped = false
    @Published var recordingPaused = false
    @Published var watchers: [String] = []
    /// Why the last request failed, in FERB's words (or the connection's); nil after a request that worked.
    @Published var problem: String?
    /// The result of the last action, one line ("Paused: …", "codex answered.").
    @Published var note: String?
    /// Where Send diagnostics to developer stands, under its button: prepared, sent, or not sent and kept.
    @Published var diagnostic: ReportState?

    init(rpc: DaemonRPCTransport) {
        self.rpc = rpc
    }

    /// One RPC. A failure becomes the problem line and nil; never a crash or an empty list that reads as "nothing".
    @discardableResult
    func ask(_ method: String, _ params: [String: Any] = [:], wait: Int? = nil) async -> [String: Any]? {
        do {
            let reply: [String: Any]
            if let wait { reply = try await rpc.call(method, params: params, wait: wait) }
            else { reply = try await rpc.call(method, params: params) }
            problem = nil
            return reply
        } catch {
            problem = error.localizedDescription
            return nil
        }
    }

    func refresh(_ section: FERBSection) async {
        note = nil
        switch section {
        case .tasks: await loadTasks(); await loadStopState()
        case .schedules: await loadSchedules()
        case .noticed: await loadNotices()
        case .memory: await loadMemory()
        case .settings: await loadSettings()
        }
    }

    // MARK: Tasks

    func loadTasks(more: Bool = false) async {
        var params: [String: Any] = [:]
        if more, let last = tasks.last, !last.createdAtText.isEmpty { params["before"] = last.createdAtText }
        guard let reply = await ask("tasks.list", params) else { return }
        let rows = (reply["tasks"] as? [[String: Any]] ?? []).compactMap(TaskHistoryRow.init)
        tasks = more ? tasks + rows : rows
        moreTasks = reply["more"] as? Bool ?? false
    }

    func loadStopState() async {
        guard let reply = await ask("ping") else { return }
        stopped = reply["stopped"] as? Bool ?? false
    }

    func setVerdict(_ row: TaskHistoryRow, _ verdict: String) async {
        guard await ask("verdict.set", ["task_id": row.id, "verdict": verdict]) != nil else { return }
        await loadTasks()
    }

    /// Exactly this task: `task.stop` with no ids stops every running task.
    func stop(_ row: TaskHistoryRow) async {
        guard row.isRunning else { return }
        guard await ask("task.stop", ["task_ids": [row.id]]) != nil else { return }
        await loadTasks()
    }

    func stopEverything() async {
        guard await ask("stop.engage") != nil else { return }
        stopped = true
    }

    func resumeEverything() async {
        guard await ask("stop.lift") != nil else { return }
        stopped = false
    }

    func inspect(_ row: TaskHistoryRow) async {
        guard let trace = await ask("trace.get", ["task_id": row.id]) else { return }
        do { open(try await records.write(trace)) } catch { problem = error.localizedDescription }
    }

    // MARK: Schedules & Reminders

    func loadSchedules() async {
        if let reply = await ask("schedules.list") {
            schedules = (reply["schedules"] as? [[String: Any]] ?? []).compactMap(ScheduleRow.init)
        }
        if let reply = await ask("reminders.list") {
            reminders = (reply["pending"] as? [[String: Any]] ?? []).compactMap(ReminderRow.init)
        }
    }

    func change(_ schedule: ScheduleRow, _ action: String) async {
        guard let reply = await ask("schedules.change", ["id": schedule.id, "action": action]) else { return }
        await loadSchedules()
        note = reply["text"] as? String
    }

    func cancel(_ reminder: ReminderRow) async {
        guard await ask("reminders.cancel", ["id": reminder.id]) != nil else { return }
        await loadSchedules()
    }

    // MARK: Noticed

    func loadNotices() async {
        guard let reply = await ask("notices.list") else { return }
        notices = (reply["notices"] as? [[String: Any]] ?? []).compactMap(NoticeRow.init)
    }

    // MARK: Memory

    func loadMemory() async {
        var params: [String: Any] = ["status": "active"]
        if let memoryFilter { params["imported_from"] = memoryFilter }
        if let reply = await ask("memory.list", params) {
            activeMemory = (reply["items"] as? [[String: Any]] ?? []).compactMap(MemoryRow.init)
        }
        if let reply = await ask("memory.profile", [:]) { coreProfile = CoreProfileView(reply) }
        await loadImportStatus()
    }

    // MARK: Memory import (kb area memory)

    /// What the import would read and how many model calls it takes; nothing is sent yet.
    func estimateImport(_ source: MemoryImportSource, path: String = "") async {
        importEstimate = nil
        guard let reply = await ask("memory.import.estimate", ["source": source.rawValue, "path": path],
                                    wait: Self.runtimeWait) else { return }
        importEstimate = MemoryImportEstimate(source: source.rawValue, path: path, reply: reply)
    }

    func startImport() async {
        guard let estimate = importEstimate else { return }
        guard await ask("memory.import.start", ["source": estimate.source, "path": estimate.path],
                        wait: Self.runtimeWait) != nil else { return }
        importEstimate = nil
        await loadImportStatus()
    }

    func cancelImport() async {
        guard await ask("memory.import.cancel") != nil else { return }
        await loadImportStatus()
    }

    func loadImportStatus() async {
        guard let reply = await ask("memory.import.status") else { return }
        let wasRunning = importStatus?.running ?? false
        importStatus = MemoryImportStatus(reply["current"] as? [String: Any])
        if wasRunning && !(importStatus?.running ?? false) { await loadMemory() }
    }

    /// The core profile (always in FERB's prompts): add an item to it or take one out; the memory itself stays.
    func setInProfile(_ item: MemoryRow, _ on: Bool) async {
        guard await ask("memory.profile", ["op": on ? "add" : "remove", "id": item.id]) != nil else { return }
        await loadMemory()
    }

    func retract(_ item: MemoryRow) async {
        guard await ask("memory.retract", ["id": item.id, "reason": "the owner, in FERB's window"]) != nil else { return }
        await loadMemory()
    }

    /// Forget is permanent: first the daemon's own list of what would go, for the owner to read.
    func forgetPreview(_ item: MemoryRow) async -> [String]? {
        guard let reply = await ask("memory.forget", ["id": item.id]) else { return nil }
        return reply["will_delete"] as? [String] ?? []
    }

    /// Only after the owner's Forget on that list.
    func forget(_ item: MemoryRow) async {
        guard await ask("memory.forget", ["id": item.id, "confirm": true]) != nil else { return }
        await loadMemory()
    }

    // MARK: About (DMN-6)

    func loadAbout() async {
        if let reply = await ask("about.get", [:], wait: Self.runtimeWait) { about = AboutDocument(reply) }
    }

    // MARK: Data: Restore and saved secrets (APP-2)

    func loadRestore() async {
        guard let reply = await ask("recovery.list") else { return }
        restoreRows = (reply["records"] as? [[String: Any]] ?? []).compactMap(RestoreRow.init)
        let days = (reply["days"] as? NSNumber)?.intValue ?? 0
        let total = (reply["total"] as? NSNumber)?.intValue ?? restoreRows.count
        restoreLine = "Copies of changed files are kept \(days) days; items moved to the Trash while they are still there."
            + (total > restoreRows.count ? " Showing the newest \(restoreRows.count) of \(total)." : "")
    }

    /// Puts one back; never over a change made since (then the earlier version goes beside it, and the note says so).
    func restore(_ row: RestoreRow) async {
        guard let reply = await ask("recovery.restore", ["id": row.id]) else { return }
        let was = reply["original"] as? String ?? row.original, now = reply["restored_to"] as? String ?? ""
        note = reply["in_place"] as? Bool == true ? "Put back \(was)."
            : "\(was) has changed since, so the earlier version was put beside it as \(now)."
        await loadRestore()
    }

    func loadSecrets() async {
        guard let reply = await ask("secrets.saved") else { return }
        savedSecrets = (reply["entries"] as? [[String: Any]] ?? []).compactMap(SavedSecretRow.init)
    }

    /// Only after the owner's Delete in the one question the page asks (permanent).
    func deleteSecret(_ row: SavedSecretRow) async {
        guard await ask("secrets.delete", ["place": row.place, "name": row.name]) != nil else { return }
        note = "Deleted “\(row.name)” for \(row.place)."
        await loadSecrets()
    }

    // MARK: Settings (AI, Permissions and Browsers are plan C's SetupModel)

    func loadSettings() async {
        if let reply = await ask("settings.get") { settings = SettingsSnapshot(reply) }
        await loadBanners()
        if let reply = await ask("sense.status") {
            recordingPaused = reply["paused"] as? Bool ?? false
            watchers = reply["watchers"] as? [String] ?? []
        }
    }

    /// Shown at once; put back if FERB refuses, and then shows what FERB stored (it may normalize it).
    func set(_ key: String, _ value: SettingValue) async {
        let before = settings.values[key]
        settings.values[key] = value
        guard let reply = await ask("settings.set", ["key": key, "value": value.json]) else {
            settings.values[key] = before
            return
        }
        if let stored = reply["value"].flatMap(SettingValue.init) { settings.values[key] = stored }
    }

    func toggle(_ key: String) -> Binding<Bool> {
        Binding(get: { self.settings.bool(key) }, set: { on in Task { await self.set(key, .bool(on)) } })
    }

    /// Plan D's banner switches (config `[banners]`, through `notifications.categories`).
    func loadBanners() async {
        guard let reply = await ask("notifications.categories") else { return }
        banners = reply["categories"] as? [String: Bool] ?? [:]
    }

    func setBanner(_ category: BannerCategory, _ on: Bool) async {
        guard let reply = await ask("notifications.categories", ["set": [category.rawValue: on]]) else { return }
        banners = reply["categories"] as? [String: Bool] ?? banners
    }

    func banner(_ category: BannerCategory) -> Binding<Bool> {
        Binding(get: { self.banners[category.rawValue] ?? true },
                set: { on in Task { await self.setBanner(category, on) } })
    }

    func checkRuntime(_ name: String) async {
        guard let reply = await ask("runtimes.check", ["runtime": name], wait: Self.runtimeWait) else { return }
        let ok = reply["ok"] as? Bool ?? false
        note = ok ? "\(name) answered." : "\(name) didn't answer: \(reply["detail"] as? String ?? "no answer")"
    }

    func pauseRecording(_ paused: Bool) async {
        guard let reply = await ask(paused ? "sense.pause" : "sense.resume") else { return }
        recordingPaused = reply["paused"] as? Bool ?? paused
    }

    func checkHealth() async {
        guard let reply = await ask("doctor.check", ["selftest": true], wait: Self.healthWait) else { return }
        health = HealthLine.lines(reply)
    }

    /// Send diagnostics to developer (owner 2026-10-27): FERB writes the diagnostic file (secrets removed) and the
    /// page offers View and Send, as a failure report does; nothing leaves the Mac until the owner presses Send.
    func prepareDiagnostic(appVersion: String?) async {
        diagnostic = .preparing
        var params: [String: Any] = ["send": true]
        if let appVersion { params["app_version"] = appVersion }
        guard let reply = await ask("reports.diagnostic", params, wait: Self.diagnosticWait),
              let report = FailureReport(reply) else {
            diagnostic = .failed(problem ?? "FERB couldn't write the diagnostic file.")
            return
        }
        diagnostic = .ready(report)
    }

    /// Send, or Send again after it couldn't: one try (`reports.send`).
    func sendDiagnostic(appVersion: String?) async {
        guard let report = diagnostic?.report, diagnostic != .sending(report) else { return }
        diagnostic = .sending(report)
        diagnostic = await ReportSender.send(report, appVersion: appVersion, rpc: rpc)
    }

    func exportDiagnostic(appVersion: String?) async -> URL? {
        var params: [String: Any] = [:]
        if let appVersion { params["app_version"] = appVersion }
        guard let reply = await ask("reports.diagnostic", params, wait: Self.diagnosticWait),
              let path = reply["path"] as? String else { return nil }
        return URL(fileURLWithPath: path)
    }
}
