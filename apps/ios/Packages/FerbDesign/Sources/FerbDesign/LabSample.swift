import SwiftUI

// The glass lab's twelve phone screens with the lab's own invented data (MobileScreens.swift at 23a0fde: MobileScreen,
// SampleWork, the pages' and sheets' sample words, SkyNow.greeting and sunLine), for the gallery, the screenshots and
// the screens' defaults. Every name, task, code and place is invented; nothing here is FERB's real data. One
// deliberate difference, the same as Android's (engine/JOURNAL.md 101): the relay sheet's rows.

/// MobileScreens.swift MobileScreen: the twelve screens, with the lab's iPhone notes.
public enum LabScreen: String, CaseIterable, Identifiable, Sendable {
    case home, chat, question, secret, tasks, files, mac, relay, profile, unlock, pair, codes
    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .home: return "Home"
        case .chat: return "Chat"
        case .question: return "Question"
        case .secret: return "Private answer"
        case .tasks: return "Tasks"
        case .files: return "Files"
        case .mac: return "Mac"
        case .relay: return "Relay"
        case .profile: return "Profile"
        case .unlock: return "Unlock"
        case .pair: return "Pair"
        case .codes: return "Codes"
        }
    }

    public var note: String {
        switch self {
        case .home: return "Opening FERB: the sky over you right now, its colour from the sun's real height where you are. The sun is FERB's amber dot, where it really is."
        case .chat: return "Tabs at the top. Your words on white, FERB's on black, a task's parts in one white card with Watch live and Stop."
        case .question: return "A question slides up over whatever is open and shows the exact thing. Yes asks for Face ID."
        case .secret: return "A private answer goes straight to the Mac, encrypted, and is not kept on the phone."
        case .tasks: return "What waits for you, what runs, what finished: one white card per group."
        case .files: return "Your Mac's files: search, places, recent, and sending files to the Mac."
        case .mac: return "Your phone and the Mac, joined by a still line of dots, and how they are connected. Nothing moves."
        case .relay: return "On the shared relay the line goes through it; the sheet says what works there and how to get everything."
        case .profile: return "The profile button: this phone's own settings, the lock, staying connected, and forgetting the Mac."
        case .unlock: return "FERB opens with Face ID every time."
        case .pair: return "First run: the Mac is a dashed circle until it is paired. One button."
        case .codes: return "Both screens show the same code; you say yes on the Mac only if they match."
        }
    }
}

/// The lab's sample data.
public enum LabSample {
    /// SampleWork: the home screen's list with the lab's defaults (one waiting, one done).
    public static let waiting: [(String, String)] = [("Renew the domain", "Waiting for your yes"), ("Pay the electricity bill", "Asks before paying $48.20"),
                                                    ("Reply to Priya", "Waiting for your yes"), ("Install the Figma update", "Asks before running a command"),
                                                    ("Sign in to the bank", "Needs a code from you")]
    public static let failed: [(String, String)] = [("Download the October invoice", "The site asked for a code that never came"),
                                                   ("Book the table for Friday", "The restaurant's page didn't load"), ("Back up Photos", "The drive wasn't connected")]
    public static let done: [(String, String)] = [("Backed up the thesis folder", "Copied to the external drive"), ("Cleaned up Downloads", "214 files moved to Archive"),
                                                 ("Summarised the newsletters", "Six sources, saved as a note"), ("Renamed the scans", "38 files"),
                                                 ("Checked the flight", "On time, gate 14"), ("Filed the receipts", "12 receipts in Expenses"),
                                                 ("Updated the slides", "Numbers from the new sheet"), ("Archived old mail", "1,204 emails"),
                                                 ("Compressed the videos", "Saved 3.1 GB")]

