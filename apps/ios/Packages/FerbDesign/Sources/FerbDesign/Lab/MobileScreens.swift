// Ported from ~/Desktop/ferb-glass-lab Sources/FerbGlassLab/MobileScreens.swift at 23a0fde (LabSource.commit): the
// twelve phone screens' pages and sheets and the frame they sit in (MobileContent, as PhoneScreen). Kept: every
// layout, number and word of the lab's iPhone. Changed so the app can draw real data with them (kb PHN-509, the way
// Android's design module did): each page and sheet is a stateless view whose data and callbacks are parameters,
// with defaults that draw the lab's sample (LabSample.swift); the pages that hold lists (chat, tasks, files) scroll
// when the list is longer than the screen (the lab's still screens never are); `os` is PhoneInsets. The lab's stage
// (MobileStage, LabModel) is not carried; the gallery (FerbGallery) is its stand-in.

import SwiftUI

// The phone screens. One description, the same on both phones. Every name, task, code and place is invented.

// MARK: The frame

/// Every phone screen (MobileContent): the sky over the owner, the season's layer when `season` is on (the home
/// screen only; it fades out over 1.4 s when switched off, as a chat starts), the page below the status bar, and a
/// sheet over it. The sky's ink and the moment are given to everything inside. It fills the whole screen, under the
/// system's bars: the app passes their size as `insets`.
public struct PhoneScreen<Page: View, SheetContent: View>: View {
    let moment: SkyMoment
    var insets: PhoneInsets = .lab
    var season = false
    /// The sky glides to a new time over 2.2 s (the lab's Stage), unless off.
    var glide = true
    var sheet: SheetContent
    var page: Page

    public init(moment: SkyMoment, insets: PhoneInsets = .lab, season: Bool = false, glide: Bool = true,
                @ViewBuilder sheet: () -> SheetContent, @ViewBuilder page: () -> Page) {
        self.moment = moment
        self.insets = insets
        self.season = season
        self.glide = glide
        self.sheet = sheet()
        self.page = page()
    }

    public var body: some View {
        ZStack {
            SkyView(date: moment.date, latitude: moment.place.latitude, longitude: moment.place.longitude)
                .animation(glide ? .easeInOut(duration: 2.2) : nil, value: moment.date)
                .accessibilityHidden(true)
            // The season's layer is for the home screen only; a chat keeps just the sky, sun, moon and stars.
            ZStack {
                if season, SeasonLayer.hasLayer(moment.season, night: moment.night) {
                    SeasonLayer(season: moment.season, night: moment.night).transition(.opacity)
                }
            }
            .animation(.easeInOut(duration: 1.4), value: season)
            page.padding(.top, insets.top)
            sheet
        }
        .background(Color.black)
        .environment(\.skyInk, moment.ink)
        .environment(\.skyMoment, moment)
        .environment(\.phoneInsets, insets)
        .ignoresSafeArea()
    }
}

public extension PhoneScreen where SheetContent == EmptyView {
    init(moment: SkyMoment, insets: PhoneInsets = .lab, season: Bool = false, glide: Bool = true, @ViewBuilder page: () -> Page) {
        self.init(moment: moment, insets: insets, season: season, glide: glide, sheet: { EmptyView() }, page: page)
    }
}

// MARK: Data

/// A row of work: its mark, title, detail and the value on its right.
public struct WorkRow: Equatable, Sendable {
    public var kind: MarkKind
    public var title: String
    public var detail: String
    public var value: String?
    public init(kind: MarkKind, title: String, detail: String, value: String? = nil) {
        self.kind = kind; self.title = title; self.detail = detail; self.value = value
    }
}

/// A labelled group of rows on the sky.
public struct WorkGroup: Equatable, Sendable {
    public var label: String
    public var rows: [WorkRow]
    public init(label: String, rows: [WorkRow]) { self.label = label; self.rows = rows }
}

/// One thing in the chat.
public enum ChatItem: Equatable, Sendable {
    case owner(String)
    case ferb(String)
    case task(headline: String, progress: String, parts: [PlanPart])
    case typing
}

/// A place on the Mac: its symbol, name and how many things are in it.
public struct PlaceRow: Equatable, Sendable {
    public var symbol: String
    public var title: String
    public var count: String?
    public init(symbol: String, title: String, count: String?) { self.symbol = symbol; self.title = title; self.count = count }
}

