import Foundation
import NotchCore

/// What Settings → Stored data changes in the chat this app holds (STORE-424, STORE-425).
extension NotchModel {
    /// The owner deleted this turn in FERB's database: it leaves the chat shown here too, so nothing draws or writes it
    /// back.
    func removeTurns(requestIDs: Set<String>) {
        changeConversation { $0.removeTurns(requestIDs: requestIDs) }
    }

    /// A fresh conversation (`chat.attach` with `new`): FERB's planner reads no earlier message from now on; the chat
    /// history stays. Nil when done, else why not.
    func startFreshConversation() async -> String? {
        do {
            let attached = try await rpc.call("chat.attach", params: ["new": true])
            guard let id = attached["session_id"] as? String else { return "FERB didn't start a fresh conversation." }
            conversationID = id
            preferences.set(id, forKey: "FERBConversationID")
            // STORE-427: the chat shows where FERB stopped reading (saved like FERB's other lines), and the engine's
            // entries are followed in the new conversation from now on
            if changeConversation({ $0.addNotice(id: "conversation:" + id, text: StoredData.freshMarker, tag: nil) }) {
                saveConversation()
            }
            if entryWatch.supported {
                stopEntryWatch()
                startEntryWatch()
            }
            return nil
        } catch {
            return error.localizedDescription
        }
    }
}
