import Foundation
@testable import FERBNotch

@MainActor
final class FakeNotificationCenter: NotificationCenterPosting {
    var allowed = true
    private(set) var posted: [(id: String, title: String, body: String, userInfo: [String: String], thread: String)] = []
    private(set) var withdrawn: [String] = []

    func authorized() async -> Bool { allowed }
    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String) {
        posted.append((id, title, body, userInfo, thread))
    }
    func withdraw(ids: [String]) { withdrawn += ids }
}
