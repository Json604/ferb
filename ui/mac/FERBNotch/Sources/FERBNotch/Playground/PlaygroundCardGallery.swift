#if FERB_PLAYGROUND
import AppKit
import SwiftUI
import NotchCore

/// FERB's record cards, all at once (APP-802; owner 2026-11-09: "yes draw them. BUT show them in the playground for me to
/// see htem all at once what are they and how they look"). Each sample is shaped exactly as the daemon sends it
/// (ferb/daemon/areas/records.py, supervisor `_task_record` / `_tasks_card`, cards.done_cards), read through the real
/// `ResultCard` and drawn by the real `ResultCardView`. Invented names, sites and files only.
@MainActor
enum PlaygroundRecordCards {
    struct Entry {
        let title: String
        let when: String
        let payload: [String: Any]
    }

    static func entries(download: URL) -> [Entry] {
        let briefTask: [String: Any] = [
            "type": "task", "task_id": "7c1e0a52-brief", "name": "Afternoon brief", "status": "done", "verified": true,
            "line": "📋 Afternoon brief: done and checked.",
            "questions": [["question": "Which project folder should the brief go in?", "answer": "Studio launch"]],
            "cards": [["type": "send", "kind": "mailbox_send", "recipient": "Alex Moreno", "site": "mail.studio.example"],
                      ["type": "file", "path": "/Users/sample/Documents/Studio launch/Afternoon brief.pdf",
                       "name": "Afternoon brief.pdf"],
                      ["type": "link", "host": "notes.studio.example"]]]
        let visaTask: [String: Any] = [
            "type": "task", "task_id": "3fa9d811-visa", "name": "Visa slot check", "status": "failed", "verified": false,
            "line": "📋 Visa slot check: couldn't finish.",
            "cards": [["type": "link", "host": "appointments.visa.example"]]]
        let invoicesTask: [String: Any] = [
            "type": "task", "task_id": "b2047c9e-invoices", "name": "Collect studio invoices", "status": "done_unverified",
            "verified": false, "note": "the October invoice from Print Co. was saved",
            "line": "📋 Collect studio invoices: done, not all of it confirmed (couldn't confirm: the October invoice from Print Co. was saved).",
            "cards": [["type": "download", "path": download.path, "name": download.lastPathComponent],
                      ["type": "submit", "site": "billing.printco.example"]]]
        let runningTask: [String: Any] = [
            "type": "task", "task_id": "e51d33f0-tidy", "name": "Tidy the Downloads folder", "status": "running",
            "verified": false, "line": "📋 Tidy the Downloads folder: running."]
        func listed(_ task: [String: Any]) -> [String: Any] { task.filter { $0.key != "line" } }

        return [
            Entry(title: "Reminders and timers",
                  when: "You ask about your reminders or timers (\"what reminders do I have?\"). FERB lists every pending "
                      + "one from its timer rows, soonest first.",
                  payload: ["type": "reminders", "count": 3,
                            "line": "⏰ Your reminders and timers (3): Tea — Mon 09 Nov 16:20; Call the framer — …",
                            "reminders": [
                                ["id": "t-31", "kind": "timer", "name": "Tea", "at": PlaygroundSamples.time(240)],
                                ["id": "r-12", "kind": "reminder", "name": "Call the framer", "at": PlaygroundSamples.time(5400)],
                                ["id": "r-14", "kind": "reminder", "name": "Renew the studio domain",
                                 "at": PlaygroundSamples.time(3 * 86400)]]]),
            Entry(title: "Schedules (already drawn before)",
                  when: "You ask about your schedules. Shown here for comparison with the new list cards.",
                  payload: ["type": "schedules", "count": 1, "line": "🗓 Your schedules (1): Morning brief — every weekday at 09:00",
                            "schedules": [["id": "s-2", "name": "Morning brief", "repeat": "every weekday at 09:00",
                                           "state": "active", "next_run": PlaygroundSamples.next(hour: 9, weekdaysOnly: true)]]]),
            Entry(title: "Tasks",
                  when: "You ask what FERB is running or has done (\"what are you working on?\"). Every running task, "
                      + "then this chat's ended ones, each as its task card says it.",
                  payload: ["type": "tasks", "count": 4,
                            "line": "📋 Your tasks (4): 1 running, 1 done and checked, 1 couldn't finish, 1 done, not all of it confirmed; running: Tidy the Downloads folder",
                            "tasks": [listed(runningTask), listed(briefTask), listed(visaTask), listed(invoicesTask)]]),
            Entry(title: "One task: what it did",
                  when: "Your question is about what a task did (\"did you send the brief?\"). Its status as FERB's code "
                      + "set it, whether it was checked, the questions you answered for it and what its records show "
                      + "it did. Its send, file and site cards are also drawn on their own under it.",
                  payload: briefTask),
            Entry(title: "One task: done, not all of it confirmed",
                  when: "The same card for a task whose check couldn't confirm everything: code's couldn't-confirm note.",
                  payload: invoicesTask),
            Entry(title: "Memory",
                  when: "You ask what FERB remembers. What memory holds now, and the forgets on record (when and how "
                      + "many; what was forgotten is never kept).",
                  payload: ["type": "memory", "count": 3, "line": "🧠 FERB remembers 3 things; 2 forgotten (their contents aren't kept).",
                            "items": [
                                ["id": "m-1", "kind": "preference", "subject": "briefs", "key": "length",
                                 "text": "Keep morning briefs short, with decisions first.", "origin": "owner_stated"],
                                ["id": "m-2", "kind": "fact", "subject": "Alex Moreno", "key": "email",
                                 "text": "Alex Moreno's work email is alex@studio.example.", "origin": "owner_stated"],
                                ["id": "m-3", "kind": "habit", "subject": "work", "key": "focus",
                                 "text": "Usually blocks 2–3:30 PM for focused work.", "origin": "inferred"]],
                            "forgotten": [["at": PlaygroundSamples.time(-6 * 86400), "items": 1],
                                          ["at": PlaygroundSamples.time(-86400), "items": 1]]]),
            Entry(title: "Permissions in force",
                  when: "You ask what FERB may do without asking (\"can you message Alex without asking me?\"). The "
                      + "standing send scopes, the destinations it knows and whether routine questions are off.",
                  payload: ["type": "permissions", "count": 1,
                            "line": "🔐 Standing permissions: 1 send scope in force; 3 known destinations (2 approved); routine questions off until Mon 09 Nov 17:00.",
                            "standing_sends": [["held_by": "5d2c9e10-schedule", "capability": "mailbox_send",
                                                "identity": "ferb", "account": "ferb@studio.example", "site": "",
                                                "recipients": ["alex@studio.example"], "any_text": true]],
                            "known_destinations": [
                                ["kind": "email", "value": "alex@studio.example", "how": "named", "approved": false],
                                ["kind": "email", "value": "billing@printco.example", "how": "confirmed", "approved": true],
                                ["kind": "chat", "value": "(kept only as a fingerprint)", "how": "answered", "approved": true]],
                            "routine_questions_off_until": PlaygroundSamples.time(1800)]),
            Entry(title: "Saved logins",
                  when: "You ask which logins FERB has. Names and sites only: never a password, a username or a card "
                      + "number.",
                  payload: ["type": "logins", "count": 2,
                            "line": "🔑 Saved logins (2): billing.printco.example — Print Co. login; shop.example — Studio card",
                            "owner_saved": [
                                ["place": "billing.printco.example", "name": "Print Co. login", "kind": "secret", "unusable": false],
                                ["place": "shop.example", "name": "Studio card", "kind": "card", "unusable": false],
                                ["place": "old-portal.example", "name": "Old portal", "kind": "secret", "unusable": true]],
                            "ferb_own": [["name": "FERB mailbox", "account": "f…@studio.example",
                                          "sites": ["mail.studio.example"]]]]),
            Entry(title: "About FERB",
                  when: "You ask about FERB itself (\"which version are you, which model?\"). Its version, the model "
                      + "each role uses and how many permissions are granted.",
                  payload: ["type": "about", "line": "ℹ️ FERB 0.27.3; planner Codex gpt-6-astra; worker Codex gpt-6-astra; checker Codex gpt-6-astra-mini; 7 of 8 permissions granted.",
                            "version": ["ferb": "0.27.3"],
                            "models": ["planner": "Codex gpt-6-astra", "worker": "Codex gpt-6-astra",
                                       "checker": "Codex gpt-6-astra-mini"],
                            "permissions": [["id": "accessibility", "status": "granted"],
                                            ["id": "screen_recording", "status": "granted"],
                                            ["id": "full_disk_access", "status": "denied"],
                                            ["id": "input_monitoring", "status": "granted"]]]),
            Entry(title: "Form submitted",
                  when: "A task's end, when its records show a form submitted on a site.",
                  payload: ["type": "submit", "site": "billing.printco.example"]),
            Entry(title: "Moved to the Trash",
                  when: "A task's end, when it moved a file to the Trash (FERB keeps a recovery copy).",
                  payload: ["type": "trash", "path": "/Users/sample/Downloads/old-invoice-draft.pdf",
                            "name": "old-invoice-draft.pdf"]),
            Entry(title: "Deleted permanently",
                  when: "A task's end, when it deleted a file permanently (after your Yes).",
                  payload: ["type": "delete", "path": "/Users/sample/Downloads/duplicate-scan.png", "name": "duplicate-scan.png"]),
            Entry(title: "Downloaded",
                  when: "A task's end, for each file its browser tab downloaded; the chip shows it in Finder.",
                  payload: ["type": "download", "path": download.path, "name": download.lastPathComponent]),
        ]
    }

