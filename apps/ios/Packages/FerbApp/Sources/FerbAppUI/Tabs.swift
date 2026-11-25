import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import PhotosUI
import ImageIO
import SwiftUI
import UniformTypeIdentifiers

/// The tabs (Mobile.swift TopBar; Android's Main.kt): chat (the home screen until a chat starts), tasks, files, the
/// Mac; a short note over the page.
struct MainView: View {
    @Bindable var model: AppModel
    let shell: AppShell
    let insets: PhoneInsets
    let moment: SkyMoment
    @State private var picking = Picking()

    var body: some View {
        let waiting: PhoneTab? = (!model.ui.questions.isEmpty && model.tab != .tasks) ? .tasks : nil
        ZStack(alignment: .top) {
            // one tab bar whose pill slides; the page under it comes in from the side of its tab (kb PHN-622)
            TabBarHost(chosen: model.tab, waiting: barWaiting(waiting), onTab: { model.choose($0) }, onProfile: { model.show(.profile) },
                       temporary: model.tab == .chat ? temporaryToggle(model) : nil) {
                MotionSwitch(TabKey(tab: model.tab, home: model.tab == .chat && !model.earlier && model.home,
                                    earlier: model.tab == .chat && model.earlier,
                                    temporary: model.tab == .chat && model.temporary != nil), kind: .tab,
                             direction: { a, b in (PhoneTab.allCases.firstIndex(of: b.tab) ?? 0) - (PhoneTab.allCases.firstIndex(of: a.tab) ?? 0) }) { k in
                    switch k.tab {
                    case .chat:
                        if k.home { HomeView(model: model, insets: insets, moment: moment, picking: picking) }
                        else { ChatView(model: model, insets: insets, waiting: waiting, picking: picking) }
                    case .tasks:
                        TasksView(model: model)
                    case .files:
                        FilesView(model: model, insets: insets, waiting: waiting, picking: picking)
                    case .mac:
                        MacView(model: model, waiting: waiting)
                    }
                }
            }
            if let n = model.ui.note {
                NoteBanner(text: n.text) { model.note(nil) }
                    .padding(.horizontal, 16).padding(.top, 64)
                    .task(id: n) {
                        try? await Task.sleep(nanoseconds: 5_000_000_000)
                        if model.ui.note == n { model.note(nil) }
                    }
            }
        }
        .ferbPickers(picking, model: model)
    }
}

/// The page the tab host shows: the tab, and for the chat whether it is still the home screen.
/// The tab and, for the chat, its page (kb PHN-696): today's home page, today's chat, or the earlier days.
private struct TabKey: Hashable {
    let tab: PhoneTab
    let home: Bool
    var earlier = false
    /// The temporary chat in place of the chat (the owner, 2026-11-23): its own page, so on and off change pages.
    var temporary = false
}

/// The temporary chat's dashed bubble in the chat tab's top bar: one tap starts it, the same tap ends it (the owner,
/// 2026-11-23, as ChatGPT's and Claude's apps); it waits while a toggle is on its way.
@MainActor func temporaryToggle(_ model: AppModel) -> TemporaryToggle {
    let on = model.temporary != nil
    return TemporaryToggle(on: on, label: L.s(on ? "temporary_chat_end" : "temporary_chat"), busy: model.ui.busy.contains("temporary"),
                           menuLabel: L.s("top_bar_more")) {
        model.toggleTemporary()
    }
}

extension MainView {
    /// The amber dot as each page's own bar gives it (the home screen's comes from what happened while away; Tasks
    /// never has one).
    fileprivate func barWaiting(_ waiting: PhoneTab?) -> PhoneTab? {
        switch model.tab {
        case .chat where model.home: awayRows(model.ui).contains { $0.kind == .asking } ? .tasks : nil
        case .tasks: nil
        default: waiting
        }
    }
}

/// A short line over the page; it goes by itself after a few seconds, and on a tap.
struct NoteBanner: View {
    let text: String
    let close: () -> Void
    var body: some View {
        Card(padding: 14) {
            Text(text).type(15, .regular, Ink.black).frame(maxWidth: .infinity, alignment: .leading)
        }
        .onTapGesture(perform: close)
        .accessibilityAddTraits(.updatesFrequently)
    }
}

// ---- picking files for the Mac (PHN-115, PHN-127): the system's pickers, no permission ---------------------------------

/// What the composer's + and the Files tab's buttons opened: the Files picker, the Photos picker, or a folder.
@Observable
final class Picking {
    var forChat = false
    var choosing = false
    var files = false
    var photos = false
    var folder = false
    var photoItems: [PhotosPickerItem] = []

    func ask(forChat: Bool) {
        self.forChat = forChat
        choosing = true
    }
}

/// A photo from the Photos picker, as a file copied into FERB's own temporary folder.
struct PickedPhoto: Transferable {
    let url: URL
    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(contentType: .item) { SentTransferredFile($0.url) } importing: { received in
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("picked-\(UUID().uuidString)")
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let to = dir.appendingPathComponent(received.file.lastPathComponent)
            try FileManager.default.copyItem(at: received.file, to: to)
            return PickedPhoto(url: to)
        }
    }
}

extension View {
    func ferbPickers(_ p: Picking, model: AppModel) -> some View {
        modifier(Pickers(p: p, model: model))
    }
}

private struct Pickers: ViewModifier {
    @Bindable var p: Picking
    let model: AppModel

