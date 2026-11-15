import AppKit
import SwiftUI
import NotchCore

/// The running tasks by "N active tasks" (APP-900, superseding APP-899's ring: the owner kept the earlier circles, "i
/// wnat previous with your approached + adn exapn list"): at most two round live pictures with their amber outline and
/// dot, then a "+N" chip that opens the others as a plain list. A click on a picture shows its short view (the larger
/// picture, its line, Detach activity).
struct TaskActivityBubble: View {
    @ObservedObject var model: NotchModel
    @State private var hoveredTaskID: String?
    @State private var listOpen = false

    /// The round picture's size.
    static let size: CGFloat = 27

    /// A task waits for the owner while a question or a Yes of its own is open (`needs_user`, TaskItem).
    static func waits(_ item: TaskItem) -> Bool { item.status == "needs_user" }

    /// "Amazon order, waiting for you" / "Amazon order, paused" / "Amazon order, working".
    static func label(_ item: TaskItem) -> String {
        item.title + (waits(item) ? ", waiting for you" : item.isPaused ? ", paused" : ", working")
    }

    /// APP-891: a preview is shown once: a task whose card rows show their live pictures on screen now has no picture
    /// here (the work row's count and time still count it). A paused task keeps its picture as it was (APP-901).
    private var activeTasks: [TaskItem] {
        model.taskItems.filter { $0.isUnderway && !model.rowsShowPreview(of: $0.backendID ?? $0.id) }
    }
    private var featured: [TaskItem] { Array(activeTasks.prefix(2)) }
    private var remaining: [TaskItem] { Array(activeTasks.dropFirst(2)) }
    /// Live only while someone can see it (APP-403, APP-407): the notch open and not stepped aside.
    private var live: Bool { model.expanded && !model.isYielded }

    var body: some View {
        HStack(spacing: 6) {
            ForEach(featured) { item in
                taskButton(item)
                    .onHover { inside in if inside { hoveredTaskID = item.id } }
            }
            if !remaining.isEmpty {
                Button {
                    withAnimation(.easeOut(duration: 0.16)) { listOpen.toggle() }
                } label: {
                    Text("+\(remaining.count)")
                        .font(.system(size: 12, weight: .medium).monospacedDigit())
                        .padding(.horizontal, 9)
                        .frame(minWidth: Self.size, minHeight: Self.size)
                        .background(.white.opacity(listOpen ? 0.16 : 0.08), in: Capsule())
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .help("Show the other running tasks")
                .accessibilityLabel("\(remaining.count) more running \(remaining.count == 1 ? "task" : "tasks")")
                .popover(isPresented: $listOpen, arrowEdge: .bottom) {
                    otherTasks
                        .background(TaskPreviewWindow(model: model, hidesFromCapture: true))   // APP-406
                }
            }
        }
        .onChange(of: activeTasks.map(\.id)) { _, ids in
            model.discardFinishedPreviews()
            if let hoveredTaskID, !ids.contains(hoveredTaskID) { self.hoveredTaskID = nil }
            if remaining.isEmpty { listOpen = false }
        }
        .onChange(of: model.previewDismissalRevision) { _, _ in
            hoveredTaskID = nil
            listOpen = false
        }
        .onChange(of: model.expanded) { _, expanded in
            if !expanded { hoveredTaskID = nil; listOpen = false }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Task activity")
    }

    private func taskButton(_ item: TaskItem) -> some View {
        Button { hoveredTaskID = item.id } label: {
            // Live while visible (APP-403): not while the notch is closed or stepped aside for another window.
            TaskVideoView(preview: model.preview(for: item), compact: true, active: live)
                .frame(width: Self.size, height: Self.size)
                .clipShape(Circle())
                .overlay(Circle().stroke(Palette.amber.opacity(0.65), lineWidth: 1))
                .overlay(alignment: .bottomTrailing) {
                    Circle().fill(Palette.amber).frame(width: 6, height: 6).offset(x: 1, y: 1)
                }
        }
        .buttonStyle(.plain)
        .popover(isPresented: Binding(get: { hoveredTaskID == item.id },
                                     set: { if !$0, hoveredTaskID == item.id { hoveredTaskID = nil } }), arrowEdge: .bottom) {
            TaskShortView(model: model, subject: PreviewSubject(taskID: item.id)) { hoveredTaskID = nil }
                .background(TaskPreviewWindow(model: model, hidesFromCapture: true))
        }
        .help(item.title)
        .accessibilityLabel(Self.label(item))
    }

    /// The tasks past the first two, one plain row each: its picture, its name, what it does now (amber while it waits
    /// for the owner) and its own window.
    var otherTasks: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(remaining) { item in
                let waiting = Self.waits(item)
                HStack(spacing: 10) {
                    // APP-890: a click shows it larger, closed as every enlarged picture is (APP-889)
                    EnlargeablePicture(model: model, label: "Live preview of " + item.title) {
                        TaskVideoView(preview: model.preview(for: item), compact: true, active: live)
                            .frame(width: 64, height: 40)
                            .clipShape(RoundedRectangle(cornerRadius: 7))
                    } large: {
                        TaskVideoView(preview: model.preview(for: item))
                            .frame(width: EnlargedPreview.size.width, height: EnlargedPreview.size.height)
                            .clipShape(RoundedRectangle(cornerRadius: 9))
                    }
                    VStack(alignment: .leading, spacing: 1) {
                        Text(item.title).font(.system(size: 12.5, weight: .medium)).lineLimit(1)
                        Text(item.currentLine).font(.system(size: 11.5))
                            .foregroundStyle(waiting ? Palette.amber : Palette.secondary).lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    DetachActivityButton(model: model, subject: PreviewSubject(taskID: item.id), title: item.title) {
                        hoveredTaskID = nil
                        listOpen = false
                    }
                }
                .padding(6)
                .accessibilityElement(children: .contain)
                .accessibilityLabel(Self.label(item))
            }
        }
        .padding(6)
        .frame(width: 320, alignment: .leading)
    }
}

/// What a live picture stands for (APP-912): a running task, or one step of it, as a card row or a checklist line shows
/// it (APP-888, `step`: the step task id it watches, `label`: the row's words). The short view, its Detach activity and
/// the detached window take it, so a row's picture and the round picture by "N active tasks" open the same views.
struct PreviewSubject: Equatable {
    let taskID: String
    var step: String? = nil
    var label: String? = nil

