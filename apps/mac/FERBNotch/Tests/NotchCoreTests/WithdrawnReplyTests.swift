import XCTest
@testable import NotchCore

/// A streamed reply the planner withdrew (APP-786, owner 2026-11-07: "the ferb reply to taks doesnt show, it did appear
/// in teh short time notch, but i cant see it in teh nothc chat"): the planner's first plan broke a rule and was sent
/// back for repair (task.repair, 18 s); the chat showed nothing under the owner's message until the new reply came.
final class WithdrawnReplyTests: XCTestCase {
    // decision: APP-786
    func testAWithdrawnReplyShowsFERBStillReplyingUntilTheRealOneComes() {
        var archive = ConversationArchive()
        let id = UUID()
        archive.begin(id: id, request: "fill both applications", sources: [])
        archive.acknowledge(id: id, text: "Sure, on it.")
        XCTAssertTrue(archive.turns[0].showsTyping)                       // reading the message
        archive.streamPlan(id: id, delta: "I'm filling both applications now.")
        XCTAssertFalse(archive.turns[0].showsTyping)                      // its words are showing
        XCTAssertTrue(archive.turns[0].firstReplySays)
        archive.abortPlanStream(id: id)                                   // withdrawn: the plan goes back for repair
        XCTAssertTrue(archive.turns[0].showsTyping, "never nothing under the owner's message")
        archive.showPlan(id: id, text: "I'm filling both applications now, leaving them unsubmitted.")
        XCTAssertFalse(archive.turns[0].showsTyping)
        XCTAssertTrue(archive.turns[0].firstReplySays)
    }
}