    func body(content: Content) -> some View {
        content
            .confirmationDialog("", isPresented: $p.choosing, titleVisibility: .hidden) {
                Button(L.s("pick_photos")) { p.photos = true }
                Button(L.s("pick_files")) { p.files = true }
                // a whole folder goes from the Files tab; the chat's box takes files and photos (kb PHN-690)
                if !p.forChat { Button(L.s("files_send_folder")) { p.folder = true } }
                Button(L.s("cancel"), role: .cancel) {}
            }
            .fileImporter(isPresented: $p.files, allowedContentTypes: [.item], allowsMultipleSelection: true) { r in
                if case .success(let urls) = r, !urls.isEmpty { model.sendFiles(urls, forChat: p.forChat) }
            }
            .photosPicker(isPresented: $p.photos, selection: $p.photoItems, maxSelectionCount: 20, matching: nil)
            .onChange(of: p.photoItems) { _, items in
                guard !items.isEmpty else { return }
                p.photoItems = []
                let forChat = p.forChat
                Task {
                    var urls: [URL] = []
                    for item in items { if let photo = try? await item.loadTransferable(type: PickedPhoto.self) { urls.append(photo.url) } }
                    if urls.count < items.count { model.note(Note("files_unreadable")) }
                    if !urls.isEmpty { await model.sendFiles(urls, forChat: forChat).value }
                    for u in urls { try? FileManager.default.removeItem(at: u.deletingLastPathComponent()) }
                }
            }
            .fileImporter(isPresented: $p.folder, allowedContentTypes: [.folder], allowsMultipleSelection: false) { r in
                if case .success(let urls) = r, let u = urls.first { model.sendFolder(u, forChat: p.forChat) }
            }
    }
}

// ---- the composer every page shares --------------------------------------------------------------------------------

/// The owner's draft and send; + picks files that wait in the box as chips until he sends (kb PHN-690: nothing leaves
/// the phone before Send; the cross takes one out), with files already on the Mac from the Files tab beside them.
struct Composer: View {
    @Bindable var model: AppModel
    let placeholder: String
    let picking: Picking
    /// The home page has no chat to float over: the toast comes in above the box here (kb APP-1109).
    var toast = false
    @Environment(\.skyInk) private var ink
    @FocusState private var boxFocused: Bool

    var body: some View {
        let t = model.transfers
        let chips = model.outgoing.chips
        // the temporary chat's box is the chat's: + and its files too (the owner, 2026-11-24, ORCH-715)
        let temporary = model.temporary != nil
        let hasFiles = !chips.isEmpty || !t.ready.isEmpty
        VStack(spacing: 8) {
            if toast { AlertToastView(model: model) }
            // the work row above the message box (kb PHN-725; FERB.app ChatBody.swift WorkRowView)
            // (a temporary chat's own while it shows)
            if let r = temporary ? model.temporaryRunning : model.ui.running { WorkRowFor(model: model, r: r) }
            // PHN-643: what the microphone is doing, in words above the box
            if let micText {
                Text(micText).type(13.5, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
                    .accessibilityAddTraits(.updatesFrequently)
            }
            // hold to speak (PHN-603): the words land in the box while held and stay there; never sent by themselves
            PhoneComposer(placeholder: hasFiles ? L.s("chat_placeholder_files") : placeholder, text: Binding(get: { model.draft }, set: { model.typed($0) }),
                          onSend: { model.send() }, onAttach: { picking.ask(forChat: true) },
                          onMicDown: {
                              if model.voicePress() == .askMicrophone {
                                  Task { model.microphoneAnswered(await AppleSpeech.ask()) }
                              }
                          },
                          onMicUp: { model.voiceRelease() }, listening: model.listening,
                          listeningText: micText ?? L.s("voice_listening"), micLabel: L.s("voice_speak"),
                          micState: model.mic, micLevel: model.micLevel,
                          attachments: !hasFiles ? nil : AnyView(AttachmentRow {
                              ForEach(chips, id: \.key) { c in
                                  AttachmentChip(c.name, detail: ByteCountFormatter.string(fromByteCount: c.size, countStyle: .file),
                                                 thumbnail: Thumbnail.of(c.path, c.name),
                                                 removeLabel: L.s("attach_remove", c.name)) { model.unpick(c.key) }
                              }
                              ForEach(t.ready, id: \.uploadId) { a in
                                  AttachmentChip(a.name, detail: L.s("attach_on_mac"), removeLabel: L.s("attach_remove", a.name)) { t.unattach(a.uploadId) }
                              }
                          }.accessibilityIdentifier("attachments")),
                          focus: $boxFocused)
        }
        // FERB's own question (a clarify) is answered by a message: its notification's tap puts the cursor here
        .task(id: model.focusAsk) {
            if model.focusAsk { boxFocused = true; model.focusTaken() }
        }
        // a tick when FERB starts and stops hearing him
        .sensoryFeedback(trigger: model.mic) { old, new in
            if new == .listening && old != .listening { return .start }
            if old == .listening && new != .listening { return .stop }
            return nil
        }
    }

    private var micText: String? {
        switch model.mic {
        case .readying: L.s("voice_getting_ready")
        case .listening: L.s("voice_listening")
        case .finishing: L.s("voice_finishing")
        case .idle: nil
        }
    }
}

// ---- home ------------------------------------------------------------------------------------------------------

struct HomeView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let moment: SkyMoment
    let picking: Picking

    var body: some View {
        let texts = model.homeWords(moment) ?? HomeTexts(greeting: "Hello", subline: "")
        let away = awayRows(model.ui)
        let asking = L.s("question_default_title")
        HomePage(texts: texts, away: away.map { WorkRow(kind: $0.kind, title: $0.title.isEmpty ? asking : $0.title, detail: $0.detail) },
                 insets: insets, onTab: { model.choose($0) }, onProfile: { model.show(.profile) },
                 onRow: { i in if i < away.count { open(model, away[i]) } }, onMore: { model.choose(.tasks) },
                 composer: AnyView(Composer(model: model, placeholder: texts.placeholder, picking: picking, toast: true)),
                 // a new day with nothing said yet: earlier days are one tap away (kb PHN-626)
                 earlier: model.rows.contains { if case .older = $0 { return false }; return true } ? L.s("older_available") : nil,
                 onEarlier: { model.openChat() },
                 // the memory import's live card shows on the home page too while one runs (the owner, kb MEM-457)
                 live: homeImportCard(model.ui.status?.memoryImport))
    }
}

/// The memory import's card above the home page's box while one runs (kb MEM-457), on FERB's dark surface as in the
/// chat; nil when there is none.
func homeImportCard(_ card: ImportCard?) -> AnyView? {
    guard let card, !card.rows.isEmpty else { return nil }
    return AnyView(MemoryImportCard(title: card.title, rows: card.rows.map {
        MemoryImportRow(key: $0.key, name: $0.name, line: $0.line, detail: $0.detail, done: Int($0.done), total: Int($0.total),
                        count: $0.count, foundLine: $0.foundLine, note: $0.note)
    })
    .environment(\.onFerb, true)
    .accessibilityIdentifier("memory-import"))
}

