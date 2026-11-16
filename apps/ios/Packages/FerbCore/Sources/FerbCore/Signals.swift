import Foundation
import Network

/// The phone's own signals to the link (the twin of Android's AndroidSignals, kb PHN-563): whether the app is in
/// front (the app passes its scene phase) and the network the phone uses (NWPathMonitor). Only another network than
/// the last one seen, or its loss, is a change (Wi-Fi to mobile data, one Wi-Fi to another); it is reported once
/// after 400 ms of quiet, as the phone lab did (Link.kt), since a handover arrives as several updates. The first path
/// is where the link began, not a change. The link tells iroh, which rebinds; it rebuilds nothing.
///
/// iOS has no Android "Stay connected" (a foreground service): an app in the background runs only as long as iOS
/// lets it. The app keeps the link active while it holds a background task (`backgroundUntil`), then lets it pause.
@MainActor
public final class PhoneSignals {
    private let phone: FerbPhone
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "com.ferb.phone.path")
    private var last: String?
    private var pending: DispatchWorkItem?
    public private(set) var foreground = false

    public init(phone: FerbPhone) {
        self.phone = phone
    }

    public func start() {
        monitor.pathUpdateHandler = { [weak self] path in
            let (signature, kind) = Self.describe(path)
            Task { @MainActor in self?.path(signature, kind) }
        }
        monitor.start(queue: queue)
    }

    public func stop() {
        monitor.cancel()
        pending?.cancel()
    }

    /// The app's scene became active (true) or left the screen (false). In the background the link stays up only
    /// while the app holds its background task; [backgroundEnded] then pauses it.
    public func scene(active: Bool) {
        foreground = active
        if active { phone.setActive(true) }
    }

    /// iOS is about to suspend the app (its background time ended): the link pauses; nothing dials until FERB opens.
    public func backgroundEnded() {
        if !foreground { phone.setActive(false) }
    }

    private func path(_ signature: String, _ kind: String) {
        defer { last = signature }
        guard let was = last, was != signature else { return }
        pending?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.phone.networkChanged(kind) }
        pending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4, execute: work)
    }

    nonisolated static func describe(_ path: NWPath) -> (String, String) {
        guard path.status == .satisfied else { return ("none", "none") }
        let kind = path.usesInterfaceType(.wifi) ? "wifi"
            : path.usesInterfaceType(.cellular) ? "cellular"
            : path.usesInterfaceType(.wiredEthernet) ? "ethernet" : "other"
        let names = path.availableInterfaces.map(\.name).joined(separator: ",")
        return ("\(kind):\(names)", kind)
    }
}
