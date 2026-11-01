import Foundation
@testable import FERBNotch

@MainActor
final class FakeNotificationCenter: NotificationCenterPosting {
    var allowed = true
    private(set) var posted: [(id: String, title: String, body: String, userInfo: [String: String], thread: String)] = []
    private(set) var withdrawn: [String] = []
    /// Whether each posted banner carried FERB's notification sound, in order.
    private(set) var sounds: [Bool] = []

    var undecidedState = false
    var answer = true
    private(set) var requests = 0
    func authorized() async -> Bool { allowed }
    func undecided() async -> Bool { undecidedState }
    func request() async -> Bool { requests += 1; undecidedState = false; allowed = answer; return answer }
    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String, sound: Bool) {
        posted.append((id, title, body, userInfo, thread))
        sounds.append(sound)
    }
    func withdraw(ids: [String]) { withdrawn += ids }
}
