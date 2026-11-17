import FerbMobile
import Foundation
import Observation
import XCTest
@testable import FerbAppModel

/// The work a change costs the screens (kb PHN-710, PHN-714; Android's FrameWorkTest): a streamed word in a long chat
/// changes the rows and the places and nothing every other view reads (the root, the tabs, the chat's frame read `ui`),
/// and the rows of a long chat are made off the main thread.
@MainActor
final class FrameWorkTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func line(_ i: Int, _ text: String) -> Item {
        .notice(notice: NoticeItem(entry: "e\(i)", at: Date().timeIntervalSinceReferenceDate - Double(5000 - i), legacy: false,
                                   reference: "e\(i)", after: nil, text: text, quotes: [], cards: nil))
    }

    private func view(_ n: Int, last: String) -> TranscriptView {
        TranscriptView(conversation: "c1", items: (0..<n).map { line($0, "A line from FERB number \($0), long enough to wrap.") } + [line(n, last)],
                       questions: [], live: [], outbox: [], older: .complete)
    }

    // decision: PHN-710, PHN-714
    func testAStreamedWordInALongChatChangesOnlyTheRows() async {
        AppModel.longChatsMadeOnMain = 0
        rig.phone.view = view(2_000, last: "The reply")
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("rows") { vm.rows.count > 2_000 && !vm.places.today.isEmpty }
        var text = "The reply"
        for i in 0..<10 {
            var uiChanged = false
            withObservationTracking({ _ = vm.ui; _ = vm.home }, onChange: { uiChanged = true })
            text += " word\(i)"
            let want = text
            rig.phone.view = view(2_000, last: want)
            rig.phone.emit(.transcript(conversation: "c1"))
            await eventually("the word \(i)") {
                (vm.rows.last.map { "\($0)".contains(want) } ?? false) && (vm.places.today.last.map { "\($0)".contains(want) } ?? false)
            }
            XCTAssertFalse(uiChanged, "a streamed word changed what the root and the tabs read (word \(i))")
        }
        XCTAssertEqual(AppModel.longChatsMadeOnMain, 0, "a long chat's rows were made on the main thread")
    }
}
