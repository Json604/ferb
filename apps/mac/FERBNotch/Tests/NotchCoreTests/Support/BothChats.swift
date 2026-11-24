import Foundation
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The two chats FERB.app shows, for tests that must hold for each alike (owner 2026-11-24: "i told you to match normal
/// chat and temp chat UI, why are they even diff???"): the saved chat, and the temporary chat held in the daemon's
/// memory. The same steps run on both; only where the conversation comes from differs.
let bothChats: [ChatSource] = [.saved, .temporary]

extension XCTestCase {
    /// A model showing `source`'s chat, its sounds recorded, against a daemon whose `chat.send` stays open with nothing
    /// said (the test hands the turn its frames): the temporary chat on as `chat.temporary.get` says it, or the saved
    /// chat.
    @MainActor
    func chatModel(_ source: ChatSource, _ name: String) -> (NotchModel, FakeSoundPlayer, ScriptedRPC) {
        let rpc = ScriptedRPC()
        rpc.replies["chat.attach"] = ["session_id": "S1", "capabilities": ["input_paths"]]
        rpc.heldStreams = ["chat.send"]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("\(name)-\(source)"))
        let sounds = FakeSoundPlayer()
        model.feedback.player = sounds
        if source == .temporary {
            model.applyTemporaryChat(["on": true, "id": "t-test", "conversation": "temporary:t-test", "rev": 1,
                                      "entries": [[String: Any]]()])
        }
        return (model, sounds, rpc)
    }
}

@MainActor
extension NotchModel {
    /// The owner types `text` and sends it, into the chat that shows: its turn's id.
    func sendFromComposer(_ text: String) -> UUID {
        session.draft = text
        startWork()
        return session.activeTaskID!
    }

    /// The turn `id` as the shown chat draws it.
    func shownTurn(_ id: UUID) -> ConversationTurn? { shownArchive.turns.first { $0.id == id } }
}
