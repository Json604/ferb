import AppKit
import SwiftUI
import NotchCore

/// GATE-579, APP-828: a payment question's one-time picture of the payment page, fetched once by the question's id
/// (`questions.picture`, FERB.app only; the daemon hands it out once) and shared by the notch and the chat window. It
/// lives in memory only while its question is open: never saved with the chat, so a restored chat shows the card
/// without it. A picture that doesn't arrive (the call failed, the daemon had none left to give, it couldn't be read)
/// says so in one line (`missing`), never nothing (APP-887); answering never waits for it.
@MainActor
final class PaymentPictures: ObservableObject {
    /// The card's line when the daemon captured the page but FERB.app couldn't get or read the picture.
    static let notShown = "FERB couldn't show the payment page."
    @Published private(set) var images: [String: NSImage] = [:]
    @Published private(set) var missing: [String: String] = [:]
    private var asked: Set<String> = []
    private let rpc: any DaemonRPCTransport

    init(rpc: any DaemonRPCTransport) { self.rpc = rpc }

    func load(_ approval: DaemonApproval) {
        let id = approval.id
        guard approval.hasPicture, !asked.contains(id) else { return }
        asked.insert(id)
        Task { [weak self] in
            guard let self else { return }
            let reply = try? await self.rpc.call("questions.picture", params: ["approval_id": id])
            let decoded: CGImage?
            if let encoded = reply?["image"] as? String {
                decoded = await Task.detached(priority: .userInitiated, operation: {
                    PreviewFrame.decode(base64: encoded)
                }).value
            } else {
                decoded = nil
            }
            guard self.asked.contains(id) else { return }
            if let decoded {
                self.images[id] = NSImage(cgImage: decoded, size: NSSize(width: decoded.width, height: decoded.height))
            } else {
                self.missing[id] = Self.notShown
            }
        }
    }

    /// Only the open questions keep their pictures; an answered one's goes.
    func keep(_ open: Set<String>) {
        asked.formIntersection(open)
        if images.keys.contains(where: { !open.contains($0) }) { images = images.filter { open.contains($0.key) } }
        if missing.keys.contains(where: { !open.contains($0) }) { missing = missing.filter { open.contains($0.key) } }
    }
}

/// The picture under the payment card's first line, fitted to the card's width (at most `cardHeight` tall); a click
/// shows it larger with a ✕, and a click on it, outside it or Esc makes it small again (APP-889). Until it
/// comes, a quiet placeholder of its own: the view always draws something, so it appears and asks for the picture
/// (an empty Group never appears, which left the 2026-11-11 cards without their picture, APP-887). A picture the
/// daemon couldn't capture, or FERB.app couldn't get, is one line in its place.
struct PaymentPictureView: View {
    @ObservedObject var model: NotchModel
    @ObservedObject var pictures: PaymentPictures
    let approval: DaemonApproval

    /// APP-889: the picture in the card stays a picture of the card's width, at most this tall, so it never reads as
    /// already enlarged in the wide chat window; the whole page is one click away, with its ✕.
    static let cardHeight: CGFloat = 180

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let image = pictures.images[approval.id] {
                EnlargeablePicture(model: model, label: "The payment page") {
                    Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                        .frame(maxWidth: .infinity, maxHeight: Self.cardHeight)
                        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                } large: {
                    Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                        .frame(maxWidth: min(image.size.width, 900), maxHeight: min(image.size.height, 700))
                        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
            } else if let problem = approval.pictureProblem ?? pictures.missing[approval.id] {
                Text(problem).font(.system(size: 11)).foregroundStyle(Palette.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(.white.opacity(0.05))
                    .frame(maxWidth: .infinity).frame(height: 72)
                    .overlay(ProgressView().controlSize(.small))
                    .accessibilityLabel("Loading the payment page")
            }
        }
        .onAppear { pictures.load(approval) }
    }
}
