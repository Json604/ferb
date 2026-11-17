import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI

/// MobileScreens.swift PhoneFilesPage (Android's Files.kt): search, the transfers under way (the lab's progress card),
/// the places and recent files, and sending files to the Mac; a place opens as a folder of the lab's rows. A key
/// place's entry shows its lock and asks for Face ID for that very path (PHN-121). A fetched file opens only on the
/// owner's tap: QuickLook, or the share sheet (PHN-517).
struct FilesView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let waiting: PhoneTab?
    let picking: Picking
    @State private var query = ""
    @State private var preview: URL?
    @State private var share: ShareItem?
    @Environment(\.skyInk) private var ink

    var body: some View {
        let files = model.ui.files
        VStack(spacing: 0) {
            TopBar(chosen: .files, waiting: waiting, onTab: { model.choose($0) }, onProfile: { model.show(.profile) })
            ScrollView {
                VStack(spacing: 0) {
                    searchField(files)
                    ForEach(groups(model.transfers.list), id: \.first!.key) { g in
                        TransferGroupCard(model: model, group: g, batch: g.first?.batch.flatMap { model.transfers.batches[$0] },
                                          open: { t in preview = URL(fileURLWithPath: t.local); model.handedOut(t.key) },
                                          share: { urls, key in share = ShareItem(urls: urls); if let key { model.handedOut(key) } })
                            .padding(.top, 12)
                    }
                    if model.ui.link?.phase == .online, model.ui.link?.bulkAllowed == false { line(L.s("files_needs_better_path")) }
                    if let p = files.problem { line(p.text) }
                    if files.search != nil { found(files) } else if files.folder == nil { places(files) } else { folder(files) }
                }
                .padding(.horizontal, 16).padding(.bottom, 12)
            }
            HStack(spacing: 8) {
                Pill(title: "Send files to your Mac", kind: .black, symbol: "arrow.up.doc") { picking.ask(forChat: false) }
            }
            .padding(.horizontal, 16).padding(.top, 8).padding(.bottom, insets.bottom + 8)
        }
        .onAppear { model.filesOpened() }
        .fullScreenCover(item: $preview) { url in QuickLookView(url: url) { preview = nil } }
        .sheet(item: $share) { s in ShareSheet(urls: s.urls) { share = nil } }
    }

    private func line(_ text: String) -> some View {
        Text(text).type(14, .medium, ink.low).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 8).padding(.top, 12)
    }

    /// The lab's black search pill, now a field: the Mac's names, inside the folder open (or the whole home folder).
    private func searchField(_ files: FilesUi) -> some View {
        let hint = files.folder.map { L.s("search_in", nameOf($0.path)) } ?? "Search your Mac"
        return HStack(spacing: 10) {
            Image(systemName: "magnifyingglass").font(.system(size: 17, weight: .medium)).foregroundStyle(Ink.onDark)
            TextField("", text: $query, prompt: Text(hint).foregroundColor(Ink.onDark))
                .font(.system(size: 16.5)).foregroundStyle(Ink.onDark).tint(Ink.onDark)
                .submitLabel(.search).autocorrectionDisabled().textInputAutocapitalization(.never)
                .onSubmit { model.search(query) }
                .accessibilityLabel(hint)
            if files.search != nil || !query.isEmpty {
                Button { query = ""; model.closeSearch() } label: {
                    Image(systemName: "xmark").font(.system(size: 13, weight: .semibold)).foregroundStyle(Ink.onDark)
                }
                .accessibilityLabel(L.s("search_close"))
            }
        }
        .padding(.horizontal, 16).frame(minHeight: 50)
        .background(Ink.black, in: Capsule())
        .padding(.top, 14)
    }

    @ViewBuilder private func found(_ files: FilesUi) -> some View {
        if let s = files.search {
            SkyLabel(text: s.path.map { L.s("search_found_in", nameOf($0)) } ?? L.s("search_found_mac"))
            if s.loading { line(L.s("search_looking")) }
            else if let p = s.problem { line(p.text) }
            else if let f = s.found {
                if f.files.isEmpty { line(L.s("search_none", s.query)) } else {
                    Card {
                        ForEach(Array(f.files.enumerated()), id: \.offset) { i, e in
                            if i > 0 { Divide() }
                            CardRow(title: e.name, detail: foundDetail(e), onClick: { model.openFound(e) }) {
                                if e.dir { RowIcon(symbol: "folder") } else { FileIcon(name: e.name, width: 26) }
                            } trailing: { Opens() }
                        }
                    }
                }
                // more pages (PHN-609): "Load more" while the Mac says there are; the plain line only when it doesn't page
                if f.next != nil { loadMore { model.searchMore() } } else if f.cut { line(L.s("search_cut", f.files.count)) }
                if f.incomplete { line(L.s("search_incomplete", Int(f.looked))) }
                if f.skipped > 0 { line(L.plural("search_skipped", Int(f.skipped))) }
            }
        }
    }

    /// "Load more" (PHN-609): the next page of 500 from the Mac, added in place under what is shown.
    private func loadMore(_ action: @escaping () -> Void) -> some View {
        let busy = model.ui.busy.contains("more")
        return Card {
            CardRow(title: L.s(busy ? "files_loading_more" : "files_load_more"), detail: nil, onClick: busy ? nil : action) {
                RowIcon(symbol: "arrow.down.circle")
            }
        }
        .padding(.top, 12)
    }

    @ViewBuilder private func places(_ files: FilesUi) -> some View {
        let places: [(String, String, String)] = [(AppModel.places[0], "place_desktop", "menubar.dock.rectangle"),
                                                  (AppModel.places[1], "place_documents", "doc"),
                                                  (AppModel.places[2], "place_downloads", "arrow.down.circle")]
        SkyLabel(text: L.s("files_places"))
        Card {
            ForEach(Array(places.enumerated()), id: \.offset) { i, p in
                if i > 0 { Divide() }
                CardRow(title: L.s(p.1), detail: files.loading == p.0 ? L.s("files_opening") : nil, onClick: { model.openFolder(p.0) }) {
                    RowIcon(symbol: p.2)
                } trailing: { Opens(value: files.counts[p.0].map { String($0) }) }
            }
        }
        SkyLabel(text: L.s("files_recent"))
        if let recent = files.recent {
            if recent.files.isEmpty { line(L.s("recent_empty")) } else {
                Card {
                    ForEach(Array(recent.files.enumerated()), id: \.offset) { i, e in
                        if i > 0 { Divide() }
                        CardRow(title: e.name, detail: recentDetail(e), onClick: { model.openFound(e) }) {
                            if e.dir { RowIcon(symbol: "folder") } else { FileIcon(name: e.name, width: 26) }
                        } trailing: { Opens() }
                    }
                }
            }
            if recent.skipped > 0 { line(L.plural("recent_skipped", Int(recent.skipped))) }
        } else if let n = files.recentNote {
            line(n.text)
        }
    }

    @ViewBuilder private func folder(_ files: FilesUi) -> some View {
        if let f = files.folder {
            HStack {
                Button { model.folderUp() } label: {
                    Image(systemName: "chevron.left").font(.system(size: 18, weight: .semibold)).foregroundStyle(ink.top)
                }
                .accessibilityLabel(L.s("files_back"))
                Text(nameOf(f.path)).type(15, .semibold, ink.low).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                if f.locked { Image(systemName: "lock.fill").font(.system(size: 14)).foregroundStyle(ink.low) }
            }
            .padding(.top, 14).padding(.vertical, 8)
            if f.entries.isEmpty { line(L.s("files_empty")) } else {
                Card {
                    ForEach(Array(f.entries.enumerated()), id: \.offset) { i, e in
                        if i > 0 { Divide() }
                        CardRow(title: e.name, detail: entryDetail(e), onClick: { model.openEntry(e) }) {
                            if e.dir { RowIcon(symbol: "folder") } else { FileIcon(name: e.name, width: 26) }
                        } trailing: {
                            if e.locked { Image(systemName: "lock.fill").font(.system(size: 15)).foregroundStyle(Ink.faint) } else { Opens() }
                        }
                    }
                }
                .padding(.top, 4)
                if f.cut { line(L.s("files_cut", f.entries.count, Int(f.total))) }
                if f.next != nil { loadMore { model.folderMore() } }
                // a key place's folder isn't fetched whole: each of its files asks for Face ID by itself (PHN-121)
                if !f.locked, f.entries.contains(where: { !$0.locked }) {
                    let busy = model.ui.busy.contains("tree")
                    Card {
                        CardRow(title: L.s("files_get_folder"), detail: busy ? L.s("files_opening") : nil, onClick: busy ? nil : { model.fetchFolder() }) {
                            RowIcon(symbol: "arrow.down.circle")
                        }
                    }
                    .padding(.top, 12)
                }
            }
        }
    }

    private func entryDetail(_ e: FileEntry) -> String? {
        if e.locked { return L.bio("files_key_place") }
        if e.dir { return nil }
        let size = ByteCountFormatter.string(fromByteCount: Int64(e.size), countStyle: .file)
        guard e.modified > 0 else { return size }
        return L.s("file_detail", size, Date(timeIntervalSince1970: TimeInterval(e.modified)).formatted(date: .abbreviated, time: .omitted))
    }

    private func foundDetail(_ e: FoundFile) -> String? {
        let parent = (e.path as NSString).deletingLastPathComponent
        let where_ = parent.isEmpty ? nil : (parent as NSString).lastPathComponent
        if e.dir { return where_ }
        let size = ByteCountFormatter.string(fromByteCount: Int64(e.size), countStyle: .file)
        return where_.map { L.s("file_detail", $0, size) } ?? size
    }

    private func recentDetail(_ e: FoundFile) -> String? {
        let date = e.at.flatMap { ISO8601DateFormatter().date(from: $0) }
            ?? (e.modified > 0 ? Date(timeIntervalSince1970: TimeInterval(e.modified)) : nil)
        guard let date else { return nil }
        let day = date.formatted(date: .abbreviated, time: .omitted)
        switch e.source {
        case "made": return e.taskName.flatMap { $0.isEmpty ? nil : L.s("recent_made_for", $0, day) } ?? L.s("recent_made", day)
        case "fetched": return L.s("recent_fetched", day)
        case "sent": return L.s("recent_sent", day)
        default: return day
        }
    }
}

