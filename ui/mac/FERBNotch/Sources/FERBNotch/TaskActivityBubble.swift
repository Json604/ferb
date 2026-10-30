import AppKit
import SwiftUI

struct TaskActivityBubble: View {
    @ObservedObject var model: NotchModel
    @State private var hoveredTaskID: String?
    @State private var ringOpen = false

    private var activeTasks: [TaskItem] { model.taskItems.filter(\.isRunning) }
    private var featured: [TaskItem] { Array(activeTasks.prefix(2)) }
    private var remaining: [TaskItem] { Array(activeTasks.dropFirst(2)) }

    var body: some View {
        HStack(spacing: 4) {
            ForEach(featured) { item in
                taskButton(item)
                    .onHover { inside in if inside { hoveredTaskID = item.id } }
            }
            if !remaining.isEmpty {
                Button {
                    withAnimation(.easeOut(duration: 0.16)) { ringOpen.toggle() }
                } label: {
                    Text("+")
                        .font(.system(size: 9, weight: .semibold).monospacedDigit())
                        .frame(width: 27, height: 27)
                        .background(.white.opacity(0.10), in: Circle())
                        .overlay(Circle().stroke(.white.opacity(0.16), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .help("Show other running tasks")
                .popover(isPresented: $ringOpen, arrowEdge: .bottom) {
                    taskRing.padding(12)
                        .background(TaskPreviewWindow(model: model, hidesFromCapture: true))
                }
                .zIndex(ringOpen ? 2 : 0)
            }
        }
        .onChange(of: activeTasks.map(\.id)) { _, ids in
            model.discardFinishedPreviews()
            if let hoveredTaskID, !ids.contains(hoveredTaskID) { self.hoveredTaskID = nil }
            if remaining.isEmpty { ringOpen = false }
        }
        .onChange(of: model.previewDismissalRevision) { _, _ in
            hoveredTaskID = nil
            ringOpen = false
        }
        .onChange(of: model.expanded) { _, expanded in
            if !expanded { hoveredTaskID = nil; ringOpen = false }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Task activity")
    }

    private func taskButton(_ item: TaskItem) -> some View {
        Button { hoveredTaskID = item.id } label: {
            // Live while visible (decision 1): not while the notch is closed or stepped aside for another window.
            TaskVideoView(preview: model.preview(for: item), compact: true, active: model.expanded && !model.isYielded)
                .frame(width: 27, height: 27)
                .clipShape(Circle())
                .overlay(Circle().stroke(Palette.amber.opacity(0.65), lineWidth: 1))
                .overlay(alignment: .bottomTrailing) {
                    Circle().fill(Palette.amber).frame(width: 6, height: 6).offset(x: 1, y: 1)
                }
        }
        .buttonStyle(.plain)
        .popover(isPresented: Binding(get: { hoveredTaskID == item.id },
                                     set: { if !$0, hoveredTaskID == item.id { hoveredTaskID = nil } }), arrowEdge: .bottom) {
            expandedBubble(for: item)
                .background(TaskPreviewWindow(model: model, hidesFromCapture: true))
        }
        .help(item.title)
        .accessibilityLabel("Activity for \(item.title)")
    }

    private var taskRing: some View {
        let radius = max(CGFloat(48), CGFloat(remaining.count) * 12)
        let diameter = radius * 2 + 44
        return ZStack {
            Circle().fill(.regularMaterial).overlay(Circle().stroke(.white.opacity(0.16), lineWidth: 1))
                .frame(width: diameter, height: diameter)
            Circle().stroke(.white.opacity(0.13), lineWidth: 1)
                .frame(width: radius * 2 - 4, height: radius * 2 - 4)
            ForEach(Array(remaining.enumerated()), id: \.element.id) { index, item in
                let angle = Double(index) * 2 * Double.pi / Double(max(remaining.count, 1)) - Double.pi / 2
                taskButton(item)
                    .onHover { inside in hoveredTaskID = inside ? item.id : hoveredTaskID }
                    .position(x: diameter / 2 + radius * CGFloat(cos(angle)),
                              y: diameter / 2 + radius * CGFloat(sin(angle)))
            }
            Image(systemName: "plus")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Palette.secondary)
                .position(x: diameter / 2, y: diameter / 2)
        }
        .frame(width: diameter, height: diameter)

    }

    private func expandedBubble(for item: TaskItem) -> some View {
        VStack(spacing: 8) {
            TaskVideoView(preview: model.preview(for: item))
                .frame(width: 300, height: 170)
                .clipShape(RoundedRectangle(cornerRadius: 9))
            HStack(alignment: .center, spacing: 9) {
            ProgressView().controlSize(.small)
            VStack(alignment: .leading, spacing: 3) {
                Text(item.title).font(.system(size: 11, weight: .medium)).lineLimit(1)
                Text(item.activity.last?.title ?? "Starting task…")
                    .font(.system(size: 10)).foregroundStyle(Palette.secondary).lineLimit(1)
            }
            Button {
                hoveredTaskID = nil
                ringOpen = false
                model.onDetachTaskActivity?(item.id)
            } label: {
                Image(systemName: "arrow.up.forward.square")
                    .font(.system(size: 12)).frame(width: 23, height: 24)
            }
            .buttonStyle(.plain)
            .help("Detach activity")
            .accessibilityLabel("Detach task activity")
        }
        }
        .padding(.horizontal, 11).padding(.vertical, 8)
        .frame(width: 322, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 13))
        .overlay(RoundedRectangle(cornerRadius: 13).stroke(.white.opacity(0.12), lineWidth: 1))
        .shadow(color: .black.opacity(0.18), radius: 10, y: 4)
    }
}

final class TaskActivityPanel: NSPanel {
    init(model: NotchModel, taskID: String, onClose: @escaping () -> Void) {
        let size = CGSize(width: 350, height: 370)
        let screen = NSScreen.main?.visibleFrame ?? CGRect(x: 100, y: 100, width: 1200, height: 800)
        let origin = CGPoint(x: screen.midX - size.width / 2, y: screen.midY - size.height / 2)
        super.init(contentRect: CGRect(origin: origin, size: size),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        title = "FERB Task Activity"
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        isMovableByWindowBackground = true
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 1)
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        sharingType = .none             // hidden from screen sharing and recordings (notes/preview.md decision 4)
        let view = DetachedTaskActivityView(model: model, taskID: taskID, onClose: onClose)
        let hosting = NSHostingView(rootView: view)
        hosting.sizingOptions = []
        contentView = hosting
    }
}

private struct DetachedTaskActivityView: View {
    @ObservedObject var model: NotchModel
    let taskID: String
    let onClose: () -> Void

    private var task: TaskItem? { model.taskItems.first { $0.id == taskID } }

    var body: some View {
        Group {
            if let task {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .top) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("FERB is working").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.amber)
                            Text(task.title).font(.system(size: 13, weight: .medium)).lineLimit(2)
                        }
                        Spacer()
                        Button(action: onClose) {
                            Image(systemName: "xmark").font(.system(size: 10, weight: .semibold))
                                .frame(width: 23, height: 23)
                        }.buttonStyle(.plain).help("Close activity")
                    }
                    TaskVideoView(preview: model.preview(for: task))
                        .frame(height: 170)
                        .clipShape(RoundedRectangle(cornerRadius: 9))
                    Divider().opacity(0.4)
                    ScrollView {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(task.activity) { item in
                                Label(item.title, systemImage: item.symbol)
                                    .font(.system(size: 11)).textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            if task.activity.isEmpty { Text("Starting task…").font(.system(size: 11)).foregroundStyle(Palette.secondary) }
                        }
                    }
                }
                .padding(15)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
                .overlay(RoundedRectangle(cornerRadius: 16).stroke(.white.opacity(0.14), lineWidth: 1))
            }
        }
        .onChange(of: model.taskItems) { _, items in
            if !items.contains(where: { $0.id == taskID && $0.isRunning }) { onClose() }
        }
    }
}
