import AppKit
import SwiftUI
import NotchCore

/// Whether previews may be asked for now, shared by every preview loop (notes/preview.md): nobody can see one while the
/// displays sleep, the screen is locked or another user's session is in front; and once the daemon refuses
/// `tasks.preview` (not FERB.app, or a daemon from before previews) nobody asks again until it connects anew.
@MainActor
final class PreviewAccess: ObservableObject {
    @Published private(set) var gate = PreviewGate()
    @Published private(set) var refusal: PreviewRefusal?
    private var observers: [NSObjectProtocol] = []

    init() {
        let workspace = NSWorkspace.shared.notificationCenter
        func watch(_ center: NotificationCenter, _ name: Notification.Name, _ change: @escaping (inout PreviewGate) -> Void) {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { if let self { change(&self.gate) } }
            })
        }
        watch(workspace, NSWorkspace.screensDidSleepNotification) { $0.displaysAsleep = true }
        watch(workspace, NSWorkspace.screensDidWakeNotification) { $0.displaysAsleep = false }
        watch(workspace, NSWorkspace.sessionDidResignActiveNotification) { $0.sessionInactive = true }
        watch(workspace, NSWorkspace.sessionDidBecomeActiveNotification) { $0.sessionInactive = false }
        let distributed = DistributedNotificationCenter.default()
        watch(distributed, Notification.Name("com.apple.screenIsLocked")) { $0.screenLocked = true }
        watch(distributed, Notification.Name("com.apple.screenIsUnlocked")) { $0.screenLocked = false }
    }

    func refused(_ refusal: PreviewRefusal) { self.refusal = refusal }
    /// The daemon connected again (it may have been updated): previews are asked for again.
    func reconnected() { if refusal != nil { refusal = nil } }
}

/// One backpressured capture loop shared by the thumbnail, popover and detached player, for a task or one step of it
/// (`worker`: a card row's step task id).
@MainActor
final class TaskPreview: ObservableObject {
    @Published private(set) var image: NSImage?
    /// The latest answer: what FERB is doing now, drawn over the frozen frame.
    @Published private(set) var answer: PreviewAnswer?
    @Published private(set) var status = "Waiting for a visual task…"
    private var readers = 0
    private var job: Task<Void, Never>?
    private let rpc: any DaemonRPCTransport
    let taskID: String
    let worker: String?
    let access: PreviewAccess

    init(rpc: any DaemonRPCTransport, taskID: String, worker: String? = nil, access: PreviewAccess) {
        self.rpc = rpc
        self.taskID = taskID
        self.worker = worker
        self.access = access
    }

    /// The line over the frame (decision 2), or why there is no live picture.
    var overlay: String? {
        if let refusal = access.refusal { return refusal.text }
        return answer?.overlay
    }

    func acquire() {
        readers += 1
        guard job == nil else { return }
        job = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                // Nobody could see it: ask for nothing (no capture) until they can.
                guard self.access.refusal == nil, self.access.gate.mayCapture else {
                    if let refusal = self.access.refusal { self.status = refusal.text }
                    do { try await Task.sleep(for: .seconds(1)) } catch { return }
                    continue
                }
                var params: [String: Any] = ["task_id": self.taskID]
                if let worker = self.worker { params["worker"] = worker }
                do {
                    let frame = try await self.rpc.call("tasks.preview", params: params)
                    guard !Task.isCancelled else { return }
                    let answer = PreviewAnswer(frame)
                    if let encoded = frame["image"] as? String,
                       let bytes = Data(base64Encoded: encoded), let decoded = NSImage(data: bytes) {
                        self.image = decoded
                    } else if answer.dropsLastFrame {
                        self.image = nil
                    }                           // otherwise the last frame stays, frozen, with the line over it
                    self.answer = answer
                    self.status = self.image == nil ? answer.emptyText : (answer.overlay ?? "Live")
                    let delay = answer.nextRequestDelay
                    if delay == .zero { await Task.yield() } else { try await Task.sleep(for: delay) }
                } catch {
                    guard !Task.isCancelled else { return }
                    if let code = (error as? DaemonRPCError)?.code, let refusal = PreviewRefusal(code: code) {
                        self.access.refused(refusal)
                        self.status = refusal.text
                        continue
                    }
                    // A brief failure keeps the last frame.
                    self.status = "Live preview unavailable"
                    try? await Task.sleep(for: .seconds(1))
                }
            }
        }
    }

    func release() {
        readers = max(0, readers - 1)
        if readers == 0 { stop() }
    }

    func stop() {
        job?.cancel()
        job = nil
        image = nil
        answer = nil
    }
}

struct TaskVideoView: View {
    @ObservedObject var preview: TaskPreview
    @ObservedObject private var access: PreviewAccess
    var compact = false
    var active = true
    @State private var subscribed = false
    /// The window showing it isn't covered (notes/preview.md: capture only while someone can see it).
    @State private var windowVisible = true

    init(preview: TaskPreview, compact: Bool = false, active: Bool = true) {
        self.preview = preview
        self.access = preview.access
        self.compact = compact
        self.active = active
    }

    var body: some View {
        ZStack {
            Color.black
            if let image = preview.image {
                Image(nsImage: image).resizable().aspectRatio(contentMode: compact ? .fill : .fit)
            } else if compact {
                ProgressView().controlSize(.mini)
            } else {
                Text(preview.status).font(.system(size: 12)).foregroundStyle(.secondary)
                    .multilineTextAlignment(.center).padding(.horizontal, 12)
            }
        }
        .overlay(alignment: .bottom) {
            // What FERB is doing now, over its last frame of this window (decision 2).
            if !compact, preview.image != nil, let line = preview.overlay {
                Text(line)
                    .font(.system(size: 11, weight: .medium)).lineLimit(2)
                    .padding(.horizontal, 9).padding(.vertical, 5)
                    .background(.black.opacity(0.72), in: Capsule())
                    .padding(8)
            }
        }
        .clipped()
        .background(WindowVisibilityReader { visible in
            windowVisible = visible
            subscribe(active && visible)
        })
        .onAppear { subscribe(active && windowVisible) }
        .onChange(of: active) { _, value in subscribe(value && windowVisible) }
        .onDisappear { subscribe(false) }
        .accessibilityLabel(preview.overlay ?? preview.status)
    }

    private func subscribe(_ value: Bool) {
        guard subscribed != value else { return }
        subscribed = value
        if value { preview.acquire() } else { preview.release() }
    }
}

/// Reports whether the window holding this view is visible on screen (not covered by other windows, not ordered out).
struct WindowVisibilityReader: NSViewRepresentable {
    let changed: (Bool) -> Void

    func makeNSView(context: Context) -> Reader {
        let view = Reader()
        view.changed = changed
        return view
    }
    func updateNSView(_ nsView: Reader, context: Context) { nsView.changed = changed }

    final class Reader: NSView {
        var changed: ((Bool) -> Void)?
        private var observer: NSObjectProtocol?
        private var last: Bool?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let observer { NotificationCenter.default.removeObserver(observer) }
            observer = nil
            guard let window else { return }
            observer = NotificationCenter.default.addObserver(forName: NSWindow.didChangeOcclusionStateNotification,
                                                              object: window, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.report() }
            }
            report()
        }

        private func report() {
            guard let window else { return }
            let visible = window.occlusionState.contains(.visible)
            guard visible != last else { return }
            last = visible
            let changed = changed
            DispatchQueue.main.async { changed?(visible) }
        }

        deinit { if let observer { NotificationCenter.default.removeObserver(observer) } }
    }
}