    private static let separator: Character = "\u{1F}"

    /// One activity window per subject (APP-898): the task's id, or the task's and its step's.
    var key: String {
        guard let step else { return taskID }
        return taskID + String(Self.separator) + step + String(Self.separator) + (label ?? "")
    }

    init(taskID: String, step: String? = nil, label: String? = nil) {
        self.taskID = taskID
        self.step = step
        self.label = label
    }

    init(key: String) {
        let parts = key.split(separator: Self.separator, maxSplits: 2, omittingEmptySubsequences: false).map(String.init)
        taskID = parts[0]
        step = parts.count > 1 ? parts[1] : nil
        label = parts.count > 2 && !parts[2].isEmpty ? parts[2] : nil
    }

    /// Its task among the app's tasks, by the task's own id or the daemon's.
    @MainActor func task(in model: NotchModel) -> TaskItem? {
        model.taskItems.first { $0.id == taskID || $0.backendID == taskID }
    }

    /// The live picture it shows: the task's (the step that acted last), or its step's.
    @MainActor func preview(in model: NotchModel, task: TaskItem) -> TaskPreview {
        guard let step else { return model.preview(for: task) }
        return model.preview(task: task.backendID ?? task.id, worker: step)
    }

    @MainActor func title(_ task: TaskItem) -> String { label ?? task.title }

    /// What FERB is doing now: the step's own live line, else the task's (APP-890's one line).
    @MainActor func currentLine(in model: NotchModel, task: TaskItem) -> String {
        if let step, let live = model.stepLive[step], !live.text.isEmpty { return RowDisplay.sentence(live.text) }
        return task.currentLine
    }
}

/// A live picture's short view (APP-890): the larger picture (enlarges on a click, APP-889), what FERB is doing now and
/// Detach activity. One view for the round picture by "N active tasks" and for a card row's or checklist line's picture
/// (APP-912).
struct TaskShortView: View {
    @ObservedObject var model: NotchModel
    let subject: PreviewSubject
    /// Closes the popover showing it (Detach activity opens the window instead).
    let dismiss: () -> Void