/// What happened while the owner was away: questions first, then failures, then what finished (HomePage).
func awayRows(_ ui: UiState) -> [WorkItem] {
    ui.work.waiting + ui.away.filter { $0.kind == .failed } + ui.away.filter { $0.kind != .failed }
}

/// A day's start as words: "Today", "Yesterday", else the weekday and date in the phone's language (kb PHN-627).
/// The chat's "today" as the model holds it (kb PHN-697); ChatModel.chatDay itself where none is held yet.
func chatDayOf(_ ui: UiState) -> Int { ui.chatDay > 0 ? ui.chatDay : ChatModel.chatDay(Date(), Calendar.current) }

/// A day's start as words: "Today", "Yesterday", else the weekday and date in the phone's language (kb PHN-627).
func dayLabel(_ day: Int, today: Int = ChatModel.chatDay(Date(), Calendar.current)) -> String {
    if day == today { return L.s("day_today") }
    if day == today - 1 { return L.s("day_yesterday") }
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(identifier: "UTC")!
    let date = Date(timeIntervalSince1970: Double(day) * 86_400)
    let f = DateFormatter()
    f.timeZone = utc.timeZone
    f.setLocalizedDateFormatFromTemplate(today - day < 300 ? "EEEEdMMMM" : "EEEEdMMMMy")
    return f.string(from: date)
}

@MainActor func open(_ model: AppModel, _ item: WorkItem) {
    if let id = item.approvalId { model.goToQuestion(id) } else { model.openChat() }
}

// ---- chat ------------------------------------------------------------------------------------------------------

