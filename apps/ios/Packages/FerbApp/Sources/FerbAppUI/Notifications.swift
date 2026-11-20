import FerbAppModel
import Foundation
import UserNotifications

/// FERB's notifications on iPhone (kb PHN-415, PHN-417, PHN-422, PHN-566): LOCAL notifications posted by FERB while
/// it runs (open, or in the short background time iOS gives), from the plan FerbAppModel's NotificationPlanner makes
/// (what, worded or wordless, which buttons). iOS's own "Show Previews" setting decides what the lock screen shows;
/// each category also carries the public line for a hidden preview. A question's buttons are a category of its own
/// (iOS's actions are fixed per category), registered as it is posted and dropped with it. Nothing is ever pushed
/// from a server (stage 5).
@MainActor
final class NotificationPoster: NSObject, UNUserNotificationCenterDelegate {
    /// Sends an answer given in a notification (AppShell → Notifier.answer).
    var answer: ((String, Int?, String?) async -> NotificationAction?)?
    /// A tap opens the app on the question or the chat.
    var open: ((NotificationTap) -> Void)?
    /// Lock my Mac (true) or It's fine from insomnia's notice (AppShell → Notifier.insomniaAnswer).
    var insomnia: ((String, Bool) async -> Void)?
    private var categories: [String: UNNotificationCategory] = [:]
    private var taps: [String: NotificationTap] = [:]
    private var center: UNUserNotificationCenter? { Bundle.main.bundleIdentifier == nil ? nil : .current() }

    nonisolated static let typedAction = "typed"
    nonisolated static let choicePrefix = "choice."

    func registerCategories() {
        for kind in [NotificationKind.question, .check, .done, .link, .mac] {
            categories[kind.rawValue] = Self.category(id: kind.rawValue, buttons: [], publicText: "")
        }
        center?.setNotificationCategories(Set(categories.values))
    }

    /// Asks iOS once whether FERB may show notifications (after FERB's own words, the notifications sheet).
    static func ask() async -> Bool {
        (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])) ?? false
    }

    static func notAsked() async -> Bool {
        await UNUserNotificationCenter.current().notificationSettings().authorizationStatus == .notDetermined
    }

    func apply(_ actions: [NotificationAction]) {
        for a in actions {
            switch a {
            case .post(let n): post(n)
            case .remove(let id): remove(id)
            }
        }
    }

    private func post(_ n: PlannedNotification) {
        let request = Self.request(n)
        taps[n.id] = n.tap
        let categoryId = request.content.categoryIdentifier
        if categories[categoryId] == nil || !n.buttons.isEmpty {
            categories[categoryId] = Self.category(id: categoryId, buttons: n.buttons, publicText: n.publicText)
            center?.setNotificationCategories(Set(categories.values))
        }
        center?.add(request)
    }

    private func remove(_ id: String) {
        center?.removeDeliveredNotifications(withIdentifiers: [id])
        center?.removePendingNotificationRequests(withIdentifiers: [id])
        taps[id] = nil
        if categories.removeValue(forKey: "q." + id) != nil { center?.setNotificationCategories(Set(categories.values)) }
    }

    /// The request iOS shows for a plan: its words (or none), a category with its buttons, its thread, and no sound
    /// for a quiet re-post. Never a picture.
    nonisolated static func request(_ n: PlannedNotification) -> UNNotificationRequest {
        let c = UNMutableNotificationContent()
        c.title = n.title
        c.body = n.body
        c.threadIdentifier = n.kind.rawValue
        c.categoryIdentifier = n.buttons.isEmpty ? n.kind.rawValue : "q." + n.id
        c.sound = n.quietly ? nil : .default
        c.interruptionLevel = n.quietly ? .passive : .active
        c.userInfo = ["id": n.id]
        return UNNotificationRequest(identifier: n.id, content: c, trigger: nil)
    }

    nonisolated static func category(id: String, buttons: [NotificationButton], publicText: String) -> UNNotificationCategory {
        let actions: [UNNotificationAction] = buttons.enumerated().map { i, b in
            if let choice = b.choice {
                // insomnia's Show me opens the app (its unlock, then the camera); every other button is answered in the
                // background, from the lock screen too (no .authenticationRequired)
                let options: UNNotificationActionOptions = choice == NotificationButton.showMe ? [.foreground] : []
                return UNNotificationAction(identifier: choicePrefix + String(choice), title: b.label, options: options)
            }
            return UNTextInputNotificationAction(identifier: typedAction, title: b.label, options: [],
                                                 textInputButtonTitle: b.label, textInputPlaceholder: b.placeholder ?? "")
        }
        if publicText.isEmpty { return UNNotificationCategory(identifier: id, actions: actions, intentIdentifiers: [], options: []) }
        return UNNotificationCategory(identifier: id, actions: actions, intentIdentifiers: [],
                                      hiddenPreviewsBodyPlaceholder: publicText, options: [])
    }

    // ---- UNUserNotificationCenterDelegate ----------------------------------------------------------------------------

    /// FERB in front: the planner already stays quiet while the owner looks at the open app; what it posts shows.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async
        -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let id = response.notification.request.identifier
        let action = response.actionIdentifier
        let typed = (response as? UNTextInputNotificationResponse)?.userText
        await MainActor.run { () -> Task<Void, Never>? in
            if action == UNNotificationDefaultActionIdentifier {
                open?(taps[id] ?? (id.hasPrefix("q:") ? .question(String(id.dropFirst(2))) : .app))
                return nil
            }
            if id.hasPrefix("in:") {
                let notice = String(id.dropFirst(3))
                let choice = action.hasPrefix(Self.choicePrefix) ? Int(action.dropFirst(Self.choicePrefix.count)) : nil
                switch choice {
                case NotificationButton.showMe: open?(.camera); return nil
                case NotificationButton.lockMac, NotificationButton.fine:
                    guard let insomnia else { return nil }
                    let lock = choice == NotificationButton.lockMac
                    return Task { await insomnia(notice, lock) }
                default: return nil
                }
            }
            guard id.hasPrefix("q:") else { return nil }
            let approval = String(id.dropFirst(2))
            let choice = action.hasPrefix(Self.choicePrefix) ? Int(action.dropFirst(Self.choicePrefix.count)) : nil
            let text = action == Self.typedAction ? typed : nil
            guard choice != nil || text != nil, let answer else { return nil }
            // the Notifier delivers what follows (gone, or why not) through `apply`
            return Task { _ = await answer(approval, choice, text) }
        }?.value
    }
}
