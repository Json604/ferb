#if FERB_PLAYGROUND
import AppKit
import NotchCore

/// Invented records for reviewing the actual product views. No owner data is read.
@MainActor
enum PlaygroundSamples {
    static func time(_ seconds: Double = 0) -> String {
        ISO8601DateFormatter().string(from: Date().addingTimeInterval(seconds))
    }

    static func next(hour: Int, weekday: Int? = nil, day: Int? = nil, weekdaysOnly: Bool = false) -> String {
        let calendar = Calendar.current
        var parts = DateComponents()
        parts.hour = hour
        parts.minute = 0
        parts.weekday = weekday
        parts.day = day
        var date = calendar.nextDate(after: Date(), matching: parts, matchingPolicy: .nextTime) ?? Date()
        while weekdaysOnly && calendar.isDateInWeekend(date) {
            date = calendar.date(byAdding: .day, value: 1, to: date) ?? date.addingTimeInterval(86400)
        }
        return ISO8601DateFormatter().string(from: date)
    }

    static var tasks: [[String: Any]] { [
        ["task_id": "sample-brief", "goal": "Make a short brief for my afternoon", "status": "done",
         "created_at": time(-900), "answer": "Your afternoon brief is ready. Two decisions need your attention; the rest can wait."],
        ["task_id": "sample-research", "goal": "Compare three options for a studio monitor", "status": "done",
         "created_at": time(-3600), "answer": "Compared the three options on size, connectivity and price. The comparison is ready to review."],
        ["task_id": "sample-retry", "goal": "Collect this week's invoices", "status": "failed",
         "created_at": time(-7200), "answer": "The sample account needs to be reconnected before I can continue."],
        ["task_id": "sample-yesterday", "goal": "Organize the launch notes", "status": "done",
         "created_at": time(-86400), "answer": "Grouped the notes into decisions, questions and next steps.", "verdict": "pass"]
    ] }
    static var schedules: [[String: Any]] { [
        ["id": "sample-morning", "request": "Bring me a short morning brief", "name": "Morning brief", "rule": "Every weekday at 9:00 AM",
         "state": "active", "next_run": next(hour: 9, weekdaysOnly: true), "failures": 0,
         "last_result": ["at": time(-10800), "status": "done", "answer": "Three updates and one decision for today."]],
        ["id": "sample-weekly", "request": "Summarize the week and prepare Monday's priorities", "name": "Weekly review", "rule": "Fridays at 5:00 PM",
         "state": "paused", "next_run": next(hour: 17, weekday: 6), "failures": 0],
        ["id": "sample-invoices", "request": "Collect the studio's monthly invoices", "name": "Monthly invoices", "rule": "First day of each month at 10:00 AM",
         "state": "active", "next_run": next(hour: 10, day: 1), "failures": 1,
         "last_result": ["at": time(-86400), "status": "failed", "answer": "The sample account needs to be reconnected."]]
    ] }
    static var reminders: [[String: Any]] { [
        ["id": "sample-water", "kind": "reminder", "message": "Take a break and get some water", "name": "Water break", "at": time(1200)],
        ["id": "sample-review", "kind": "reminder", "message": "Review the launch mockups with Alex", "name": "Mockup review", "at": time(7200)],
        ["id": "sample-timer", "kind": "timer", "message": "Focus session finished", "name": "25-minute focus timer", "at": time(1500)]
    ] }
    static var notices: [[String: Any]] { [
        ["id": "sample-notice", "text": "The launch notes have changed since your last review.", "at": time(-1800), "shown": true],
        ["id": "sample-notice-2", "text": "There's a quieter block of time this afternoon for the design review.",
         "at": time(-5400), "shown": true, "quotes": [["text": "Design review moved to tomorrow", "origin": "Sample calendar"]]]
    ] }
    static var memories: [[String: Any]] { [
        ["id": "sample-memory-1", "kind": "fact", "content": "Keep morning briefs short, with decisions first.",
         "label": "Your words", "about": "Briefs", "status": "active", "in_profile": true],
        ["id": "sample-memory-2", "kind": "fact", "content": "Keep drafts in the studio project folder.",
         "label": "Your words", "about": "Files", "status": "active", "in_profile": false],
        ["id": "sample-memory-3", "kind": "fact", "content": "Leave an uninterrupted block for design work after lunch.",
         "label": "FERB's understanding", "about": "Workday", "status": "active", "in_profile": false]
    ] }
    static var settings: [String: Any] { [
        "sense.window_titles": true, "privacy.exclude_apps": ["Password manager"], "privacy.exclude_domains": ["bank.example"],
        "sense.trail_retention_days": 7, "retention.trace_days": 30, "retention.decision_days": 30,
        "retention.evidence_days": 7, "retention.recovery_days": 7, "retention.resource_days": 30,
        "retention.operational_days": 90
    ] }

    /// A drawn sample page, used for both the preview and What FERB saw. Never a desktop capture.
    static func image() -> String {
        let image = NSImage(size: NSSize(width: 960, height: 600))
        image.lockFocus()
        NSColor(calibratedWhite: 0.10, alpha: 1).setFill()
        NSRect(x: 0, y: 0, width: 960, height: 600).fill()
        func text(_ value: String, x: CGFloat, y: CGFloat, size: CGFloat, color: NSColor = .white) {
            (value as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: [.font: NSFont.systemFont(ofSize: size), .foregroundColor: color])
        }
        text("PLAYGROUND SAMPLE · NO SCREEN WAS CAPTURED", x: 48, y: 544, size: 12, color: .systemOrange)
        text("Afternoon brief", x: 48, y: 460, size: 40)
        text("A little less noise. A clearer next step.", x: 48, y: 420, size: 20, color: .lightGray)
        for (index, row) in ["Review the launch mockups", "Choose a direction for the studio page", "Leave 90 minutes for focused work"].enumerated() {
            let y = CGFloat(320 - index * 72)
            NSColor(calibratedWhite: 0.17, alpha: 1).setFill()
            NSBezierPath(roundedRect: NSRect(x: 48, y: y, width: 864, height: 56), xRadius: 12, yRadius: 12).fill()
            text(row, x: 70, y: y + 17, size: 19)
        }
        text("Example account detail", x: 48, y: 68, size: 14, color: .lightGray)
        NSColor.black.setFill()
        NSRect(x: 240, y: 66, width: 250, height: 22).fill()
        image.unlockFocus()
        guard let tiff = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:]) else { return "" }
        return png.base64EncodedString()
    }
}
#endif