    var body: some View {
        if let task = subject.task(in: model) {
            let title = subject.title(task), preview = subject.preview(in: model, task: task)
            VStack(spacing: 8) {
                // APP-890: a click shows it larger, closed as every enlarged picture is (APP-889)
                EnlargeablePicture(model: model, label: "Live preview of " + title) {
                    TaskVideoView(preview: preview)
                        .frame(width: 300, height: 170)
                        .clipShape(RoundedRectangle(cornerRadius: 9))
                } large: {
                    TaskVideoView(preview: preview)
                        .frame(width: EnlargedPreview.size.width, height: EnlargedPreview.size.height)
                        .clipShape(RoundedRectangle(cornerRadius: 9))
                }
                HStack(alignment: .center, spacing: 9) {
                    if task.isPaused { PausedGlyph(size: 16) } else { ProgressView().controlSize(.small) }
                    VStack(alignment: .leading, spacing: 3) {
                        Text(title).font(.system(size: 11, weight: .medium)).lineLimit(1)
                        Text(subject.currentLine(in: model, task: task))
                            .font(.system(size: 10))
                            .foregroundStyle(TaskActivityBubble.waits(task) ? Palette.amber : Palette.secondary).lineLimit(1)
                    }
                    DetachActivityButton(model: model, subject: subject, title: title, then: dismiss)
                }
            }
            .padding(.horizontal, 11).padding(.vertical, 8)
            .frame(width: 322, alignment: .leading)
            .ferbGlass(in: RoundedRectangle(cornerRadius: 13))
            .overlay(RoundedRectangle(cornerRadius: 13).stroke(.white.opacity(0.12), lineWidth: 1))
            .shadow(color: .black.opacity(0.18), radius: 10, y: 4)
        }
    }
}

/// Detach activity (APP-898): the live picture in a window of its own that stays open.
struct DetachActivityButton: View {
    @ObservedObject var model: NotchModel
    let subject: PreviewSubject
    let title: String
    var then: () -> Void = {}

    var body: some View {
        Button {
            then()
            model.onDetachTaskActivity?(subject.key)
        } label: {
            Image(systemName: "arrow.up.forward.square")
                .font(.system(size: 12)).frame(width: 23, height: 24)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Open in its own window")
        .accessibilityLabel("Open \(title) in its own window")
    }
}

/// The detached task activity windows (APP-898, owner 2026-11-12: "i shdoul be able to of all the tasks running"): one
/// per running task. Detaching another task's activity opens its own window beside the ones open; detaching a task
/// whose window is open brings that window forward. Each closes on its own: its ✕, or its task ending
/// (DetachedTaskActivityView). Each is a TaskActivityPanel, so each is hidden from screen sharing and recordings
/// (APP-406) and lives in FERB's overlay space on the Space the owner is on (APP-796).
@MainActor
final class TaskActivityWindows {
    /// How far each further window opens from the last one, so none hides another exactly.
    static let cascade: CGFloat = 28

    private(set) var windows: [(taskID: String, window: NSWindow)] = []
    private let make: (_ taskID: String, _ close: @escaping () -> Void) -> NSWindow
    private let bringForward: (NSWindow) -> Void
    private let release: (NSWindow) -> Void

    /// `make` builds a task's window (its `close` closes it here); `bringForward` orders one in; `release` orders one
    /// out for good.
    init(make: @escaping (_ taskID: String, _ close: @escaping () -> Void) -> NSWindow,
         bringForward: @escaping (NSWindow) -> Void, release: @escaping (NSWindow) -> Void) {
        self.make = make
        self.bringForward = bringForward
        self.release = release
    }

    func window(for taskID: String) -> NSWindow? { windows.first { $0.taskID == taskID }?.window }

    func open(_ taskID: String) {
        if let window = window(for: taskID) {
            bringForward(window)
            return
        }
        let window = make(taskID) { [weak self] in self?.close(taskID) }
        if let last = windows.last?.window {
            window.setFrameOrigin(CGPoint(x: last.frame.minX + Self.cascade, y: last.frame.minY - Self.cascade))
        }
        windows.append((taskID, window))
        bringForward(window)
    }

    func close(_ taskID: String) {
        guard let index = windows.firstIndex(where: { $0.taskID == taskID }) else { return }
        let window = windows.remove(at: index).window
        release(window)
    }