struct ChatView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let waiting: PhoneTab?
    let picking: Picking
    @Environment(\.skyInk) private var ink
    @State private var tallest: CGFloat = 0

    var body: some View {
        VStack(spacing: 0) {
            TopBar(chosen: .chat, waiting: waiting, onTab: { model.choose($0) }, onProfile: { model.show(.profile) },
                   temporary: temporaryToggle(model))
            // the temporary chat says what it is above everything in it
            let temporary = model.temporary
            if temporary != nil {
                TemporaryBanner(L.s("temporary_banner")).padding(.top, 10).accessibilityIdentifier("temporary-banner")
            }
            // the app's one status line: small, grey on a black capsule, its tone's mark (kb PHN-660, PHN-661)
            if let (text, tone) = statusLine {
                StatusLine(text, tone: tone).padding(.top, 10).transition(.motionItem)
            }
            if let need = model.ui.macNeed {
                MacNeedCard(need: need, busy: model.ui.macLine != nil) { model.unlockMac() }
            }
            // two places, as FERB.app separates days (kb PHN-696): today's chat (only today's rows, the way to earlier
            // days at its top) or the earlier days (one continuous chat with a day's start between days), never both
            let earlier = model.earlier
            let todayDay = chatDayOf(model.ui)
            // messages he sent with files go at the end, after the outbox, until their files are on the Mac (PHN-691)
            // made by the model when the rows change, off the main actor for a long chat (kb PHN-714)
            let places = model.places
            // a question the chat doesn't hold yet (the Mac's status brought it first) is a box at the end of today's
            // chat, as FERB.app draws its open questions (ConversationTranscript.swift:205-208; kb PHN-770)
            let placed = Set(model.rows.compactMap { if case .asking(_, let id, _, _, _) = $0 { return id } else { return nil } })
            // (a temporary chat's questions are its own, shown only in it)
            let loose: [ChatRow] = model.ui.questions.filter { !placed.contains($0.approvalId) && temporary?.owns($0) != true }
                .map { .asking(key: "q:\($0.approvalId)", approvalId: $0.approvalId, title: $0.title, reason: nil, hidden: $0.hidden) }
            // the way to earlier days at the top when there are some (rows of them, or older pages the Mac still has),
            // and the memory import's card last while one runs, never in the earlier days (kb MEM-456); a temporary
            // chat shows only itself
            // (its messages with files on their way at its end, as the chat's: ORCH-715)
            let rows = temporary.map { t in
                t.shown(model.ui.questions)
                    + model.outgoing.messages.filter(\.temporary).map { ChatRow.withFiles(key: "send:\($0.id)", item: $0) }
            } ?? ChatModel.shown(places, earlier: earlier, loose: loose, importing: model.ui.status?.memoryImport)
            let endDay = ChatModel.dayAt(rows, rows.count - 1)
            // the day he is looking at and the latest messages' day: the sticky label and the way back (PHN-662, 663)
            let shownDay = { (s: ListScroll) -> Int? in
                s.topKey.flatMap { k in rows.firstIndex { $0.key == k } }.flatMap { ChatModel.dayAt(rows, $0) }
            }
            ZStack(alignment: .bottom) {
                if temporary != nil {
                    // an empty temporary chat: how it ends, where its conversation will be
                    if rows.isEmpty {
                        TemporaryEmpty(L.s("temporary_empty")).frame(maxHeight: .infinity).accessibilityIdentifier("temporary-empty")
                    }
                } else if rows.isEmpty && model.ui.conversation == nil && model.ui.link?.phase == .online {
                    // nothing stored yet and the first page on its way: the waiting look, never a blank screen (PHN-628)
                    ChatWaiting(L.s("chat_loading")).frame(maxHeight: .infinity, alignment: .top)
                }
                // the one list every conversation on the phone uses (kb PHN-826): follows the end, the jump control, the
                // keyboard; the chat adds its days, its question boxes and its toast
                FollowingList(end: rows.last.map { last in
                                  // his own message always goes to the end (PHN-628)
                                  let mine: Bool = { switch last { case .outbox, .withFiles: true; default: false } }()
                                  return ListEnd(key: last.key, mine: mine, value: last)
                              },
                              keys: rows.map(\.key),
                              // with the way to today always there, the end keeps room for it (kb PHN-698)
                              padding: EdgeInsets(top: 16, leading: 16, bottom: earlier ? 64 : 12, trailing: 16),
                              // the Chat tab tapped on today's chat: to its end (kb PHN-662; in the earlier days it goes
                              // back to today, PHN-698); a notification's tap or the Tasks tab's row: the question's box
                              // in view (kb PHN-770)
                              toEnd: model.chatToEnd, reveal: model.toQuestion.map { ListReveal(id: "q:\($0.id)", n: $0.n) },
                              jump: { s in
                                  let jump = ChatJump.of(atEnd: s.atEnd, unseen: s.unseen, shownDay: shownDay(s), endDay: endDay, earlier: earlier)
                                  if jump == .none { return nil }
                                  return ListJump(text: jump == .newMessage ? L.s("chat_new_message")
                                                    : jump == .day ? endDay.map { dayLabel($0, today: todayDay) }
                                                    : jump == .today ? L.s("day_today") : nil,
                                                  label: L.s(jump == .today ? "chat_to_today" : "chat_to_latest"),
                                                  action: jump == .today ? { model.backToToday() } : nil)
                              },
                              over: { s in
                                  // the day he is in, held at the top while he looks through the chat (kb PHN-663)
                                  if earlier, !s.atEnd, let day = shownDay(s), let top = s.topKey.flatMap({ k in rows.firstIndex { $0.key == k } }),
                                     !Self.isDay(rows[top]) {
                                      StickyDay(dayLabel(day, today: todayDay)).padding(.top, 8)
                                          .frame(maxHeight: .infinity, alignment: .top)
                                          .transition(.motionItem)
                                          .accessibilityIdentifier("sticky-day")
                                  }
                                  // FERB's alerts float over the chat's foot as a toast: no room of their own (kb APP-1109)
                                  AlertToastView(model: model).padding(.horizontal, 16).padding(.bottom, 8)
                                      .frame(maxHeight: .infinity, alignment: .bottom)
                              }) {
                    ForEach(rows) { row in
                        // each row gets only what it draws: a streamed word re-evaluates its own row (kb PHN-714)
                        ChatRowView(model: model, row: row, today: todayDay, olderLoading: model.ui.busy.contains("older")).id(row.key)
                            // only the earlier days load older pages; today's chat never does
                            .onAppear { if earlier, case .older(_, .available) = row { model.loadOlder() } }
                            .transition(.motionItem)  // a row that arrives rises in (kb PHN-624)
                    }
                }
                // the chat's height as answers fold to it (FERB.app's CollapsePolicy): the tallest it has been, so the
                // keyboard never folds or unfolds what is on screen; the earlier days' cards are a record (APP-769)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { h in if h > tallest { tallest = h } }
                .environment(\.chatViewport, tallest)
                .environment(\.cardsLive, !earlier)
                .environment(\.questions, model.ui.questions)
                .environment(\.busy, model.ui.busy)
            }
            Composer(model: model, placeholder: L.s("chat_placeholder"), picking: picking)
                .padding(.horizontal, 16).padding(.bottom, insets.bottom + 8)
        }
        .animation(FerbMotion.item.animation, value: statusLine?.0)
    }

    private static func isDay(_ row: ChatRow) -> Bool { if case .day = row { return true } else { return false } }

    /// The line under the tabs: the Mac can't be reached, or its unlock is on its way or slow (PHN-641). What needs the
    /// owner is the card (PHN-649), never this line.
    /// The line under the tabs: only what the top bar's connection control cannot say (kb PHN-756). The link's state is
    /// the control's (PHN-751), and each message that waits says so in its own row ("Waiting for your Mac"), so the
    /// line never speaks of the link. Left: the Mac's unlock once connected, and screen use.
    private var statusLine: (String, StatusTone)? {
        // the Mac asleep in a brief Power Nap wake says so in its own words first (kb SENSE-430)
        if let line = MacAsleep.line(model.ui.status, model.ui.link) { return (line, .info) }
        // the one thing about the link the control can't say: insomnia mode was on, so the Mac may be asleep (APP-981)
        if MacAsleep.shown(model.ui.status, model.ui.link, lost: model.ui.linkLost) { return (L.s("mac_asleep"), .info) }
        // not connected: the control in the top bar says it, the waiting rows say they wait
        if model.ui.link?.phase != .online { return nil }
        // PHN-641: the Mac's side after the app opened: words only, it clears by itself
        if model.ui.macLine == .opening { return (L.s("mac_opening"), .working) }
        if model.ui.macLine == .slow { return (L.s("mac_slow"), .working) }
        // FERB may use the Mac's screen until the owner is back (PHN-611; "Ask me again" is on the Mac tab)
        if model.ui.status?.screenUse?.active == true { return (L.s("screen_use_active"), .info) }
        return nil
    }
}

struct ChatRowView: View {
    @Bindable var model: AppModel
    let row: ChatRow
    /// The chat's today and whether older pages are on their way, handed in: a row never reads the model's whole state.
    var today: Int
    var olderLoading: Bool
    @Environment(\.skyInk) private var ink

    /// Whatever FERB says or did sits on FERB's dark surface; the owner's own messages stay white (kb PHN-734).
    private var fromFerb: Bool {
        switch row {
        case .owner, .outbox, .withFiles, .day, .earlierLink, .older: return false
        default: return true
        }
    }

    var body: some View {
        content.environment(\.onFerb, fromFerb)
    }

