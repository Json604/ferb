import AppKit
import SwiftUI
import NotchCore

/// "What FERB saw" (step 12e): the screenshots FERB kept, exactly as the model got them (stored redacted copies; the
/// daemon never captures anew for this and never serves an excluded app's). `seen.list` and `seen.image` are FERB.app
/// only. Functional and bare: the owner polishes it later.
@MainActor
final class SeenModel: ObservableObject {
    @Published private(set) var list: SeenList?
    @Published private(set) var images: [String: NSImage] = [:]
    @Published private(set) var problem: String?
    @Published private(set) var loading = false
    private(set) var scope: SeenScope
    private let rpc: any DaemonRPCTransport

    init(rpc: any DaemonRPCTransport, scope: SeenScope) {
        self.rpc = rpc
        self.scope = scope
    }

    func show(_ scope: SeenScope) {
        guard scope != self.scope || list == nil else { return }
        self.scope = scope
        list = nil
        images = [:]
        Task { await load() }
    }

    func load() async {
        loading = true
        defer { loading = false }
        do {
            list = SeenList(try await rpc.call("seen.list", params: scope.params))
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }

    /// The image's bytes, fetched once when its row opens.
    func image(_ capture: SeenCapture) async {
        guard images[capture.id] == nil else { return }
        do {
            let reply = try await rpc.call("seen.image", params: ["id": capture.id])
            if let encoded = reply["image"] as? String, let bytes = Data(base64Encoded: encoded),
               let image = NSImage(data: bytes) {
                images[capture.id] = image
            }
        } catch {
            problem = error.localizedDescription
        }
    }
}

/// The list: each capture's time and what it shows; opening a row shows the image.
struct SeenListView: View {
    @ObservedObject var model: SeenModel
    @State private var open: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let problem = model.problem {
                Text(problem).font(.caption).foregroundStyle(.red).textSelection(.enabled)
            }
            if let list = model.list {
                Text("Exactly what FERB's AI was shown, with secrets painted over. Kept \(list.days) days.")
                    .font(.caption).foregroundStyle(.secondary)
                if list.captures.isEmpty { Text(model.scope.emptyText).foregroundStyle(.secondary) }
                ForEach(list.captures) { capture in row(capture) }
                if let more = list.moreLine { Text(more).font(.caption).foregroundStyle(.secondary) }
            } else if model.loading {
                ProgressView()
            }
        }
        .task { if model.list == nil { await model.load() } }
    }

    @ViewBuilder
    private func row(_ capture: SeenCapture) -> some View {
        let expanded = open.contains(capture.id)
        VStack(alignment: .leading, spacing: 4) {
            Button {
                if expanded { open.remove(capture.id) } else { open.insert(capture.id) }
            } label: {
                HStack(alignment: .firstTextBaseline) {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right").font(.caption)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(capture.label).lineLimit(1)
                        Text([capture.at?.formatted(date: .abbreviated, time: .standard) ?? "", capture.what]
                            .filter { !$0.isEmpty }.joined(separator: " · "))
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if expanded {
                Group {
                    if let image = model.images[capture.id] {
                        Image(nsImage: image).resizable().aspectRatio(contentMode: .fit)
                            .frame(maxWidth: .infinity, maxHeight: 420)
                    } else {
                        ProgressView().frame(maxWidth: .infinity)
                    }
                }
                .task { await model.image(capture) }
            }
        }
    }
}

/// The window the notch's "What FERB saw" opens: one message's (or task's) screenshots. It opens only on the owner's
/// click (their own foreground action).
@MainActor
final class SeenWindowController: NSObject, NSWindowDelegate {
    private let model: SeenModel
    private var window: NSWindow?

    init(rpc: any DaemonRPCTransport) {
        model = SeenModel(rpc: rpc, scope: .all)
    }

    func show(_ scope: SeenScope) {
        model.show(scope)
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 640, height: 560),
                                   styleMask: [.titled, .closable, .miniaturizable, .resizable],
                                   backing: .buffered, defer: false)
            created.title = "What FERB saw"
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.sharingType = .none                   // FERB's own pictures stay out of screen sharing
            created.contentView = NSHostingView(rootView: ScrollView {
                SeenListView(model: model).padding(16).frame(maxWidth: .infinity, alignment: .leading)
            })
            created.center()
            window = created
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}

/// Under the owner's message in the notch: opens what FERB saw for it.
struct SeenButton: View {
    let scope: SeenScope
    @ObservedObject var model: NotchModel

    var body: some View {
        if model.onShowSeen != nil {
            HStack {
                Spacer()
                Button("What FERB saw") { model.onShowSeen?(scope) }
                    .buttonStyle(.plain).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                    .help("The screenshots FERB kept for this message, as its AI saw them")
            }
        }
    }
}

/// Settings → Data → What FERB saw: every screenshot FERB kept, newest first.
struct SeenSection: View {
    @StateObject private var model: SeenModel

    init(rpc: any DaemonRPCTransport) {
        _model = StateObject(wrappedValue: SeenModel(rpc: rpc, scope: .all))
    }

    var body: some View {
        SeenListView(model: model)
        Button("Refresh") { Task { await model.load() } }
    }
}