struct ShareItem: Identifiable {
    let id = UUID()
    let urls: [URL]
}

/// A Mac path's last part (`~` is the Mac's home folder).
func nameOf(_ path: String) -> String {
    if path == "~" || path.isEmpty { return L.s("search_mac") }
    let last = (path.hasSuffix("/") ? String(path.dropLast()) : path).split(separator: "/").last.map(String.init)
    return last ?? path
}

/// A fetch is its own card; the files sent together share one (the lab's "Sending 2 photos to your Mac").
func groups(_ all: [FileTransfer]) -> [[FileTransfer]] {
    var out: [[FileTransfer]] = []
    var seen: Set<String> = []
    for t in all {
        if let b = t.batch {
            if seen.insert(b).inserted { out.append(all.filter { $0.batch == b }) }
        } else {
            out.append([t])
        }
    }
    return out
}

/// The lab's progress card with Pause or Go on and Cancel; a fetched file gets Open and Share, on the owner's tap only.
/// A whole folder says how many of its files and bytes are done and what was left out; its files are shared together.
struct TransferGroupCard: View {
    @Bindable var model: AppModel
    let group: [FileTransfer]
    let batch: TransferBatch?
    var open: (FileTransfer) -> Void = { _ in }
    var share: ([URL], String?) -> Void = { _, _ in }

