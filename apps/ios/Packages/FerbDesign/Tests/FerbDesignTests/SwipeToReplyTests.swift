import SwiftUI
import XCTest
@testable import FerbDesign

/// Swipe to reply (kb APP-1146): which drags are a reply's and which the chat's scroll, how far the row follows, and
/// when letting go replies.
final class SwipeToReplyTests: XCTestCase {
    // decision: APP-1146
    func testOnlyARightwardSidewaysDragIsAReplysTheRestIsTheScrolls() {
        XCTAssertEqual(SwipeToReply.decide(CGSize(width: 14, height: 3)), .sideways(14))
        XCTAssertEqual(SwipeToReply.decide(CGSize(width: 3, height: 14)), .upDown, "up or down is the chat's scroll")
        XCTAssertEqual(SwipeToReply.decide(CGSize(width: 12, height: -10)), .upDown, "a diagonal is the scroll's too")
        XCTAssertEqual(SwipeToReply.decide(CGSize(width: -14, height: 0)), .upDown, "a leftward drag is no reply")
    }

    // decision: APP-1146
    func testTheRowFollowsAShortWayAndLettingGoPastTheReachReplies() {
        XCTAssertEqual(SwipeToReply.follow(0), 0)
        XCTAssertEqual(SwipeToReply.follow(30), 30, "one to one up to the reach")
        XCTAssertEqual(SwipeToReply.follow(SwipeToReply.reach + 9), SwipeToReply.reach + 3, "then a third as much")
        XCTAssertEqual(SwipeToReply.follow(400), SwipeToReply.most, "never further than a short way")
        XCTAssertFalse(SwipeToReply.replies(CGSize(width: SwipeToReply.reach - 1, height: 0)))
        XCTAssertTrue(SwipeToReply.replies(CGSize(width: SwipeToReply.reach, height: 8)))
        XCTAssertFalse(SwipeToReply.replies(CGSize(width: 80, height: 70)), "a scroll that drifted sideways")
    }
}
