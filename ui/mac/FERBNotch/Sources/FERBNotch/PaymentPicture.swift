import AppKit
import SwiftUI
import NotchCore

/// GATE-579, APP-828: a payment question's one-time picture of the payment page, fetched once by the question's id
/// (`questions.picture`, FERB.app only; the daemon hands it out once) and shared by the notch and the chat window. It
/// lives in memory only while its question is open: never saved with the chat, so a restored chat shows the card
/// without it. A picture that doesn't arrive leaves the card as it is; answering never waits for it.
@MainActor
final class PaymentPictures: ObservableObject {
    @Published private(set) var images: [String: NSImage] = [:]
    private var asked: Set<String> = []
    private let rpc: any DaemonRPCTransport

    init(rpc: any DaemonRPCTransport) { self.rpc = rpc }

    func load(_ approval: DaemonApproval) {
        let id = approval.id
        guard approval.hasPicture, !asked.contains(id) else { return }
        asked.insert(id)
        Task { [weak self] in
            guard let self, let reply = try? await self.rpc.call("questions.picture", params: ["approval_id": id]),
                  let encoded = reply["image"] as? String,
                  let decoded = await Task.detached(priority: .userInitiated, operation: {
                      PreviewFrame.decode(base64: encoded)
                  }).value,
                  self.asked.contains(id) else { return }
            self.images[id] = NSImage(cgImage: decoded, size: NSSize(width: decoded.width, height: decoded.height))
        }
    }

    /// Only the open questions keep their pictures; an answered one's goes.
    func keep(_ open: Set<String>) {
        asked.formIntersection(open)
        if images.keys.contains(where: { !open.contains($0) }) { images = images.filter { open.contains($0.key) } }
    }
}

/// The picture under the payment card's first line, fitted to the card's width; a click shows it larger.
struct PaymentPictureView: View {
    @ObservedObject var model: NotchModel
    @ObservedObject var pictures: PaymentPictures
    let approval: DaemonApproval
    @State private var enlarged = false

    var body: some View {
        Group {
            if let image = pictures.images[approval.id] {
                Button { enlarged = true } label: {
                    Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                        .frame(maxWidth: .infinity)
                        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
                .buttonStyle(.plain)
                .help("Click to enlarge")
                .accessibilityLabel("The payment page")
                .popover(isPresented: $enlarged, arrowEdge: .bottom) {
                    Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                        .frame(maxWidth: min(image.size.width, 900), maxHeight: min(image.size.height, 700))
                        .padding(8)
                        .accessibilityLabel("The payment page")
                        .background(TaskPreviewWindow(model: model, hidesFromCapture: true))
                }
            } else if let problem = approval.pictureProblem {
                Text(problem).font(.system(size: 11)).foregroundStyle(Palette.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .onAppear { pictures.load(approval) }
    }
}
