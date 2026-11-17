import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// The four tabs (Mobile.swift TopBar; Android's Main.kt): chat (the home screen until a chat starts), tasks, files,
/// the Mac; a short note over the page.
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
            TabBarHost(chosen: model.tab, waiting: barWaiting(waiting), onTab: { model.choose($0) }, onProfile: { model.show(.profile) }) {
                MotionSwitch(TabKey(tab: model.tab, home: model.home), kind: .tab,
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
private struct TabKey: Hashable {
    let tab: PhoneTab
    let home: Bool
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
                Button(L.s("files_send_folder")) { p.folder = true }
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

/// The owner's draft and send; + picks files that go to the Mac and ride with the next message (PHN-127), shown above
/// it while they travel and once they arrived; the mic isn't on the phone yet.
struct Composer: View {
    @Bindable var model: AppModel
    let placeholder: String
    let picking: Picking
    @Environment(\.skyInk) private var ink

    var body: some View {
        let t = model.transfers
        VStack(spacing: 8) {
            let moving = t.list.filter(\.forChat)
            if !moving.isEmpty { TransferGroupCard(model: model, group: moving, batch: nil) }
            if !t.ready.isEmpty {
                HStack(spacing: 10) {
                    ForEach(t.ready.prefix(4), id: \.uploadId) { a in
                        Button { t.unattach(a.uploadId) } label: {
                            HStack(spacing: 4) {
                                FileIcon(name: a.name, width: 18)
                                Text(a.name).type(13.5, .medium, ink.low).lineLimit(1).frame(maxWidth: 120, alignment: .leading)
                                Image(systemName: "xmark").font(.system(size: 11, weight: .semibold)).foregroundStyle(ink.low)
                            }
                        }
                        .accessibilityLabel(L.s("attach_remove", a.name))
                    }
                    if t.ready.count > 4 { Text("+\(t.ready.count - 4)").type(13.5, .medium, ink.low) }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 6)
                // where they already are on the Mac (PHN-608): the message only points FERB at them
                Text(L.s("attach_place", t.readyPlace ?? model.ui.mac?.uploadFolder?.shown ?? L.s("tr_place_default"))).type(13, .medium, ink.low)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
            }
            // PHN-643: what the microphone is doing, in words above the box
            if let micText {
                Text(micText).type(13.5, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
                    .accessibilityAddTraits(.updatesFrequently)
            }
            // hold to speak (PHN-603): the words land in the box while held and stay there; never sent by themselves
            PhoneComposer(placeholder: placeholder, text: Binding(get: { model.draft }, set: { model.typed($0) }),
                          onSend: { model.send() }, onAttach: { picking.ask(forChat: true) },
                          onMicDown: {
                              if model.voicePress() == .askMicrophone {
                                  Task { model.microphoneAnswered(await AppleSpeech.ask()) }
                              }
                          },
                          onMicUp: { model.voiceRelease() }, listening: model.listening,
                          listeningText: micText ?? L.s("voice_listening"), micLabel: L.s("voice_speak"),
                          micState: model.mic, micLevel: model.micLevel)
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
                 composer: AnyView(Composer(model: model, placeholder: texts.placeholder, picking: picking)))
    }
}

/// What happened while the owner was away: questions first, then failures, then what finished (HomePage).
func awayRows(_ ui: UiState) -> [WorkItem] {
    ui.work.waiting + ui.away.filter { $0.kind == .failed } + ui.away.filter { $0.kind != .failed }
}

@MainActor func open(_ model: AppModel, _ item: WorkItem) {
    if let id = item.approvalId { model.show(.asking(approvalId: id)) } else { model.openChat() }
}

// ---- chat ------------------------------------------------------------------------------------------------------

struct ChatView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let waiting: PhoneTab?
    let picking: Picking
    @Environment(\.skyInk) private var ink

    var body: some View {
        VStack(spacing: 0) {
            TopBar(chosen: .chat, waiting: waiting, onTab: { model.choose($0) }, onProfile: { model.show(.profile) })
            if let (text, tap) = statusLine {
                Button { if tap { model.unlockMac() } } label: {
                    Text(text).type(14, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading)
                }
                .disabled(!tap)
                .padding(.horizontal, 24).padding(.top, 10)
            }
            ScrollViewReader { reader in
                ScrollView {
                    LazyVStack(spacing: 12) {
                        ForEach(model.ui.rows) { row in
                            ChatRowView(model: model, row: row).id(row.key)
                                .onAppear { if case .older(_, .available) = row { model.loadOlder() } }
                                .transition(.motionItem)  // a row that arrives rises in (kb PHN-624)
                        }
                    }
                    .animation(FerbMotion.item.animation, value: model.ui.rows.map(\.key))
                    .padding(.horizontal, 16).padding(.top, 16).padding(.bottom, 12)
                }
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: model.ui.rows.count, initial: true) { _, _ in
                    if let last = model.ui.rows.last { withAnimation { reader.scrollTo(last.key, anchor: .bottom) } }
                }
            }
            Composer(model: model, placeholder: L.s("chat_placeholder"), picking: picking)
                .padding(.horizontal, 16).padding(.bottom, insets.bottom + 8)
        }
    }

    /// The line under the tabs: the Mac can't be reached, its unlock is on its way or slow (PHN-641), or it is needed
    /// after the owner cancelled its prompt (a tap gives it).
    private var statusLine: (String, Bool)? {
        if model.ui.link?.phase != .online { return (L.s("chat_offline"), false) }
        // PHN-641: the Mac's side after the app opened: words only, it clears by itself
        if model.ui.macLine == .opening { return (L.s("mac_opening"), false) }
        if model.ui.macLine == .slow { return (L.s("mac_slow"), false) }
        if model.ui.needsMacUnlock { return (L.s("chat_unlock_mac"), true) }
        // FERB may use the Mac's screen until the owner is back (PHN-611; "Ask me again" is on the Mac tab)
        if model.ui.status?.screenUse?.active == true { return (L.s("screen_use_active"), false) }
        return nil
    }
}

struct ChatRowView: View {
    @Bindable var model: AppModel
    let row: ChatRow
    @Environment(\.skyInk) private var ink

    var body: some View {
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
        case .line(_, let text, let quotes):
            VStack(alignment: .leading, spacing: 4) {
                Text(text).type(14.5, .medium, ink.low)
                ForEach(quotes, id: \.self) { Text($0).type(14, .regular, ink.low) }
            }
            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
        case .task(let t):
            TaskCard(headline: t.headline, progress: L.s("progress", t.done, t.total), parts: t.parts,
                     watchTitle: L.s("live_title"), stopTitle: L.s("live_stop"),
                     onWatch: { model.watchLive(t.taskId) }, onStop: { model.stop(t.taskId) })
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
            TypingDots().frame(maxWidth: .infinity, alignment: .leading)
        case .asking(_, let id, let title):
            Card {
                CardRow(title: title ?? L.s("question_default_title"), detail: L.s("question_tap"),
                        onClick: { model.show(.asking(approvalId: id)) }) { Mark(kind: .asking) } trailing: { Opens() }
            }
        case .outbox(_, let o):
            OutboxRow(model: model, item: o)
        case .older(_, let state):
            if case .needsBetterPath = state {
                Text(L.s("older_needs_better_path")).type(14, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
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
            .fullScreenCover(item: $showing) { url in QuickLookView(url: url) { showing = nil } }
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
            : item.waitingFor == "unlock" ? L.s("outbox_waiting_unlock") : L.s("outbox_waiting")
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
                notice: model.ui.status?.screenUse?.active == true ? AnyView(ScreenUseCard(model: model)) : nil)
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
