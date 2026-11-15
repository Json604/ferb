import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The Tasks view in the chat (APP-783, owner 2026-11-07: "when i click view tasks, it opens tasks but since its heoght
/// is hsorter thatn thatn the chat notch it closes - they should be of the same heiguht").
@MainActor
final class TasksHeightTests: XCTestCase {
    // decision: APP-783
    func testTasksOpensAtTheChatsOwnHeight() {
        let model = NotchModel(preferences: scratchDefaults())
        model.conversation.begin(id: UUID(), request: "find the latest Brad Pitt movie", sources: [], unredacted: true)
        model.runningTasks["t-1"] = "Brad Pitt's latest movie"
        let chat = model.bodyHeight
        model.showingTasks = true
        XCTAssertEqual(model.bodyHeight, chat)
        model.preferredTranscriptHeight = 460                     // the height the owner dragged the chat to (APP-587)
        model.showingTasks = false
        let taller = model.bodyHeight
        model.showingTasks = true
        XCTAssertEqual(model.bodyHeight, taller)
    }
}