/// A file: its name and what is known about it.
public struct FileRow: Equatable, Sendable {
    public var name: String
    public var detail: String
    public init(name: String, detail: String) { self.name = name; self.detail = detail }
}

/// A transfer under way.
public struct Transfer: Equatable, Sendable {
    public var title: String
    public var fraction: Double
    public init(title: String, fraction: Double) { self.title = title; self.fraction = fraction }
}

// MARK: Pages

public struct HomePage: View {
    @Environment(\.skyInk) private var ink
    @Environment(\.skyMoment) private var moment
    /// The words; nil takes the lab's own for the screen's moment (HomeTexts.lab).
    var texts: HomeTexts?
    /// What happened while the owner was away: questions first, then failures, then what finished.
    var away: [WorkRow]
    var insets: PhoneInsets
    var typed: String?
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    var onRow: ((Int) -> Void)?
    var onMore: (() -> Void)?
    /// The line under three rows when there are more; the lab's words by default.
    var more: (Int) -> String
    /// The app's own composer in place of the lab's still one.
    var composer: AnyView?
    /// Not from the lab: the way back to earlier days' messages on a new day (kb PHN-626); none by default.
    var earlier: String?
    var onEarlier: (() -> Void)?
    /// Not from the lab: a card FERB keeps live above the box (the memory import's progress while one runs, kb MEM-457);
    /// none by default.
    var live: AnyView?

    public init(texts: HomeTexts? = nil, away: [WorkRow] = LabSample.away, insets: PhoneInsets = .lab, typed: String? = nil,
                onTab: ((PhoneTab) -> Void)? = nil, onProfile: (() -> Void)? = nil, onRow: ((Int) -> Void)? = nil,
                onMore: (() -> Void)? = nil, more: @escaping (Int) -> String = { "\($0) more in Tasks" }, composer: AnyView? = nil,
                earlier: String? = nil, onEarlier: (() -> Void)? = nil, live: AnyView? = nil) {
        self.live = live
        self.earlier = earlier
        self.onEarlier = onEarlier
        self.texts = texts
        self.away = away
        self.insets = insets
        self.typed = typed
        self.onTab = onTab
        self.onProfile = onProfile
        self.onRow = onRow
        self.onMore = onMore
        self.more = more
        self.composer = composer
    }

    public var body: some View {
        let words = texts ?? HomeTexts.lab(moment ?? .lab(12.5))
        VStack(alignment: .leading, spacing: 0) {
            TopBar(chosen: .chat, waiting: away.contains { $0.kind == .asking } ? .tasks : nil, onTab: onTab, onProfile: onProfile)
            if let earlier { EarlierLink(earlier, onClick: onEarlier).padding(.top, 10) }
            VStack(alignment: .leading, spacing: 6) {
                Text(words.greeting).type(42, .medium, ink.top).fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                Text(words.subline).type(17, .medium, ink.top).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 70)
            .padding(.horizontal, 22)
            if let chat = words.chat {
                FerbBubble(chat, maxWidth: 300)
                    .padding(.horizontal, 16)
                    .padding(.top, 22)
            }
            Spacer()
            VStack(spacing: 0) {
                if let live { live.padding(.bottom, 12) }
                // Nothing happened: no list and no box, only the sky and the message box.
                if !away.isEmpty {
                    SkyLabel(text: words.away)
                    Card { items }
                }
                Group {
                    if let composer { composer } else { PhoneComposer(placeholder: words.placeholder, typed: typed) }
                }
                .padding(.top, 12)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }

    /// At most three rows, then how many more.
    @ViewBuilder private var items: some View {
        ForEach(Array(away.prefix(3).enumerated()), id: \.offset) { i, r in
            if i > 0 { Divide() }
            CardRow(title: r.title, detail: r.detail, onClick: onRow.map { f in { f(i) } }) { Mark(kind: r.kind) } trailing: { if r.kind != .done { Opens() } }
        }
        if away.count > 3 {
            Divide()
            Text(more(away.count - 3)).type(14.5, .medium, Ink.detail).frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 14).padding(.vertical, 12)
                .pressable(onMore)
        }
    }
}

public struct ChatPage: View {
    var items: [ChatItem]
    var insets: PhoneInsets
    var waiting: PhoneTab?
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    var onWatch: ((Int) -> Void)?
    var onStop: ((Int) -> Void)?
    var placeholder: String
    var composer: AnyView?