    func closeAll() {
        for taskID in windows.map(\.taskID) { close(taskID) }
    }
}

final class TaskActivityPanel: NSPanel {
    init(model: NotchModel, taskID: String, onClose: @escaping () -> Void) {
        let size = CGSize(width: DetachedTaskActivityView.width, height: DetachedTaskActivityView.height)
        let screen = NSScreen.main?.visibleFrame ?? CGRect(x: 100, y: 100, width: 1200, height: 800)
        let origin = CGPoint(x: screen.midX - size.width / 2, y: screen.midY - size.height / 2)
        super.init(contentRect: CGRect(origin: origin, size: size),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        title = "FERB Task Activity"
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        // APP-782: it lives in FERB's overlay space, where the window server moves nothing (APP-759): its card drags it
        // itself (WindowDragArea), as the chat window's header does
        isMovable = false
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 1)
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        sharingType = .none             // hidden from screen sharing and recordings (APP-406)
        let view = DetachedTaskActivityView(model: model, taskID: taskID, onClose: onClose)
        let hosting = NSHostingView(rootView: view)
        hosting.sizingOptions = []
        contentView = hosting
    }
}

struct DetachedTaskActivityView: View {
    @ObservedObject var model: NotchModel
    /// Its window's key (PreviewSubject.key): a task's id, or a task's and one of its steps' (APP-912).
    let taskID: String
    let onClose: () -> Void
    /// The box's size: its title, the picture and the one line (APP-890), no list under them.
    static let width: CGFloat = 350
    static let height: CGFloat = 300

    private var subject: PreviewSubject { PreviewSubject(key: taskID) }
    private var task: TaskItem? { subject.task(in: model) }

    var body: some View {
        Group {
            if let task {
                let title = subject.title(task), preview = subject.preview(in: model, task: task)
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .top) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(task.isPaused ? "Paused" : "FERB is working").font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(task.isPaused ? Palette.chatBlue : Palette.amber)
                            Text(title).font(.system(size: 13, weight: .medium)).lineLimit(2)
                        }
                        Spacer()
                        Button(action: onClose) {
                            Image(systemName: "xmark").font(.system(size: 10, weight: .semibold))
                                .frame(width: 23, height: 23)
                        }.buttonStyle(.plain).help("Close activity")
                    }
                    // APP-890: a click shows it larger, closed as every enlarged picture is (APP-889)
                    EnlargeablePicture(model: model, label: "Live preview of " + title) {
                        TaskVideoView(preview: preview)
                            .frame(width: Self.width - 30, height: 170)
                            .clipShape(RoundedRectangle(cornerRadius: 9))
                    } large: {
                        TaskVideoView(preview: preview)
                            .frame(width: EnlargedPreview.size.width, height: EnlargedPreview.size.height)
                            .clipShape(RoundedRectangle(cornerRadius: 9))
                    }
                    // APP-890 (owner 2026-11-11: "lets keep it like normla only showing what command currently ran"):
                    // the one line of what FERB is doing now, as the short view says it; the whole list stays in Show
                    // activity and the Tasks page
                    HStack(alignment: .center, spacing: 9) {
                        if task.isPaused { PausedGlyph(size: 16) } else { ProgressView().controlSize(.small) }
                        Text(subject.currentLine(in: model, task: task)).font(.system(size: 11))
                            .foregroundStyle(Palette.secondary).lineLimit(2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(15)
                .background(WindowDragArea())                    // APP-782: any empty part of the card moves it
                .ferbGlass(in: RoundedRectangle(cornerRadius: 16))
                .overlay(RoundedRectangle(cornerRadius: 16).stroke(.white.opacity(0.14), lineWidth: 1))
            }
        }
        .onChange(of: model.taskItems) { _, _ in
            if !(task?.isUnderway ?? false) { onClose() }
        }
    }
}

/// A paused task's mark where a running one has its spinner (APP-901): pause.circle in the colour paused card rows
/// take (TurnCardView's surface tint for "paused").
struct PausedGlyph: View {
    let size: CGFloat

    var body: some View {
        Image(systemName: "pause.circle").font(.system(size: size)).foregroundStyle(Palette.chatBlue)
            .accessibilityLabel("Paused")
    }
}