    var body: some View {
        let first = group[0]
        let folder = batch?.folder
        let done = group.allSatisfy { $0.phase == .done }
        let total = max(group.reduce(0) { $0 + $1.total }, 1)
        let fraction = min(max(Double(group.reduce(0) { $0 + $1.done }) / Double(total), 0), 1)
        let photos = group.allSatisfy { FileKind(name: $0.name) == .image }
        let title: String = {
            if let folder, first.way == .fetch { return done ? L.s("tr_folder_here", folder) : L.s("tr_getting", folder) }
            if let folder { return done ? L.s("tr_sent_one", folder) : L.s("tr_sending_one", folder) }
            if first.way == .fetch { return done ? first.name : L.s("tr_getting", first.name) }
            if done { return group.count == 1 ? L.s("tr_sent_one", first.name) : L.s("tr_sent_files", group.count) }
            if group.count == 1 { return L.s("tr_sending_one", first.name) }
            return photos ? L.s("tr_sending_photos", group.count) : L.s("tr_sending_files", group.count)
        }()
        let paused = group.contains { [.paused, .waitingPath, .failed].contains($0.phase) } && !group.contains { $0.phase == .running }
        let notes: [String] = {
            var lines: [String] = []
            if let folder, let batch {
                _ = folder
                let bytes = { (n: Int64) in ByteCountFormatter.string(fromByteCount: n, countStyle: .file) }
                lines.append(L.s("tr_folder_progress", group.filter { $0.phase == .done }.count, group.count,
                                 bytes(group.reduce(0) { $0 + $1.done }), bytes(group.reduce(0) { $0 + $1.total })))
                if batch.cut { lines.append(L.s("files_tree_cut", group.count)) }
                if batch.skipped > 0 { lines.append(L.plural("files_tree_skipped", Int(batch.skipped))) }
                if batch.unreadable > 0 { lines.append(L.plural("tr_unreadable", batch.unreadable)) }
            }
            if let p = group.compactMap(\.problem).first { lines.append(p.text) }
            else if first.way == .fetch, done { lines.append(L.s(first.keyPlace && folder == nil ? "tr_key_place" : "tr_on_phone")) }
            else if first.way == .send, done {
                // where it is on the Mac (PHN-608): the Mac's words when the library carries them, else the default place
                lines.append(L.s("tr_place", group.compactMap(\.macPlace).first ?? model.ui.mac?.uploadFolder?.shown ?? L.s("tr_place_default")))
            }
            return lines
        }()
        TransferCard(title: title, fraction: done ? 1 : fraction, value: done ? "" : (paused ? L.s("tr_paused") : nil),
                     note: notes.isEmpty ? nil : notes.joined(separator: "\n")) {
            HStack(spacing: 8) {
                if done && first.way == .fetch && folder != nil {
                    Pill(title: L.s("tr_share"), kind: .black, height: 42) { share(group.map { URL(fileURLWithPath: $0.local) }, nil) }
                    Pill(title: L.s("tr_done"), kind: .grey, height: 42, fill: false) { model.transfers.remove(first.key) }
                } else if done && first.way == .fetch {
                    Pill(title: L.s("tr_open"), kind: .black, height: 42) { open(first) }
                    Pill(title: L.s("tr_share"), kind: .grey, height: 42) { share([URL(fileURLWithPath: first.local)], first.key) }
                    Pill(title: L.s("tr_done"), kind: .grey, height: 42, fill: false) { model.transfers.remove(first.key) }
                } else if done {
                    Pill(title: L.s("tr_add_message"), kind: .black, height: 42) { if let b = first.batch { model.attachToChat(b) } }
                    Pill(title: L.s("tr_done"), kind: .grey, height: 42, fill: false) { model.transfers.remove(first.key) }
                } else {
                    if paused { Pill(title: L.s("tr_resume"), kind: .black, height: 42) { model.transfers.resume(first.key) } }
                    else { Pill(title: L.s("tr_pause"), kind: .grey, height: 42) { model.pause(first.key) } }
                    Pill(title: L.s("tr_cancel"), kind: .grey, height: 42, fill: false) { model.transfers.remove(first.key) }
                }
            }
            .padding(.top, 12)
        }
    }
}
