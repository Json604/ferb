import AppKit
import NotchCore
import UserNotifications

/// Posting notifications, so tests stand in for UNUserNotificationCenter (it needs a real app bundle).
@MainActor
protocol NotificationCenterPosting: AnyObject {
    func authorized() async -> Bool
    /// macOS hasn't asked the owner yet.
    func undecided() async -> Bool
    /// macOS's own prompt; true when the owner allowed it.
    func request() async -> Bool
    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String)
    func withdraw(ids: [String])
}

/// UNUserNotificationCenter for FERB.app (L34 D): FERB's own name and icon; Focus modes respected (macOS applies them to
/// the default interruption level); a click comes back through `NotificationClicks`. Built only when `AppBundle.isApp`.
@MainActor
final class UserNotificationCenter: NotificationCenterPosting {
    private let center = UNUserNotificationCenter.current()

    func authorized() async -> Bool {
        let status = await center.notificationSettings().authorizationStatus
        return status == .authorized || status == .provisional
    }

    func undecided() async -> Bool { await center.notificationSettings().authorizationStatus == .notDetermined }

    func request() async -> Bool { (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false }

    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = userInfo
        content.threadIdentifier = thread
        content.sound = .default
        center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil)) { error in
            if let error { NSLog("FERB: a notification wasn't posted (%@).", error.localizedDescription) }
        }
    }

    func withdraw(ids: [String]) {
        center.removeDeliveredNotifications(withIdentifiers: ids)
        center.removePendingNotificationRequests(withIdentifiers: ids)
    }
}

/// FERB.app as the daemon's poster (L34 D): while macOS lets FERB.app notify, it holds `notifications.subscribe`, so the
/// daemon stops using osascript; each banner is posted unless the notch is already showing it; a click opens the notch at
/// its item. Without a notification center (a development build run without a bundle) it never subscribes, and the
/// daemon keeps osascript.
@MainActor
final class NotificationPoster {
    private let center: NotificationCenterPosting?
    private var loop: Task<Void, Never>?
    private var notchShowing: () -> Bool = { false }
    private var opener: ((NotificationTarget) -> Void)?
    private var pending: NotificationTarget?
    /// The wait before subscribing again after the daemon went away or macOS didn't allow notifications.
    static let retry: Duration = .seconds(5)

    init(center: NotificationCenterPosting?) { self.center = center }

    func configure(notchShowing: @escaping () -> Bool, open: @escaping (NotificationTarget) -> Void) {
        self.notchShowing = notchShowing
        opener = open
        if let pending {
            self.pending = nil
            open(pending)
        }
    }

    func start(rpc: DaemonRPCTransport, notchShowing: @escaping () -> Bool,
               open: @escaping (NotificationTarget) -> Void) {
        configure(notchShowing: notchShowing, open: open)
        guard center != nil, loop == nil else { return }
        loop = Task { [weak self] in
            while !Task.isCancelled {
                _ = await self?.subscribeOnce(rpc: rpc)
                do { try await Task.sleep(for: Self.retry) } catch { return }
            }
        }
    }

    func stop() {
        loop?.cancel()
        loop = nil
    }

    /// One subscription, until the daemon ends it or the connection drops. False when macOS doesn't let FERB.app notify
    /// (or there is no center): then FERB.app doesn't claim the poster role and osascript keeps posting.
    @discardableResult
    func subscribeOnce(rpc: DaemonRPCTransport) async -> Bool {
        guard let center else { return false }
        let allowed = await center.authorized()
        if !allowed, !(await center.undecided()) { return false }
        // Asked when first needed (owner 2026-10-26: "ask when first needed, like Claude and ChatGPT"): while macOS
        // hasn't asked yet, the first banner brings macOS's prompt; a No hands posting back to osascript.
        var asked = allowed
        do {
            for try await event in rpc.stream("notifications.subscribe", params: ["authorized": true]) {
                if !asked {
                    asked = true
                    if !(await center.request()) { break }
                }
                receive(event)
            }
        } catch {}
        return true
    }

    func receive(_ event: [String: Any]) {
        guard let center, let item = BannerItem(event),
              NotificationGate.shouldPost(item, notchShowing: notchShowing()) else { return }
        center.post(id: item.id, title: item.title, body: item.body, userInfo: item.target.userInfo,
                    thread: item.category?.rawValue ?? "ferb")
    }

    /// A question answered anywhere: its banner goes from Notification Center.
    func withdrawQuestion(_ approvalID: String) { center?.withdraw(ids: [NotificationGate.questionID(approvalID)]) }

    func clicked(_ userInfo: [AnyHashable: Any]) {
        let target = NotificationTarget(userInfo: userInfo)
        if let opener { opener(target) } else { pending = target }
    }
}

/// UNUserNotificationCenter's delegate: banners also show while FERB.app is the active app, and a click reaches the
/// poster. Installed before launching finishes, so a click that launched FERB isn't lost.
final class NotificationClicks: NSObject, UNUserNotificationCenterDelegate {
    weak var poster: NotificationPoster?

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        Task { @MainActor [weak self] in
            self?.poster?.clicked(info)
            completionHandler()
        }
    }
}