    public init(items: [ChatItem] = LabSample.chat, insets: PhoneInsets = .lab, waiting: PhoneTab? = .tasks,
                onTab: ((PhoneTab) -> Void)? = nil, onProfile: (() -> Void)? = nil, onWatch: ((Int) -> Void)? = nil,
                onStop: ((Int) -> Void)? = nil, placeholder: String = "Add to this", composer: AnyView? = nil) {
        self.items = items
        self.insets = insets
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
        self.onWatch = onWatch
        self.onStop = onStop
        self.placeholder = placeholder
        self.composer = composer
    }

    public var body: some View {
        VStack(spacing: 0) {
            TopBar(chosen: .chat, waiting: waiting, onTab: onTab, onProfile: onProfile)
            FitOrScroll {
                VStack(spacing: 12) {
                    ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                        switch item {
                        case .owner(let text): OwnerBubble(text)
                        case .ferb(let text): FerbBubble(text)
                        case let .task(headline, progress, parts):
                            TaskCard(headline: headline, progress: progress, parts: parts,
                                     onWatch: onWatch.map { f in { f(i) } }, onStop: onStop.map { f in { f(i) } })
                        case .typing: TypingDots()
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 16)
                .padding(.bottom, 12)
            }
            Group {
                if let composer { composer } else { PhoneComposer(placeholder: placeholder) }
            }
            .padding(.horizontal, 16).padding(.bottom, insets.bottom + 8)
        }
    }
}

/// PhoneTasksPage: what waits for you, what runs, what finished: one white card per group.
public struct TasksPage: View {
    var groups: [WorkGroup]
    var waiting: PhoneTab?
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    var onRow: ((Int, Int) -> Void)?
    /// What shows when there are no groups.
    var empty: AnyView?

    public init(groups: [WorkGroup] = LabSample.tasks, waiting: PhoneTab? = .tasks, onTab: ((PhoneTab) -> Void)? = nil,
                onProfile: (() -> Void)? = nil, onRow: ((Int, Int) -> Void)? = nil, empty: AnyView? = nil) {
        self.groups = groups
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
        self.onRow = onRow
        self.empty = empty
    }