    /// The reply FERB's records chat sample sends: the cards' lines in its text (the notch leaves out each one a card
    /// stands for, ORCH-329) and the cards.
    static func reply(download: URL) -> (text: String, cards: [[String: Any]]) {
        let cards = entries(download: download).map(\.payload).filter { $0["type"] as? String != "schedules" }
        let lines = cards.compactMap { $0["line"] as? String }
        let text = (["Here is what FERB's records hold. Nothing here is real."] + lines).joined(separator: "\n")
        return (text, cards)
    }
}

/// The gallery: every record card, labelled with what it is and when FERB shows it, on the notch's black.
struct PlaygroundCardGallery: View {
    let entries: [PlaygroundRecordCards.Entry]
    @ObservedObject var model: NotchModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 26) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("FERB's record cards").font(.system(size: 20, weight: .semibold))
                    Text("Every card the daemon can now send for FERB's own state and a task's records, drawn by the "
                         + "app's real card views from invented sample data. In chat each card replaces the text line "
                         + "shown above it; an older FERB.app shows that line instead.")
                        .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                    VStack(alignment: .leading, spacing: 8) {
                        Text(entry.title).font(.system(size: 14, weight: .semibold))
                        Text("When: " + entry.when).font(.system(size: 11.5)).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        if let line = entry.payload["line"] as? String {
                            Text("Replaces the line: " + line).font(.system(size: 10.5, design: .monospaced))
                                .foregroundStyle(.tertiary).fixedSize(horizontal: false, vertical: true)
                        } else {
                            Text("Stands for no line: added under a task's answer.")
                                .font(.system(size: 10.5, design: .monospaced)).foregroundStyle(.tertiary)
                        }
                        if let card = ResultCard(entry.payload) {
                            ResultCardView(card: card, turnID: UUID(), model: model).frame(width: 460, alignment: .leading)
                        } else {
                            Text("Not drawn: the app couldn't read this sample.").foregroundStyle(Palette.red)
                        }
                    }
                }
            }
            .padding(24)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Color.black)
        .environment(\.colorScheme, .dark)
    }
}
#endif
