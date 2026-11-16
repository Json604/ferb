import QuickLook
import SwiftUI
import UIKit

/// A file fetched from the Mac, opened only on the owner's tap (kb PHN-517's iPhone half): QuickLook shows it (its own
/// Share button hands it to another app), or the share sheet hands it straight on. Nothing opens by itself; the file
/// stays in FERB's own cache until the lock, 24 hours or Forget (PHN-515), a key place's at the next return (PHN-516).
struct QuickLookView: UIViewControllerRepresentable {
    let url: URL
    let done: () -> Void

    func makeUIViewController(context: Context) -> UINavigationController {
        let ql = QLPreviewController()
        ql.dataSource = context.coordinator
        ql.delegate = context.coordinator
        ql.navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { _ in done() })
        return UINavigationController(rootViewController: ql)
    }

    func updateUIViewController(_ controller: UINavigationController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(url: url) }

    final class Coordinator: NSObject, QLPreviewControllerDataSource, QLPreviewControllerDelegate {
        let url: URL
        init(url: URL) { self.url = url }
        func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
        func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> QLPreviewItem { url as NSURL }
    }
}

/// The system share sheet for one file, or a fetched folder's files together.
struct ShareSheet: UIViewControllerRepresentable {
    let urls: [URL]
    let done: () -> Void

    func makeUIViewController(context: Context) -> UIActivityViewController {
        let c = UIActivityViewController(activityItems: urls, applicationActivities: nil)
        c.completionWithItemsHandler = { _, _, _, _ in done() }
        return c
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