    @ViewBuilder private var content: some View {
        switch row {
        case .owner(_, let text, let files, let note):
            VStack(alignment: .trailing, spacing: 4) {
                OwnerBubble(text)
                ForEach(files, id: \.self) { f in
                    HStack(spacing: 6) {
                        FileIcon(name: f, width: 18)
                        Text(f).type(13.5, .medium, ink.low).lineLimit(1)
                    }
                }
                if let note { Text(note).type(13.5, .medium, ink.low).padding(.horizontal, 6) }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        case .ferb(_, let text):
            FerbBubble(text).frame(maxWidth: .infinity, alignment: .leading)
        case .line(_, let text, let quotes, let since):
            VStack(alignment: .leading, spacing: 4) {
                // a connection line ticks its time as FERB.app's LiveLineView ("… · 0:42")
                if let since { TickingText(text, since: since, size: 14.5, color: ink.low) } else { Text(text).type(14.5, .medium, ink.low) }
                ForEach(quotes, id: \.self) { Text($0).type(14, .regular, ink.low) }
            }
            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
        // FERB.app's status words and notices under a reply: 10 points under its 12 (the phone's 16 scaled the same), grey,
        // "Delivered, with parts not done" amber, on FERB's dark surface so either reads on any sky (kb PHN-773)
        case .small(_, let text, let amber):
            Text(text).type(13.5, .medium, amber ? Ink.amber : Ink.onDark)
                .padding(.horizontal, 10).padding(.vertical, 4).background(Ink.black, in: Capsule())
                .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
        case .task(let t):
            // each running part's own live picture (kb PHN-726; FERB.app's RowPreview), asked for while the card shows
            let keys = t.taskId.map { task in t.watch.map { $0.map { Thumbnails.key(task, $0) } } } ?? []
            // FERB.app's card (kb PHN-737): no buttons in it; a part's picture opens its short view, a running part with no
            // picture its task's (Watch live and Stop are there), and View tasks is under the chat
            PartsCard(headline: t.headline, progress: L.s("progress", t.done, t.total), parts: t.parts, tint: t.tint,
                      status: t.status, footer: t.footer, footerLines: t.footerLines,
                      watching: keys.map { $0 != nil }, pictures: keys.map { $0.flatMap { model.thumbnails.pictures[$0] } },
                      onPicture: { i in if i < t.watch.count { model.showTask(t.taskId, worker: t.watch[i]) } },
                      onPart: { _ in model.showTask(t.taskId, worker: nil) },
                      more: L.s("show_more"), less: L.s("show_less"))
                .onAppear { model.rowPictures(t.key, keys.compactMap { $0 }) }
                .onChange(of: keys) { _, now in model.rowPictures(t.key, now.compactMap { $0 }) }
                .onDisappear { model.rowPictures(t.key, []) }
        case .rich(let key, let blocks, let copy, let tail, let fold):
            FerbRichBubble(copy: copy) {
                RichBlocksView(blocks, look: .onBlack, key: key, fold: fold, more: L.s("show_all"), less: L.s("show_less"),
                               tail: tail.map { ($0, false) })
            }
        case .answer(let key, let ui):
            AnswerRow(model: model, key: key, a: ui)
        case .notice(let key, let text, let quotes, let ui):
            NoticeRow(key: key, text: text, quotes: quotes, ui: ui, onReview: { model.openReview() })
        case .results(_, let cards):
            VStack(spacing: 8) { ForEach(Array(cards.enumerated()), id: \.offset) { _, c in ResultCardView(c) } }.padding(.trailing, 28)
        case .cards(_, let cards):
            Card {
                ForEach(Array(cards.enumerated()), id: \.offset) { i, c in
                    if i > 0 { Divide() }
                    if let taskId = c.taskId, let index = c.index {
                        TaskFileRow(model: model, line: c, taskId: taskId, index: index)
                    } else {
                        CardRow(title: c.title, detail: c.detail) {
                            if c.file { FileIcon(name: c.title, width: 26) } else { RowIcon(symbol: "doc") }
                        }
                    }
                }
            }
        case .typing:
            // FERB has his message and is on it: never a silent screen (kb PHN-628)
            HStack(spacing: 10) {
                TypingDots()
                Text(L.s("chat_thinking")).type(14, .medium, ink.low)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .day(_, let day):
            DayDivider(dayLabel(day, today: today))
        // FERB.app's question card in the chat, answered in place (kb PHN-770)
        case .asking(_, let id, let title, let reason, _):
            QuestionBox(model: model, approvalId: id, title: title, reason: reason)
        case .outbox(_, let o):
            OutboxRow(model: model, item: o)
        case .withFiles(_, let m):
            WithFilesRow(model: model, m: m)
        case .earlierLink:
            EarlierLink(L.s("older_available")) { model.openChat() }.accessibilityIdentifier("earlier-link")
        // the memory import's live card (kb MEM-456): every word is the Mac's
        case .importProgress(_, let card):
            MemoryImportCard(title: card.title, rows: card.rows.map {
                MemoryImportRow(key: $0.key, name: $0.name, line: $0.line, detail: $0.detail, done: Int($0.done), total: Int($0.total),
                                count: $0.count, foundLine: $0.foundLine, note: $0.note)
            })
            .accessibilityIdentifier("memory-import")
        case .older(_, let state):
            if case .needsBetterPath = state {
                Text(L.s("older_needs_better_path")).type(14, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
            } else if olderLoading {
                // earlier messages on their way: a small turning mark at the top (kb PHN-628)
                HStack(spacing: 8) {
                    Spinner(size: 14)
                    Text(L.s("chat_older_loading")).type(14, .medium, ink.low)
                }
                .frame(maxWidth: .infinity).padding(.vertical, 8)
            } else {
                Button { model.loadOlder() } label: {
                    Text(L.s("older_available")).type(14, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.horizontal, 6).padding(.vertical, 8)
            }
        }
    }
}

/// A file a finished task made, on its result card (Android's TaskFileRow): a tap fetches it (by the task and its
/// place, never by a path read in the chat), the row says how far it got, and once here a tap opens it.
struct TaskFileRow: View {
    @Bindable var model: AppModel
    let line: CardLine
    let taskId: String
    let index: Int
    @State private var showing: URL?

    var body: some View {
        let t = model.transfers.ofTask(taskId, index: UInt64(index))
        let detail: String? = {
            guard let t else { return line.detail }
            if t.phase == .done { return L.s("tr_card_ready") }
            if let p = t.problem { return p.text }
            if t.phase == .running { return L.s("tr_card_getting", Int(t.fraction * 100)) }
            return L.s("tr_card_paused")
        }()
        CardRow(title: line.title, detail: detail, onClick: {
            guard let t else { model.fetchTaskFile(taskId, index: index, name: line.title); return }
            switch t.phase {
            case .done: showing = URL(fileURLWithPath: t.local); model.handedOut(t.key)
            case .running: break
            default: model.transfers.resume(t.key)
            }
        }) { FileIcon(name: line.title, width: 26) } trailing: { Opens() }
            .ferbFullScreenCover(item: $showing) { url in QuickLookView(url: url) { showing = nil } }
    }
}

extension URL: @retroactive Identifiable { public var id: String { absoluteString } }

/// A message in the outbox (PHN-401): the owner's bubble, faded until it is sent, with what it waits for and Cancel.
struct OutboxRow: View {
    @Bindable var model: AppModel
    let item: OutboxItem
    @Environment(\.skyInk) private var ink

    var body: some View {
        let failed = item.state == "failed"
        let status: String = failed ? L.s("outbox_failed", item.reason ?? "")
            : item.state == "sending" ? L.s("outbox_sending")
            // the Mac asleep (insomnia mode was on, APP-981, or it said so, SENSE-430) says so; else what it waits for
            : L.s(MacAsleep.waitingLine(item.waitingFor, asleep: MacAsleep.waiting(model.ui.status, model.ui.link, lost: model.ui.linkLost)))
        VStack(alignment: .trailing, spacing: 4) {
            OwnerBubble(item.text).opacity(failed ? 1 : 0.7)
            HStack(spacing: 12) {
                Text(status).type(13.5, .medium, failed ? Ink.red : ink.low).frame(maxWidth: 240, alignment: .trailing)
                if item.state != "sending" {
                    Button { model.cancelOutbox(item.id) } label: {
                        Text(L.s(failed ? "outbox_remove" : "outbox_cancel")).type(13.5, .semibold, ink.top)
                    }
                    .frame(minHeight: 44)
                }
            }
            .padding(.horizontal, 6)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

// ---- tasks -----------------------------------------------------------------------------------------------------

struct TasksView: View {
    @Bindable var model: AppModel
    @Environment(\.skyInk) private var ink

    var body: some View {
        let w = model.ui.work
        let asking = L.s("question_default_title")
        let groups = [(L.s("tasks_waiting"), w.waiting), (L.s("tasks_running"), w.running), (L.s("tasks_earlier"), Array(w.earlier.prefix(30)))]
            .filter { !$0.1.isEmpty }
        TasksPage(groups: groups.map { label, items in
            WorkGroup(label: label, rows: items.map {
                WorkRow(kind: $0.kind, title: $0.title.isEmpty ? asking : $0.title, detail: $0.detail,
                        value: $0.progress.map { L.s("progress", $0.done, $0.total) })
            })
        }, waiting: nil, onTab: { model.choose($0) }, onProfile: { model.show(.profile) },
        onRow: { g, r in if g < groups.count, r < groups[g].1.count { open(model, groups[g].1[r]) } },
        empty: AnyView(Text(L.s("tasks_empty")).type(15.5, .medium, ink.low).padding(.horizontal, 8).padding(.top, 22)))
    }
}

// ---- the Mac ---------------------------------------------------------------------------------------------------

struct MacView: View {
    @Bindable var model: AppModel
    let waiting: PhoneTab?

    var body: some View {
        let l = model.ui.link
        let online = l?.phase == .online
        let kind: LinkKind = !online ? .search : (l?.path == .direct ? .direct : .relay)
        // the Mac's own name once it told it (kept by the library), else the lab's "Your Mac"
        let name = model.ui.mac?.name ?? L.s("mac_name")
        let status: String? = {
            if online, l?.path == .ownRelay { return L.s("mac_own_relay") }
            if online { return nil }   // the lab's own words: "Connected directly", "Through the shared relay"
            if l?.phase == .connecting { return L.s("mac_connecting") }
            if let s = l?.retryInS { return L.s("mac_retry_in", Int(s)) }
            return L.s("mac_offline")
        }()
        let note: String? = {
            if model.ui.status?.stopped == true { return L.s("mac_stopped") }
            if case .ownFallback(let p) = l?.relay { return p?.detail ?? L.s("mac_own_fallback") }
            if l?.macOlder == true { return L.s("mac_mac_older") }
            if l?.macNewer == true { return L.s("mac_mac_newer") }
            if !online { return l?.problem?.detail }
            return nil
        }()
        MacPage(link: kind, macName: name, ownRelay: l?.ownRelayUrl.map(hostOf), settingsDetail: L.s("mac_settings_detail").replacingOccurrences(of: "%@", with: Words.bioName),
                status: status, note: note, name: online ? nil : name, waiting: waiting,
                onTab: { model.choose($0) }, onProfile: { model.show(.profile) },
                onLink: { if online, l?.path == .publicRelay { model.show(.relay) } },
                onOwnRelay: { model.show(.ownRelay) }, onSettings: { model.openSettings() },
                onStopAll: { model.show(.stopAll) },
                notice: macNotices)
    }
}

extension MacView {
    /// The cards above the Mac tab's rows: "until I'm back" (PHN-611) and "Keep my Mac working" (APP-980).
    var macNotices: AnyView? {
        let screenUse = model.ui.status?.screenUse?.active == true
        let insomnia = model.ui.status?.insomnia
        if !screenUse && insomnia == nil { return nil }
        return AnyView(VStack(spacing: 0) {
            if screenUse { ScreenUseCard(model: model) }
            if let insomnia { InsomniaCard(model: model, i: insomnia) }
        })
    }
}

/// FERB may use the Mac's screen until the owner is back (PHN-611): the line and "Ask me again".
struct ScreenUseCard: View {
    @Bindable var model: AppModel
    var body: some View {
        Card(padding: 16) {
            VStack(alignment: .leading, spacing: 12) {
                Text(L.s("screen_use_active")).type(15.5, .medium, Ink.black).frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityAddTraits(.updatesFrequently)
                Pill(title: L.s("screen_use_end"), kind: .grey, height: 44,
                     onClick: model.ui.busy.contains("screen-use") ? nil : { model.endScreenUse() })
            }
        }
        .padding(.horizontal, 16).padding(.bottom, 12)
    }
}

/// A relay's address as the row shows it: its host.
func hostOf(_ url: String) -> String {
    var s = url
    if let r = s.range(of: "://") { s = String(s[r.upperBound...]) }
    return String(s.prefix { $0 != "/" && $0 != "?" })
}

/// The Mac's side needs the owner's Face ID once (kb PHN-649; Android's MacNeedCard): the lab's white card with what
/// happened in plain words and one button, never a line to find.
struct MacNeedCard: View {
    let need: MacNeed
    let busy: Bool
    let onUnlock: () -> Void

    var body: some View {
        Card(padding: 16) {
            VStack(alignment: .leading, spacing: 10) {
                Text(L.s("mac_needs_title")).type(17, .semibold)
                Text(Self.words(need))
                    .type(15, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                Pill(title: L.s("mac_needs_unlock"), kind: .black, symbol: Words.bioSymbol, onClick: busy ? nil : onUnlock)
            }
        }
        .padding(.horizontal, 16).padding(.top, 12)
    }

    static func words(_ need: MacNeed) -> String {
        switch need {
        case .lapsed: L.s("mac_needs_lapsed")
        case .cancelled: L.s("mac_needs_cancelled")
        case .old: L.s("mac_needs_old")
        }
    }
}

/// A message he sent with files (kb PHN-691; Android's WithFilesRow): its files and words as his message, how it stands,
/// and what he can do: stop it while its files go; try again, send the words alone (when files can't go on this path)
/// or remove it.
struct WithFilesRow: View {
    @Bindable var model: AppModel
    let m: Sending

    var body: some View {
        let bad = m.phase == .failed || m.phase == .closed
        let status: String = {
            switch m.phase {
            case .uploading: return L.s("send_progress", Int(m.fraction * 100))
            case .sending: return L.s("outbox_sending")
            case .waitingLink:
                return L.s(MacAsleep.waitingLine(nil, asleep: MacAsleep.waiting(model.ui.status, model.ui.link, lost: model.ui.linkLost)))
            case .waitingPath: return m.note.map(\.text) ?? L.s("send_waiting_path")
            case .stopped: return L.s("send_stopped")
            case .locked: return L.s("send_locked")
            case .closed: return L.s("send_closed")
            case .failed: return L.s("outbox_failed", m.note.map(\.text) ?? "")
            }
        }()
        var actions: [(String, () -> Void)] = []
        switch m.phase {
        case .uploading: actions = [(L.s("outbox_cancel"), { model.stopSending(m.id) })]
        case .sending: break
        case .waitingPath:
            if !m.text.trimmingCharacters(in: .whitespaces).isEmpty { actions.append((L.s("send_text_only"), { model.sendTextOnly(m.id) })) }
            actions.append((L.s("outbox_remove"), { model.removeSending(m.id) }))
        case .stopped, .closed:
            actions = [(L.s("send_try_again"), { model.retrySending(m.id) }), (L.s("outbox_remove"), { model.removeSending(m.id) })]
        case .failed:
            if m.note?.key != "send_files_gone" { actions.append((L.s("send_try_again"), { model.retrySending(m.id) })) }
            actions.append((L.s("outbox_remove"), { model.removeSending(m.id) }))
        case .waitingLink, .locked: actions = [(L.s("outbox_remove"), { model.removeSending(m.id) })]
        }
        return FilesMessage(files: m.chips.map { (name: $0.name, detail: ByteCountFormatter.string(fromByteCount: $0.size, countStyle: .file),
                                                    thumbnail: Thumbnail.of($0.path, $0.name)) },
                            text: m.text, status: status, bad: bad, actions: actions)
            .accessibilityIdentifier("sending")
    }
}

/// A small picture of a photo in the phone's own copy, made by ImageIO at thumbnail size; nil for other files or when
/// the copy can't be read (kept per path, so it is made once).
@MainActor
enum Thumbnail {
    private static var made: [String: Image] = [:]
    private static let kinds: Set<String> = ["jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "bmp", "tiff"]

    static func of(_ path: String, _ name: String) -> Image? {
        guard kinds.contains((name as NSString).pathExtension.lowercased()) else { return nil }
        if let i = made[path] { return i }
        guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
              let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, [kCGImageSourceCreateThumbnailFromImageAlways: true,
                                                                    kCGImageSourceThumbnailMaxPixelSize: 108,
                                                                    kCGImageSourceCreateThumbnailWithTransform: true] as CFDictionary)
        else { return nil }
        let i = Image(decorative: cg, scale: 3)
        made[path] = i
        return i
    }
}


/// FERB's alerts as a toast (kb APP-1105, APP-1109; FERB.app ChatBody.swift AlertToastView; Android ui/Main.kt AlertToast):
/// the newest one rises in with its words, and on their first line "N alerts" (when more wait) and ×. It leaves by itself
/// after six seconds in front of him (the model puts it away on every device) unless the stack is open or his finger is
/// on it. "N alerts" opens the stack: every waiting alert, older ones above, each with its own ×; tapping it again closes.
struct AlertToastView: View {
    @Bindable var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var reduce

    var body: some View {
        let alerts = model.ui.alerts
        let open = model.ui.alertsOpen && alerts.count > 1
        ZStack {
            if let newest = alerts.last {
                VStack(spacing: 10) {
                    if open { ForEach(alerts.dropLast(), id: \.id) { row($0, count: 0, open: true) } }
                    row(newest, count: alerts.count > 1 ? alerts.count : 0, open: open)
                }
                .padding(.horizontal, 12).padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Ink.darkFill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .shadow(color: .black.opacity(0.35), radius: 10, y: 4)
                .simultaneousGesture(DragGesture(minimumDistance: 0)
                    .onChanged { _ in model.holdAlert(true) }.onEnded { _ in model.holdAlert(false) })
                .transition(reduce ? .opacity : .move(edge: .bottom).combined(with: .opacity))
                .accessibilityIdentifier("alert-toast")
            }
        }
        .animation(reduce ? .easeInOut(duration: 0.2) : .spring(response: 0.4, dampingFraction: 0.8), value: alerts.map(\.id))
        .animation(reduce ? .easeInOut(duration: 0.2) : .spring(response: 0.35, dampingFraction: 1), value: open)
    }

    /// One alert: the mark, its words, and on their first line the count (on the newest, with more waiting) and ×.
    private func row(_ alert: StatusAlert, count: Int, open: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.circle").font(.system(size: 13)).foregroundStyle(Ink.amber)
            Text(alert.text).type(13.5, .regular, .white).lineLimit(open ? 4 : 2).frame(maxWidth: .infinity, alignment: .leading)
            if count > 1 {
                Button { model.toggleAlerts() } label: {
                    HStack(spacing: 3) {
                        Text(L.s("alerts_count", count)).type(12, .medium, Ink.onDark)
                        Image(systemName: open ? "chevron.down" : "chevron.up").font(.system(size: 9, weight: .semibold)).foregroundStyle(Ink.onDark)
                    }
                    .padding(.horizontal, 8).padding(.vertical, 2)
                    .background(Tone.fill2, in: Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(open ? L.s("alerts_fewer") : L.s("alerts_all", count))
                .accessibilityIdentifier("alert-count")
            }
            Button { model.dismissAlert(alert.id) } label: {
                Image(systemName: "xmark").font(.system(size: 12, weight: .medium)).foregroundStyle(Ink.onDark).frame(width: 22, height: 22)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(L.s("alert_dismiss"))
        }
        .accessibilityIdentifier("alert-row")
    }
}

/// The work row above the message box (kb PHN-725; FERB.app ChatBody.swift WorkRowView): "1 active task · 0:42", the
/// round live pictures of the first two tasks, "+N", "View tasks". A picture opens the task's short view. Its pictures
/// are asked for only while it is on screen (the model adds the rest: unlocked, in front, not on the shared relay).
struct WorkRowFor: View {
    @Bindable var model: AppModel
    let r: RunningUi
    var body: some View {
        // a task whose card rows show their own pictures has no circle here (APP-891: a picture shows once)
        let shown = r.tasks.filter { $0.taskId.map { !model.tasksWithRowPictures.contains($0) } ?? true }
        let ids = Array(shown.compactMap(\.taskId).prefix(2))
        ActiveWorkRow(words: r.words, running: r.running, since: r.since,
                tasks: shown.map { t in WorkTask(id: t.entry, title: t.title, line: t.line, waiting: t.waiting, paused: t.paused,
                                                    picture: t.taskId.flatMap { model.thumbnails.pictures[$0] }) },
                viewTasks: L.s("work_view_tasks"),
                onPicture: { w in model.showTask(r.tasks.first { $0.entry == w.id }?.taskId) },
                onMore: { model.choose(.tasks) }, onViewTasks: { model.choose(.tasks) })
            .padding(.horizontal, 6)
            .onAppear { model.workRowShown(ids) }
            .onChange(of: ids) { _, now in model.workRowShown(now) }
            .onDisappear { model.workRowShown([]) }
            .accessibilityIdentifier("work-row")
    }
}

/// A turn's answer as FERB.app draws it (kb PHN-723, PHN-724; ConversationTranscript.swift AnswerView).
struct AnswerRow: View {
    @Bindable var model: AppModel
    let key: String
    let a: AnswerUi
    @State private var details = false
    @Environment(\.openURL) private var openURL
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(a.above.enumerated()), id: \.offset) { _, c in ResultCardView(c).padding(.trailing, 28) }
            if a.bubble {
                FerbRichBubble(copy: a.copy) {
                    let tail = a.gaps.map { (GapLines.label + " " + $0, true) }
                        + (a.note.map { [("Couldn't confirm: " + $0, true)] } ?? []) + a.lines.map { ($0, false) }
                    RichBlocksView(a.blocks, look: .onBlack, key: key, fold: .answer, more: L.s("show_all"), less: L.s("show_less"), tail: tail)
                    if !a.details.isEmpty {
                        Button { details.toggle() } label: {
                            Text(L.s("answer_details")).font(.system(size: 14.5, weight: .semibold)).foregroundStyle(RichLook.onBlack.link)
                        }.buttonStyle(.plain)
                        if details { RichBlocksView(a.details, look: .onBlack, key: key + ":full") }
                    }
                }
            }
            if !a.below.isEmpty { VStack(spacing: 8) { ForEach(Array(a.below.enumerated()), id: \.offset) { _, c in ResultCardView(c) } }.padding(.trailing, 28) }
            if !a.sites.isEmpty {
                SitesChip(sites: a.sites, onOpen: { h in if let u = Present.siteLink(h) { openURL(u) } }, onAll: { model.show(.sites(row: key)) })
            }
            if !a.files.isEmpty {
                Card {
                    ForEach(Array(a.files.enumerated()), id: \.offset) { i, c in
                        if i > 0 { Divide() }
                        if let taskId = c.taskId, let index = c.index {
                            TaskFileRow(model: model, line: c, taskId: taskId, index: index)
                        } else {
                            CardRow(title: c.title, detail: c.detail) { FileIcon(name: c.title, width: 26) }
                        }
                    }
                }
                .padding(.trailing, 28)
            }
        }
    }
}

/// FERB's own message (FERB.app's NoticeView): a reminder's arrival is its one box; else its words in FERB's bubble with
/// the facts FERB remembered, the quotes with where they came from, and the page buttons.
struct NoticeRow: View {
    let key: String
    let text: String
    let quotes: [Quote]
    let ui: NoticeUi
    /// MEM-485: the day's note's Review (the list in a sheet); FERB.app's other pages the phone doesn't have (PHN-656).
    var onReview: (() -> Void)? = nil
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(ui.above.enumerated()), id: \.offset) { _, c in ResultCardView(c).padding(.trailing, 28) }
            if !ui.boxSays {
                FerbRichBubble(copy: text) {
                    RichWords(richText(FerbDesign.RichBlock.plain(text), .onBlack), look: .onBlack, key: key)
                    ForEach(Array(ui.notes.enumerated()), id: \.offset) { _, c in ResultCardView(c) }
                    if let label = ui.review {
                        Pill(title: label, height: 40, fill: false, onClick: onReview).padding(.top, 4)
                            .accessibilityIdentifier("review:open")
                    }
                    ForEach(Array(quotes.enumerated()), id: \.offset) { _, q in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(q.text).type(14.5, .regular, .white).lineSpacing(2)
                            if !q.origin.isEmpty { Text(q.origin).type(13, .regular, Ink.onDark) }
                        }
                        .padding(.leading, 10)
                        .overlay(alignment: .leading) { Capsule().fill(Ink.onDark).frame(width: 2) }
                    }
                }
            }
        }
    }
}
