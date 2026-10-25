import Foundation
@testable import FERBNotch

@MainActor
final class FakeNotificationCenter: NotificationCenterPosting {
    var allowed = true
    private(set) var posted: [(id: String, title: String, body: String, userInfo: [String: String], thread: String)] = []
    private(set) var withdrawn: [String] = []

    var undecidedState = false
    var answer = true
    private(set) var requests = 0
    func authorized() async -> Bool { allowed }
    func undecided() async -> Bool { undecidedState }
    func request() async -> Bool { requests += 1; undecidedState = false; allowed = answer; return answer }
    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String) {
        posted.append((id, title, body, userInfo, thread))
    }
    func withdraw(ids: [String]) { withdrawn += ids }
}
