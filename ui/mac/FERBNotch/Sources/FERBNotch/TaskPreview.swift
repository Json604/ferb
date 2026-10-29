import AppKit
import SwiftUI
import NotchCore

/// One backpressured capture loop shared by the thumbnail, popover and detached player.
@MainActor
final class TaskPreview: ObservableObject {
    @Published private(set) var image: NSImage?
    @Published private(set) var status = "Waiting for a visual task…"
    private var readers = 0
    private var job: Task<Void, Never>?
    private let rpc: any DaemonRPCTransport
    private let taskID: String

    init(rpc: any DaemonRPCTransport, taskID: String) {
        self.rpc = rpc
        self.taskID = taskID
    }

    func acquire() {
        readers += 1
        guard job == nil else { return }
        job = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                do {
                    let frame = try await rpc.call("tasks.preview", params: ["task_id": taskID])
                    guard !Task.isCancelled else { return }
                    if let encoded = frame["image"] as? String,
                       let bytes = Data(base64Encoded: encoded), let decoded = NSImage(data: bytes) {
                        image = decoded
                        status = "Live"
                        await Task.yield()
                    } else {
                        image = nil
                        status = "Waiting for a visual task…"
                        try await Task.sleep(for: .seconds(1))
                    }
                } catch {
                    guard !Task.isCancelled else { return }
                    image = nil
                    status = "Live preview unavailable"
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
    }
}

struct TaskVideoView: View {
    @ObservedObject var preview: TaskPreview
    var compact = false
    var active = true
    @State private var subscribed = false

    var body: some View {
        ZStack {
            Color.black
            if let image = preview.image {
                Image(nsImage: image).resizable().aspectRatio(contentMode: compact ? .fill : .fit)
            } else if compact {
                ProgressView().controlSize(.mini)
            } else {
                Text(preview.status).font(.system(size: 12)).foregroundStyle(.secondary)
            }
        }
        .clipped()
        .onAppear { subscribe(active) }
        .onChange(of: active) { _, value in subscribe(value) }
        .onDisappear { subscribe(false) }
        .accessibilityLabel(preview.status)
    }
    private func subscribe(_ value: Bool) {
        guard subscribed != value else { return }
        subscribed = value
        if value { preview.acquire() } else { preview.release() }
    }

}
