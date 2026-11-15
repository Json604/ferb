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