    /// HomePage's rows for `waiting`, `done` and `failed` counts: questions first, then failures, then what finished.
    public static func awayRows(waiting w: Int = 1, done d: Int = 1, failed f: Int = 0) -> [WorkRow] {
        waiting.prefix(w).map { WorkRow(kind: .asking, title: $0.0, detail: $0.1) } +
            failed.prefix(f).map { WorkRow(kind: .failed, title: $0.0, detail: $0.1) } +
            done.prefix(d).map { WorkRow(kind: .done, title: $0.0, detail: $0.1) }
    }

    public static let away: [WorkRow] = awayRows()

    /// ChatPage.
    public static let chat: [ChatItem] = [
        .owner("What's left on my calendar today?"),
        .ferb("Two things: the design review at 4, and a call with the bank at 6:30."),
        .owner("Compare the three standing desks I bookmarked and put the cheapest in my cart"),
        .task(headline: "On it. Three parts.", progress: "2 of 3", parts: [
            PlanPart(kind: .done, title: "Open your three desks", detail: "3 pages read"),
            PlanPart(kind: .running, title: "Compare price and delivery", detail: "Reading delivery options"),
            PlanPart(kind: .waiting, title: "Put the cheapest in your cart"),
        ]),
    ]

    /// PhoneTasksPage.
    public static let tasks: [WorkGroup] = [
        WorkGroup(label: "Waiting for you", rows: [WorkRow(kind: .asking, title: "Renew the domain", detail: "Asks before paying $12.99")]),
        WorkGroup(label: "Running", rows: [
            WorkRow(kind: .running, title: "Compare standing desks", detail: "Comparing price and delivery", value: "2 of 3"),
            WorkRow(kind: .running, title: "Summarise this week's newsletters", detail: "Reading 4 of 6"),
        ]),
        WorkGroup(label: "Earlier today", rows: [
            WorkRow(kind: .done, title: "Back up the thesis folder", detail: "Copied to the external drive"),
            WorkRow(kind: .failed, title: "Download the October invoice", detail: "The site asked for a code that never came"),
            WorkRow(kind: .done, title: "Clean up Downloads", detail: "214 files moved to Archive"),
        ]),
    ]

    /// PhoneFilesPage.
    public static let places: [PlaceRow] = [
        PlaceRow(symbol: "menubar.dock.rectangle", title: "Desktop", count: "38"),
        PlaceRow(symbol: "doc", title: "Documents", count: "112"),
        PlaceRow(symbol: "arrow.down.circle", title: "Downloads", count: "9"),
    ]
    public static let recent: [FileRow] = [FileRow(name: "Thesis draft v7.docx", detail: "2.4 MB, edited today"),
                                           FileRow(name: "Lease agreement.pdf", detail: "1.1 MB, Monday")]
    public static let transfer = Transfer(title: "Sending 2 photos to your Mac", fraction: 0.64)

    /// QuestionSheet.
    public static let command = "brew upgrade --cask figma"
    public static let commandReason = "To update Figma before opening the file you asked for."

    /// SecretSheet.
    public static let secretDetail = "The 6-digit code Northfield Energy sent you."

    /// CodesPage: "48", "27", "19".
    public static let code = "482719"

    /// The relay sheet's words. The lab's rows ("Small ones", "Off") were its old rule; the owner's is that the shared
    /// relay carries text, questions and Stop only (kb PHN-410, PHN-444): files, pictures and Watch live wait for a
    /// direct path or the owner's own relay. Android's sample says the same (its preview/labRelay); the phone keeps
    /// trying for a direct path meanwhile (PHN-425, PHN-445).
    public static let relay = RelayCopy(
        explanation: "Your phone and Mac can't reach each other directly on these networks, so they meet at a free shared relay. " +
            "It only passes on what is already encrypted, and only messages, questions and Stop: files, pictures and Watch live " +
            "wait until your phone reaches your Mac directly or through your own relay.",
        works: [
            .init("Messages, questions, answers, Stop", "Always"),
            .init("Files and pictures", "Not on the shared relay"),
            .init("Watch live", "Not on the shared relay"),
        ],
        fullSpeed: "For everything at full speed: the same Wi-Fi as your Mac, or your own relay.")