    public var body: some View {
        VStack(spacing: 0) {
            TopBar(chosen: .tasks, waiting: waiting, onTab: onTab, onProfile: onProfile)
            FitOrScroll {
                VStack(spacing: 0) {
                    if groups.isEmpty, let empty { empty }
                    ForEach(Array(groups.enumerated()), id: \.offset) { g, group in
                        SkyLabel(text: group.label)
                        Card {
                            ForEach(Array(group.rows.enumerated()), id: \.offset) { i, row in
                                if i > 0 { Divide() }
                                CardRow(title: row.title, detail: row.detail, onClick: onRow.map { f in { f(g, i) } }) { Mark(kind: row.kind) } trailing: { Opens(value: row.value) }
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, -6)
                .padding(.bottom, 16)
            }
        }
    }
}

/// PhoneFilesPage: search, a transfer, places, recent, and sending files to the Mac.
public struct FilesPage: View {
    var places: [PlaceRow]
    var recent: [FileRow]
    var transfer: Transfer?
    var insets: PhoneInsets
    var waiting: PhoneTab?
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    var onSearch: (() -> Void)?
    var onPlace: ((Int) -> Void)?
    var onFile: ((Int) -> Void)?
    var onSend: (() -> Void)?
    /// The app's own transfer cards in place of `transfer`'s (not from the lab).
    var transfers: AnyView?
    /// What shows instead of Places and Recent, such as an open folder (FolderList; not from the lab).
    var content: AnyView?
    /// Shown under Recent when there are no recent files (not from the lab).
    var recentNote: String?

    public init(places: [PlaceRow] = LabSample.places, recent: [FileRow] = LabSample.recent, transfer: Transfer? = LabSample.transfer,
                insets: PhoneInsets = .lab, waiting: PhoneTab? = .tasks, onTab: ((PhoneTab) -> Void)? = nil,
                onProfile: (() -> Void)? = nil, onSearch: (() -> Void)? = nil, onPlace: ((Int) -> Void)? = nil,
                onFile: ((Int) -> Void)? = nil, onSend: (() -> Void)? = nil, transfers: AnyView? = nil, content: AnyView? = nil,
                recentNote: String? = nil) {
        self.places = places
        self.recent = recent
        self.transfer = transfer
        self.insets = insets
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
        self.onSearch = onSearch
        self.onPlace = onPlace
        self.onFile = onFile
        self.onSend = onSend
        self.transfers = transfers
        self.content = content
        self.recentNote = recentNote
    }

    @Environment(\.skyInk) private var ink

    public var body: some View {
        VStack(spacing: 0) {
            TopBar(chosen: .files, waiting: waiting, onTab: onTab, onProfile: onProfile)
            FitOrScroll {
                VStack(spacing: 0) {
                    HStack(spacing: 10) {
                        Image(systemName: "magnifyingglass").font(.system(size: 17, weight: .medium))
                        Text("Search your Mac").type(16.5, .regular, Ink.onDark)
                        Spacer()
                    }
                    .foregroundStyle(Ink.onDark)
                    .padding(.horizontal, 16)
                    .frame(height: 50)
                    .background(Ink.black, in: Capsule())
                    .pressable(onSearch)
                    .padding(.top, 14)
                    if let transfers {
                        transfers
                    } else if let transfer {
                        TransferCard(title: transfer.title, fraction: transfer.fraction)
                            .padding(.top, 12)
                    }
                    if let content {
                        content
                    } else {
                        SkyLabel(text: "Places")
                        Card {
                            ForEach(Array(places.enumerated()), id: \.offset) { i, p in
                                if i > 0 { Divide() }
                                CardRow(title: p.title, onClick: onPlace.map { f in { f(i) } }) { RowIcon(symbol: p.symbol) } trailing: { Opens(value: p.count) }
                            }
                        }
                        SkyLabel(text: "Recent")
                        if recent.isEmpty, let recentNote {
                            Text(recentNote).type(14.5, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 8)
                        } else {
                            Card {
                                ForEach(Array(recent.enumerated()), id: \.offset) { i, f in
                                    if i > 0 { Divide() }
                                    CardRow(title: f.name, detail: f.detail, onClick: onFile.map { g in { g(i) } }) { FileIcon(name: f.name, width: 26) } trailing: { Opens() }
                                }
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
            }
            Pill(title: "Send files to your Mac", kind: .black, symbol: "arrow.up.doc", onClick: onSend)
                .padding(.horizontal, 16)
                .padding(.bottom, insets.bottom + 8)
        }
    }
}

public struct MacPage: View {
    @Environment(\.skyInk) private var ink
    var link: LinkKind
    var macName: String
    var ownRelay: String?
    var ownRelayUnset: String
    var settingsDetail: String
    var status: String?
    var note: String?
    var name: String?
    var waiting: PhoneTab?
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    var onLink: (() -> Void)?
    var onOwnRelay: (() -> Void)?
    var onSettings: (() -> Void)?
    var onStopAll: (() -> Void)?
    /// Not from the lab: a card above the rows (FERB may use the Mac's screen until the owner is back, PHN-611).
    var notice: AnyView?

    public init(link: LinkKind = .direct, macName: String = "MacBook Pro", ownRelay: String? = "relay.example.net",
                ownRelayUnset: String = "Not set", settingsDetail: String = "Each change asks for your fingerprint",
                status: String? = nil, note: String? = nil, name: String? = nil, waiting: PhoneTab? = .tasks,
                onTab: ((PhoneTab) -> Void)? = nil, onProfile: (() -> Void)? = nil, onLink: (() -> Void)? = nil,
                onOwnRelay: (() -> Void)? = nil, onSettings: (() -> Void)? = nil, onStopAll: (() -> Void)? = nil,
                notice: AnyView? = nil) {
        self.link = link
        self.macName = macName
        self.ownRelay = ownRelay
        self.ownRelayUnset = ownRelayUnset
        self.settingsDetail = settingsDetail
        self.status = status
        self.note = note
        self.name = name
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
        self.onLink = onLink
        self.onOwnRelay = onOwnRelay
        self.onSettings = onSettings
        self.onStopAll = onStopAll
        self.notice = notice
    }

    public var body: some View {
        VStack(spacing: 0) {
            TopBar(chosen: .mac, waiting: waiting, onTab: onTab, onProfile: onProfile)
            Spacer()
            LinkPicture(kind: link, macName: macName, status: status, name: name, onClick: onLink)
            if let note {
                // Not from the lab: a line under the picture when the app has something to say about the link.
                Text(note).type(14, .medium, ink.low).multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity).padding(.horizontal, 32).padding(.top, 14)
            }
            Spacer()
            if let notice { notice }
            Card {
                CardRow(title: "Your own relay", detail: ownRelay ?? ownRelayUnset, onClick: onOwnRelay) { RowIcon(symbol: "server.rack") } trailing: { Opens() }
                Divide()
                CardRow(title: "FERB's settings", detail: settingsDetail, onClick: onSettings) { RowIcon(symbol: "slider.horizontal.3") } trailing: { Opens() }
                Divide()
                CardRow(title: "Stop everything on the Mac", danger: true, onClick: onStopAll) { RowIcon(symbol: "stop.circle", danger: true) }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 40)
        }
    }
}

public struct UnlockPage: View {
    @Environment(\.skyInk) private var ink
    var insets: PhoneInsets
    /// The phone's own kind, as the lab's iPhone words name it ("Face ID"; "Touch ID", "your passcode" elsewhere).
    var bio: String
    var symbol: String
    var onUnlock: (() -> Void)?

    public init(insets: PhoneInsets = .lab, bio: String = "Face ID", symbol: String = "faceid", onUnlock: (() -> Void)? = nil) {
        self.insets = insets
        self.bio = bio
        self.symbol = symbol
        self.onUnlock = onUnlock
    }

    public var body: some View {
        VStack(spacing: 14) {
            Spacer().frame(height: 150)
            FerbMark(size: 84).launchTarget()
            Text("FERB is locked").type(28, .semibold, ink.top).padding(.top, 6).accessibilityAddTraits(.isHeader)
            Text("It opens with \(bio).").type(16, .medium, ink.top)
            Spacer()
            Card(padding: 16) {
                Pill(title: "Unlock with \(bio)", kind: .black, symbol: symbol, onClick: onUnlock).accessibilityIdentifier("unlock")
                Text("Nothing in FERB can be read until it is unlocked.").type(14, .regular, Ink.detail).padding(.top, 12)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}

public struct PairPage: View {
    var insets: PhoneInsets
    var onScan: (() -> Void)?
    var onTypeCode: (() -> Void)?
    var onGetMacApp: (() -> Void)?
    var onHelp: (() -> Void)?

    public init(insets: PhoneInsets = .lab, onScan: (() -> Void)? = nil, onTypeCode: (() -> Void)? = nil,
                onGetMacApp: (() -> Void)? = nil, onHelp: (() -> Void)? = nil) {
        self.insets = insets
        self.onScan = onScan
        self.onTypeCode = onTypeCode
        self.onGetMacApp = onGetMacApp
        self.onHelp = onHelp
    }

    public var body: some View {
        VStack(spacing: 0) {
            Spacer()
            LinkPicture(kind: .search)
            Spacer()
            Card(padding: 20) {
                VStack(alignment: .leading, spacing: 14) {
                    Text("Use FERB from your phone").type(24, .semibold).accessibilityAddTraits(.isHeader)
                    PairStep(1, "On your Mac, open FERB and choose Pair a phone.")
                    PairStep(2, "Scan the code it shows.")
                    PairStep(3, "Check both codes match, then say yes on the Mac.")
                    Pill(title: "Scan the code", kind: .black, symbol: "qrcode.viewfinder", onClick: onScan).padding(.top, 4)
                    HStack {
                        PairAction(symbol: "keyboard", title: "Type code", onClick: onTypeCode)
                        PairAction(symbol: "laptopcomputer", title: "Get the Mac app", onClick: onGetMacApp)
                        PairAction(symbol: "questionmark.circle", title: "Help", onClick: onHelp)
                    }
                    .padding(.top, 2)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}

public struct CodesPage: View {
    var code: String
    var insets: PhoneInsets
    var onCancel: (() -> Void)?

    public init(code: String = LabSample.code, insets: PhoneInsets = .lab, onCancel: (() -> Void)? = nil) {
        self.code = code
        self.insets = insets
        self.onCancel = onCancel
    }

    public var body: some View {
        VStack {
            Spacer()
            Card(padding: 20) {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(spacing: 10) {
                        Circle().fill(Ink.amber).frame(width: 10, height: 10)
                        Text("Now say yes on your Mac").type(22, .semibold).accessibilityAddTraits(.isHeader)
                    }
                    Text("Your Mac shows this phone's name and a code. Say yes there only if it is this one:")
                        .type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                    PairingCode(code)
                        .padding(.vertical, 10)
                    HStack(spacing: 10) {
                        Mark(kind: .running, size: 20)
                        Text("Waiting for your Mac").type(15, .regular, Ink.detail)
                    }
                    .frame(maxWidth: .infinity)
                    Pill(title: "Cancel", kind: .plain, height: 44, onClick: onCancel)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}

// MARK: Sheets

/// A question slides up over whatever is open and shows the exact thing; Yes asks for Face ID.
public struct QuestionSheet<Exact: View>: View {
    var symbol: String
    var title: String
    var reason: String
    var yes: String
    var no: String
    var count: String?
    /// The line under the exact thing; nil leaves it out (the Mac decides what Yes asks for, kb PHN-470).
    var note: String?
    var insets: PhoneInsets
    var onNo: (() -> Void)?
    var onYes: (() -> Void)?
    var onDismiss: (() -> Void)?
    /// Not from the lab: more answers the Mac offers beyond Yes and No (kb PHN-607), each a grey pill under them.
    var more: [(String, (() -> Void)?)]
    /// Not from the lab: one short line under the further answers (what "until I'm back at my Mac" means, PHN-611).
    var moreNote: String?
    /// Not from the lab: FERB.app's header over the question ("Check before FERB goes on", kb PHN-739).
    var header: String?
    /// Not from the lab: what FERB.app shows under the exact thing (the task's reason, the evidence).
    var below: AnyView?
    /// Not from the lab: FERB.app's line under its buttons (what the 30-minute answer covers).
    var answerNote: String?
    var exact: Exact

    public init(symbol: String, title: String, reason: String, yes: String, no: String = "No", count: String? = nil,
                note: String? = "Yes asks for Face ID.", insets: PhoneInsets = .lab, onNo: (() -> Void)? = nil,
                onYes: (() -> Void)? = nil, onDismiss: (() -> Void)? = nil, more: [(String, (() -> Void)?)] = [],
                moreNote: String? = nil, header: String? = nil, below: AnyView? = nil, answerNote: String? = nil,
                @ViewBuilder exact: () -> Exact) {
        self.header = header
        self.below = below
        self.answerNote = answerNote
        self.symbol = symbol
        self.title = title
        self.reason = reason
        self.yes = yes
        self.no = no
        self.count = count
        self.note = note
        self.insets = insets
        self.onNo = onNo
        self.onYes = onYes
        self.onDismiss = onDismiss
        self.more = more
        self.moreNote = moreNote
        self.exact = exact()
    }

    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onDismiss) {
            HStack(alignment: .top, spacing: 14) {
                SheetIcon(symbol: symbol)
                VStack(alignment: .leading, spacing: 4) {
                    if let header { Text(header).type(13.5, .medium, Ink.detail) }
                    Text(title).type(22, .semibold).accessibilityAddTraits(.isHeader)
                    if !reason.isEmpty {
                        Text(reason).type(15.5, .regular, Ink.detail)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 0)
                if let count { Text(count).type(14, .medium, Ink.faint).fixedSize() }
            }
            exact
                .padding(.top, 14)
            if let below { below }
            if let note { BiometricNote(note).padding(.top, 12) }
            // Fixed against the lab: its No and Yes both stretch and Yes has the layout priority, so No is squeezed to
            // nothing (seen in this package's first render). No takes its words' width and Yes the rest, as Android does.
            HStack(spacing: 10) {
                Pill(title: no, kind: .grey, fill: false, onClick: onNo)
                Pill(title: yes, kind: .black, onClick: onYes).frame(maxWidth: .infinity).layoutPriority(1)
            }
            .padding(.top, 18)
            ForEach(Array(more.enumerated()), id: \.offset) { _, option in
                Pill(title: option.0, kind: .grey, onClick: option.1).padding(.top, 10)
            }
            if let moreNote {
                Text(moreNote).type(14, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8).padding(.horizontal, 4)
            }
            if let answerNote {
                Text(answerNote).type(13.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 10).padding(.horizontal, 4)
            }
        }
    }
}

public extension QuestionSheet where Exact == ExactCommand {
    /// The lab's question: run a command in Terminal.
    init(command: String = LabSample.command, reason: String = LabSample.commandReason, count: String? = "1 of 2",
         insets: PhoneInsets = .lab, onNo: (() -> Void)? = nil, onYes: (() -> Void)? = nil, onDismiss: (() -> Void)? = nil) {
        self.init(symbol: "terminal", title: "Run this in Terminal?", reason: reason, yes: "Yes, run it", count: count,
                  insets: insets, onNo: onNo, onYes: onYes, onDismiss: onDismiss) { ExactCommand(command) }
    }
}

/// A private answer goes straight to the Mac, encrypted, and is not kept on the phone. The sheet sits over the keypad.
public struct SecretSheet: View {
    var title: String
    var detail: String
    var filled: Int
    var length: Int
    var insets: PhoneInsets
    var onKey: ((String) -> Void)?
    var onDismiss: (() -> Void)?
    /// What goes under the private line (the app's Send and Decline); nothing in the lab.
    var actions: AnyView?
    /// Not from the lab: FERB.app's header and the task's reason (kb PHN-739).
    var header: String?
    var below: AnyView?

    public init(title: String = "Sign-in code", detail: String = LabSample.secretDetail, filled: Int = 4, length: Int = 6,
                insets: PhoneInsets = .lab, onKey: ((String) -> Void)? = nil, onDismiss: (() -> Void)? = nil, actions: AnyView? = nil,
                header: String? = nil, below: AnyView? = nil) {
        self.header = header
        self.below = below
        self.title = title
        self.detail = detail
        self.filled = filled
        self.length = length
        self.insets = insets
        self.onKey = onKey
        self.onDismiss = onDismiss
        self.actions = actions
    }

    public var body: some View {
        ZStack(alignment: .bottom) {
            Sheet(lift: 4 * 57 + 18 + insets.bottom, bottomInset: insets.bottom, onDismiss: onDismiss) {
                HStack(spacing: 14) {
                    SheetIcon(symbol: "key")
                    VStack(alignment: .leading, spacing: 2) {
                        if let header { Text(header).type(13.5, .medium, Ink.detail) }
                        Text(title).type(19, .semibold).accessibilityAddTraits(.isHeader)
                        if !detail.isEmpty { Text(detail).type(15, .regular, Ink.detail) }
                    }
                }
                if let below { below }
                SecretCodeBoxes(filled: filled, length: length)
                    .padding(.top, 16)
                HStack(spacing: 6) {
                    Image(systemName: "lock.fill")
                    Text("Private: it goes straight to your Mac and is not kept on this phone.")
                }
                .type(14, .regular, Ink.detail)
                .padding(.top, 12)
                if let actions { actions }
            }
            SecretKeypad(bottomInset: insets.bottom, onKey: onKey)
        }
    }
}

/// What the relay sheet says: the explanation, what works there (what, how), and the line about full speed.
public struct RelayCopy: Equatable, Sendable {
    public var explanation: String
    public var works: [Work]
    public var fullSpeed: String
    public struct Work: Equatable, Sendable {
        public var what: String
        public var how: String
        public init(_ what: String, _ how: String) { self.what = what; self.how = how }
    }
    public init(explanation: String, works: [Work], fullSpeed: String) {
        self.explanation = explanation
        self.works = works
        self.fullSpeed = fullSpeed
    }
}

/// On the shared relay the line goes through it; the sheet says what works there and how to get everything.
public struct RelaySheet: View {
    var copy: RelayCopy
    var insets: PhoneInsets
    var trying: Bool
    var onClose: (() -> Void)?

    public init(copy: RelayCopy = LabSample.relay, insets: PhoneInsets = .lab, trying: Bool = true, onClose: (() -> Void)? = nil) {
        self.copy = copy
        self.insets = insets
        self.trying = trying
        self.onClose = onClose
    }

    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onClose) {
            SheetTitle("Through the shared relay", onClose: onClose)
            Text(copy.explanation)
                .type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true).padding(.top, 6)
            VStack(spacing: 0) {
                ForEach(Array(copy.works.enumerated()), id: \.offset) { i, w in
                    if i > 0 { Rectangle().fill(Ink.line).frame(height: 1) }
                    works(w.what, w.how)
                }
            }
            .background(Ink.fill, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .padding(.top, 14)
            if trying {
                HStack(spacing: 10) {
                    Mark(kind: .running, size: 20)
                    Text("Trying for a direct connection").type(15, .regular, Ink.detail)
                }
                .padding(.top, 14)
            }
            Text(copy.fullSpeed)
                .type(14, .regular, Ink.detail).padding(.top, 8).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func works(_ what: String, _ how: String) -> some View {
        HStack {
            Text(what).type(15, .regular)
            Spacer()
            Text(how).type(15, .regular, Ink.detail)
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
        .accessibilityElement(children: .combine)
    }
}

/// The profile button: this phone's own settings, the lock, staying connected, and forgetting the Mac.
public struct ProfileSheet: View {
    var macName: String
    var pairedSince: String
    var insets: PhoneInsets
    var lockOn: Bool
    var stayConnected: Bool
    var onClose: (() -> Void)?
    var onLock: ((Bool) -> Void)?
    var onStayConnected: ((Bool) -> Void)?
    var onForget: (() -> Void)?
    /// The app's own rows, after Stay connected (each starting with a Divide); nothing in the lab.
    var more: AnyView?
    /// The lock row's title ("Face ID to open"; the phone's own kind elsewhere).
    var lockTitle: String
    /// In place of Stay connected (an iPhone has none, kb PHN-566): a row with its words that opens what it says.
    var instead: (title: String, detail: String, onClick: (() -> Void)?)?
    /// PHN-801: the connection's row, first (its state as the top bar's badge shows it; its sheet on a tap).
    var connection: AnyView?
    /// kb PHN-847: "Lock FERB now", right under the lock switch; nothing in the lab.
    var lockNow: AnyView?

    public init(macName: String = "MacBook Pro", pairedSince: String = "10 November", insets: PhoneInsets = .lab,
                lockOn: Bool = true, stayConnected: Bool = true, onClose: (() -> Void)? = nil, onLock: ((Bool) -> Void)? = nil,
                onStayConnected: ((Bool) -> Void)? = nil, onForget: (() -> Void)? = nil, more: AnyView? = nil,
                lockTitle: String = "Face ID to open", instead: (title: String, detail: String, onClick: (() -> Void)?)? = nil,
                connection: AnyView? = nil, lockNow: AnyView? = nil) {
        self.connection = connection
        self.lockNow = lockNow
        self.lockTitle = lockTitle
        self.instead = instead
        self.macName = macName
        self.pairedSince = pairedSince
        self.insets = insets
        self.lockOn = lockOn
        self.stayConnected = stayConnected
        self.onClose = onClose
        self.onLock = onLock
        self.onStayConnected = onStayConnected
        self.onForget = onForget
        self.more = more
    }

    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onClose) {
            SheetTitle("This phone", onClose: onClose)
            VStack(spacing: 0) {
                if let connection {
                    connection
                    Divide()
                }
                CardRow(title: lockTitle, detail: "Every time FERB opens") { RowIcon(symbol: "lock") } trailing: {
                    PhoneSwitch(on: lockOn, onChange: onLock, label: lockTitle)
                }
                if let lockNow {
                    Divide()
                    lockNow
                }
                Divide()
                if let instead {
                    CardRow(title: instead.title, detail: instead.detail, onClick: instead.onClick) { RowIcon(symbol: "bell") } trailing: { Opens() }
                } else {
                    CardRow(title: "Stay connected", detail: "Questions arrive while the app is closed") { RowIcon(symbol: "bell") } trailing: {
                        PhoneSwitch(on: stayConnected, onChange: onStayConnected, label: "Stay connected")
                    }
                }
                if let more { more }
                Divide()
                CardRow(title: "Paired with \(macName)", detail: "Since \(pairedSince)") { RowIcon(symbol: "laptopcomputer") }
                Divide()
                CardRow(title: "Forget this Mac", danger: true, onClick: onForget) { RowIcon(symbol: "link", danger: true) }
            }
            .padding(.horizontal, -14)
            .padding(.top, 8)
        }
    }
}