    /// The lab's own relay rows, before PHN-444 (kept to show what changed; not drawn by default).
    public static let labRelayRows: [RelayCopy.Work] = [
        .init("Messages, questions, answers, Stop", "Always"), .init("Files and pictures", "Small ones"), .init("Watch live", "Off"),
    ]

    /// The gallery's times of day (Android's LAB_TIMES): name and local hour.
    public static let times: [(String, Double)] = [("Dawn", 6.1), ("Sunrise", 6.9), ("Morning", 9.0), ("Noon", 12.5),
                                                  ("Golden hour", 16.6), ("Sunset", 17.4), ("Dusk", 17.9), ("Night", 22.5)]
}

/// MobileScreens.swift SkyNow.sunLine's time left: "45 min", "2 h 12 min", and whole hours without the zero minutes
/// ("5 h", where the lab wrote "5 h 0 min"): the owner's choice, kb PHN-595. The design's spacing is kept. Android's
/// twin is :design's labMinutesLeft; both are held to apps/android/design/src/test/resources/words/time-left.json.
public func labMinutesLeft(_ left: Int) -> String {
    if left < 60 { return "\(left) min" }
    return left % 60 == 0 ? "\(left / 60) h" : "\(left / 60) h \(left % 60) min"
}

public extension HomeTexts {
    /// SkyNow.greeting and sunLine: the lab's stand-in words for the home screen (the app's own come from the corpus:
    /// HomeTexts(Moment)). Times are written as the lab writes them ("h:mm a").
    static func lab(_ now: SkyMoment) -> HomeTexts {
        let greeting: String
        switch now.place.calendar.component(.hour, from: now.date) {
        case 5..<12: greeting = "Good morning"
        case 12..<17: greeting = "Good afternoon"
        case 17..<22: greeting = "Good evening"
        default: greeting = "Still up?"
        }
        let day = now.sunDay
        let line: String
        if let rise = day.sunrise, let set = day.sunset {
            if now.date < rise { line = "Sunrise at \(now.place.clock(rise))." }
            else if now.date < set {
                let left = Int(set.timeIntervalSince(now.date) / 60)
                line = "Sunset at \(now.place.clock(set)), in \(labMinutesLeft(left))."
            } else { line = "The sun set at \(now.place.clock(set))." }
        } else {
            line = "No sunrise or sunset here today."
        }
        return HomeTexts(greeting: greeting, subline: line)
    }
}

/// One lab screen at a moment, drawn with the lab's sample (MobileContent).
public struct LabScreenView: View {
    let screen: LabScreen
    let moment: SkyMoment
    var insets: PhoneInsets = .lab
    var glide = true

    public init(screen: LabScreen, moment: SkyMoment, insets: PhoneInsets = .lab, glide: Bool = true) {
        self.screen = screen
        self.moment = moment
        self.insets = insets
        self.glide = glide
    }

    public var body: some View {
        PhoneScreen(moment: moment, insets: insets, season: screen == .home, glide: glide) {
            switch screen {
            case .question: QuestionSheet(insets: insets)
            case .secret: SecretSheet(insets: insets)
            case .relay: RelaySheet(insets: insets)
            case .profile: ProfileSheet(insets: insets)
            default: EmptyView()
            }
        } page: {
            switch screen {
            case .home, .profile: HomePage(insets: insets)
            case .chat, .question, .secret: ChatPage(insets: insets)
            case .tasks: TasksPage()
            case .files: FilesPage(insets: insets)
            case .mac: MacPage(link: .direct)
            case .relay: MacPage(link: .relay)
            case .unlock: UnlockPage(insets: insets)
            case .pair: PairPage(insets: insets)
            case .codes: CodesPage(insets: insets)
            }
        }
    }
}
