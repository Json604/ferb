import AppKit
import SwiftUI
import NotchCore

/// One row of the chat's transcript list (APP-815): the space under the top fade, the loader while an earlier page comes,
/// an entry, an open question, the end.
enum TranscriptRow: Equatable {
    case top
    case loader
    case entry(TranscriptEntry, hidden: Set<String>)
    case question(DaemonApproval)
    case bottom

    var key: String {
        switch self {
        case .top: return "transcript-top"
        case .loader: return "transcript-earlier"
        case let .entry(entry, _): return TranscriptTarget.entry(entry.id)
        case let .question(question): return TranscriptTarget.question(question.id)
        case .bottom: return "transcript-bottom"
        }
    }

    /// Drawn by SwiftUI in its row; the top and the end are only space.
    var hosted: Bool {
        switch self {
        case .top, .bottom: return false
        default: return true
        }
    }
}

/// The chat's transcript as an AppKit list (APP-815): an NSTableView whose rows host the transcript's SwiftUI row views.
/// Every row's height is known before a frame is drawn, so the view can be put anywhere at once (the scroll thumb dragged
/// to the top or the bottom) and shows what is there in that frame:
/// - a row's height is measured once at the list's width and kept by its entry id (a row's content reports any change of
///   its own height, a Show all, a row filling);
/// - a row not measured yet stands at an estimate; before the view moves anywhere, the rows it will show are measured
///   first, and the view is kept on what it showed (the first measured row in view, the top, or the end);
/// - rows near the view are measured ahead while the owner isn't scrolling;
/// - entries inserted above (an earlier page) or added below never move what is in view; at the end, it stays at the end.
@MainActor
final class TranscriptList: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    static let spacing: CGFloat = 10
    static let topSpace: CGFloat = 36
    static let endSpace: CGFloat = 3

    let scroll = NSScrollView()
    let table = ListTable()
    private let clip = ListClip()
    let marks: AnchorMarks

    private(set) var rows: [TranscriptRow] = []
    private var index: [String: Int] = [:]
    /// Builds a row's content (the transcript's row views).
    var content: (TranscriptRow) -> AnyView = { _ in AnyView(EmptyView()) }
    /// The offset and the content's height after each change, delivered once per pass after the change.
    var scrolled: (_ offset: CGFloat, _ height: CGFloat) -> Void = { _, _ in }
    /// The viewport height asked for, for estimates before the first layout.
    var viewport: CGFloat = 300

    private var environment = EnvironmentValues()
    private var environmentMark: [AnyHashable] = []
    private var hosts: [String: RowHost] = [:]
    /// Measured row heights (content and the spacing under it) at `width`.
    private var measured: [String: CGFloat] = [:]
    /// Rows whose content changed while not built: their height stands until they are measured again.
    private var stale = Set<String>()
    private(set) var width: CGFloat = 0
    private var tick = 0
    private var adjusting = 0
    private var measuring = 0
    /// Where the view goes once the list has its width (the newest entry, unless asked otherwise).
    private var pending: (() -> Void)? = nil
    private var reportQueued = false
    private var prefetchQueued = false
    private var prefetchWaiting = false
    private var lastScrollAt = Date.distantPast

    /// Heights measured this run by row key, kept across transcripts (a day opened again, the chat moving between the
    /// notch and the window): its row as measured, the width and the height.
    private static var kept: [String: (row: TranscriptRow, width: CGFloat, height: CGFloat)] = [:]

    init(marks: AnchorMarks) {
        self.marks = marks
        super.init()
        clip.list = self
        clip.drawsBackground = false
        clip.postsBoundsChangedNotifications = true
        clip.postsFrameChangedNotifications = true
        scroll.contentView = clip
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.horizontalScrollElasticity = .none
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentInsets = NSEdgeInsetsZero

        table.list = self
        table.headerView = nil
        table.style = .plain
        table.intercellSpacing = .zero
        table.backgroundColor = .clear
        table.usesAlternatingRowBackgroundColors = false
        table.selectionHighlightStyle = .none
        table.gridStyleMask = []
        table.focusRingType = .none
        table.allowsColumnResizing = false
        table.allowsColumnReordering = false
        table.allowsEmptySelection = true
        table.allowsTypeSelect = false
        table.refusesFirstResponder = true
        table.usesAutomaticRowHeights = false
        table.rowSizeStyle = .custom
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("row"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.dataSource = self
        table.delegate = self
        scroll.documentView = table

        NotificationCenter.default.addObserver(self, selector: #selector(boundsChanged), name: NSView.boundsDidChangeNotification,
                                               object: clip)
        NotificationCenter.default.addObserver(self, selector: #selector(frameChanged), name: NSView.frameDidChangeNotification,
                                               object: clip)
    }

    // MARK: - Rows

    /// The transcript's environment, given to every row; rows already built take a changed one.
    func setEnvironment(_ values: EnvironmentValues) {
        environment = values
        let mark: [AnyHashable] = [values.selectableTextGeneration, values.colorScheme == .dark, values.chatInWindowHost,
                                   values.accessibilityReduceMotion]
        guard mark != environmentMark else { return }
        environmentMark = mark
        for host in hosts.values { host.controller.rootView = root(host.row) }
    }

    /// The rows to show. Entries changed in place update their row; rows inserted or removed keep what is in view where
    /// it is (at the end: at the end).
    func update(rows new: [TranscriptRow]) {
        let old = rows
        let oldKeys = old.map(\.key), newKeys = new.map(\.key)
        if oldKeys == newKeys {
            for i in new.indices where new[i] != old[i] { replace(i, with: new[i]) }
            return
        }
        let anchor = currentAnchor()
        let before = Dictionary(old.map { ($0.key, $0) }, uniquingKeysWith: { first, _ in first })
        rows = new
        index = Dictionary(newKeys.enumerated().map { ($1, $0) }, uniquingKeysWith: { first, _ in first })
        for key in Set(oldKeys).subtracting(newKeys) {
            hosts[key].map(drop)
            given[key] = nil
        }
        // Reloaded whole (the rows' views are kept built by key, so this only places them again): every row stands where
        // its height says in this pass, and the table keeps no place of its own across the change
        adjusting += 1
        table.placing += 1
        table.reloadData()
        table.placing -= 1
        adjusting -= 1
        for (i, row) in new.enumerated() where before[row.key].map({ $0 != row }) ?? false { replace(i, with: row) }
        if old.isEmpty || anchor.key.map({ index[$0] == nil }) ?? false {
            restore(Anchor(key: nil, offset: 0, end: true))
        } else {
            restore(anchor)
        }
        measureShown()
        tileNow()
        queueReport()
        queuePrefetch()
    }

    private func replace(_ i: Int, with row: TranscriptRow) {
        rows[i] = row
        let key = row.key
        if let host = hosts[key] {
            host.row = row
            host.controller.rootView = root(row)          // its content reports a new height itself
        } else if measured[key] != nil {
            stale.insert(key)                             // measured again before it is shown
        }
    }

    // MARK: - Heights

    func height(_ i: Int) -> CGFloat {
        let row = rows[i]
        switch row {
        case .top: return Self.topSpace
        case .bottom: return Self.endSpace
        default:
            if let known = measured[row.key] { return known }
            // An estimate, once given to the table, stays what the table has until the row is measured: the table asks
            // again whenever it likes, and a changed answer would move rows with nothing keeping the view still
            if let given = given[row.key] { return given }
            let guess = estimate(row)
            given[row.key] = guess
            return guess
        }
    }

    /// Estimates the table has, by row key (APP-815).
    private var given: [String: CGFloat] = [:]

    func isMeasured(_ i: Int) -> Bool {
        !rows[i].hosted || (measured[rows[i].key] != nil && !stale.contains(rows[i].key))
    }

    /// What rows of each kind measured against their text's estimate, so later estimates of that kind come closer.
    private var learned: [Int: (estimated: CGFloat, real: CGFloat, count: Int)] = [:]

    private static func kind(_ row: TranscriptRow) -> Int {
        guard case let .entry(entry, _) = row else { return 0 }
        switch entry {
        case let .turn(turn, _):
            if turn.card != nil { return 1 }
            if let cards = turn.cards, !cards.isEmpty { return cards.contains { $0.type == "link" } ? 2 : 3 }
            return turn.answer.count > 2000 ? 4 : 5
        case .reply: return 6
        case .notice: return 7
        }
    }

    /// A height for a row not measured yet: its height this run if its content is the same, else from its text as the
    /// chat lays text out at this width, scaled by how rows of its kind measured.
    func estimate(_ row: TranscriptRow) -> CGFloat {
        if let kept = Self.kept[row.key], kept.row == row, abs(kept.width - width) < 0.5 { return kept.height }
        let text = textEstimate(row)
        guard let seen = learned[Self.kind(row)], seen.count >= 2, seen.estimated > 0 else { return text }
        return max(20, (text * seen.real / seen.estimated).rounded())
    }

    private var textEstimates: [String: (row: TranscriptRow, width: CGFloat, height: CGFloat)] = [:]

    private func textEstimate(_ row: TranscriptRow) -> CGFloat {
        if let known = textEstimates[row.key], abs(known.width - width) < 0.5, known.row == row { return known.height }
        let height = textHeight(row)
        textEstimates[row.key] = (row, width, height)
        return height
    }

    private func textHeight(_ row: TranscriptRow) -> CGFloat {
        let w = max(120, (width > 0 ? width : 400) - 50)
        func lines(_ text: String, _ cap: Int = .max) -> CGFloat {
            var total = 0
            for paragraph in text.split(separator: "\n", omittingEmptySubsequences: false) {
                total += max(1, Int(ceil(CGFloat(paragraph.utf8.count) * 6.3 / w)))
                if total >= cap { return CGFloat(cap) }
            }
            return CGFloat(total)
        }
        let fold = CGFloat(Int(max(260, viewport) * 0.5 / 15))
        func answer(_ turn: ConversationTurn) -> CGFloat {
            var h: CGFloat = 0
            if let card = turn.card {
                h += 44 + CGFloat(card.rows.count) * 22
                h += card.rows.reduce(0) { $0 + min(lines($1.text ?? "", 8), 8) * 15 }
            } else if !turn.answer.isEmpty {
                h += 16 + min(lines(turn.answer), fold + 2) * 15.5 + (lines(turn.answer) > fold ? 18 : 0)
            }
            h += CGFloat((turn.cards ?? []).count) * 34 + CGFloat(turn.artifacts.count) * 34
            return h
        }
        switch row {
        case .top: return Self.topSpace
        case .bottom: return Self.endSpace
        case .loader: return 24 + Self.spacing
        case .question: return 150 + Self.spacing
        case let .entry(entry, _):
            switch entry {
            case let .turn(turn, _):
                let request = turn.taskGoal != nil ? 16 : 16 + lines(turn.request) * 15
                return request + 7 + answer(turn) + Self.spacing
            case let .reply(_, turn):
                return 20 + answer(turn) + Self.spacing
            case let .notice(notice):
                return 16 + lines(notice.text) * 15 + CGFloat(notice.quotes.count) * 34
                    + CGFloat((notice.cards ?? []).count) * 60 + Self.spacing
            }
        }
    }

    /// Measures row `i` at the list's width; true when its height changed.
    @discardableResult
    private func measure(_ i: Int) -> Bool {
        let row = rows[i]
        guard row.hosted, width > 0 else { return false }
        measuring += 1
        defer { measuring -= 1 }
        let host = host(for: i)
        let size = host.controller.sizeThatFits(in: CGSize(width: width, height: CGFloat.greatestFiniteMagnitude))
        return record(row, ceil(size.height))
    }

    private func record(_ row: TranscriptRow, _ height: CGFloat) -> Bool {
        let h = max(1, height)
        let old = measured[row.key]
        if old == nil, case .entry = row {
            var seen = learned[Self.kind(row)] ?? (0, 0, 0)
            seen.estimated += textEstimate(row)
            seen.real += h
            seen.count += 1
            learned[Self.kind(row)] = seen
        }
        measured[row.key] = h
        stale.remove(row.key)
        Self.kept[row.key] = (row, width, h)
        if Self.kept.count > 3000 { Self.kept.removeAll(keepingCapacity: true) }
        return old.map { abs($0 - h) > 0.5 } ?? true             // the table had an estimate: always noted
    }

    /// A built row's content reported its own height (a Show all, a row filling, a reply growing). What SwiftUI reports
    /// while a row is being rebuilt can be a passing value (seen: 1,114 pt for a 94 pt row), so the row is measured again
    /// before anything moves, once this pass is over.
    fileprivate func reported(_ key: String, _ height: CGFloat) {
        guard measuring == 0, width > 0, index[key] != nil, abs((measured[key] ?? -1) - ceil(height)) > 0.5 else { return }
        reportedKeys.insert(key)
        // measured again in the table's next layout, which comes in this same layout pass (before the frame is drawn);
        // the run loop is the fallback
        table.needsLayout = true
        guard !reportedQueued else { return }
        reportedQueued = true
        RunLoop.main.perform(inModes: [.common]) { [weak self] in
            MainActor.assumeIsolated {
                self?.reportedQueued = false
                self?.remeasureReported()
            }
        }
    }

    private var reportedKeys = Set<String>()
    private var reportedQueued = false

    private func remeasureReported(inLayout: Bool = false) {
        let keys = reportedKeys
        guard !keys.isEmpty else { return }
        reportedKeys = []
        guard width > 0, adjusting == 0 else { return }
        for key in keys {
            guard let i = index[key], hosts[key] != nil else { continue }
            let anchor = currentAnchor(changing: i)
            if measure(i) {
                noteHeights(IndexSet(integer: i))
                restore(anchor)
                if !inLayout { tileNow() }
            }
        }
        queueReport()
    }

    private func noteHeights(_ changed: IndexSet) {
        guard !changed.isEmpty else { return }
        adjusting += 1
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        table.placing += 1
        table.noteHeightOfRows(withIndexesChanged: changed)
        table.tile()
        table.placing -= 1
        NSAnimationContext.endGrouping()
        adjusting -= 1
    }

    // MARK: - Where the view is

    struct Anchor {
        var key: String?
        var offset: CGFloat
        var end: Bool
    }

    private var viewHeight: CGFloat { clip.bounds.height }
    private var most: CGFloat { max(0, table.frame.height - viewHeight) }
    var atEnd: Bool { table.frame.height <= viewHeight || clip.bounds.minY >= most - 1 }

    private func shown(at y: CGFloat) -> [Int] {
        guard !rows.isEmpty else { return [] }
        let range = table.rows(in: CGRect(x: 0, y: y, width: max(1, table.bounds.width), height: max(1, viewHeight)))
        guard range.length > 0 else { return [] }
        return Array(range.location..<min(rows.count, range.location + range.length))
    }

    /// What keeps the view still across a change: the end when at the end, else the first row in view whose height was
    /// already known (its place is real), else the first row in view; `changing`: a row whose own height changes counts
    /// as known, so a row that grows (a Show all, a reply filling) grows downwards from where it stands.
    private func currentAnchor(changing: Int? = nil) -> Anchor {
        if atEnd && clip.bounds.minY > 0.5 { return Anchor(key: nil, offset: 0, end: true) }
        let y = clip.bounds.minY
        let inView = shown(at: y)
        let pick = inView.first(where: { isMeasured($0) || $0 == changing }) ?? inView.first
        guard let pick else { return Anchor(key: nil, offset: 0, end: false) }
        return Anchor(key: rows[pick].key, offset: table.rect(ofRow: pick).minY - y, end: false)
    }

    private func place(for anchor: Anchor) -> CGFloat {
        if anchor.end { return most }
        guard let key = anchor.key, let i = index[key] else { return min(clip.bounds.minY, most) }
        return min(max(0, table.rect(ofRow: i).minY - anchor.offset), most)
    }

    private func restore(_ anchor: Anchor) {
        set(place(for: anchor))
    }

    /// The table puts its rows where the view now is in this pass, even when the change came inside a layout pass
    /// (SwiftUI updating the list while it lays out the window): otherwise that frame can be drawn with no rows.
    private func tileNow() {
        table.needsLayout = true
        table.layoutSubtreeIfNeeded()
    }

    /// Puts the view at `y` now, without measuring (the caller did).
    private func set(_ y: CGFloat) {
        let target = CGPoint(x: clip.bounds.minX, y: min(max(0, y), most))
        guard abs(target.y - clip.bounds.minY) > 0.25 else { return }
        adjusting += 1
        clip.setBoundsOrigin(target)
        scroll.reflectScrolledClipView(clip)
        table.needsLayout = true
        adjusting -= 1
    }

    /// Before the view moves to `point` (the owner's scroll, the thumb, FERB's own): the rows it will show are measured
    /// and the place kept: the top at the top, the end at the end, else the first measured row in view.
    fileprivate func prepare(_ point: CGPoint) -> CGPoint {
        lastScrollAt = Date()
        guard width > 0, adjusting == 0, !rows.isEmpty else { return point }
        var y = point.y
        for _ in 0..<8 {
            let end = table.frame.height > viewHeight && y >= most - 1
            let top = y <= 0.5
            let inView = shown(at: y)
            let missing = inView.filter { !isMeasured($0) }
            guard !missing.isEmpty else { break }
            let keep = inView.first(where: isMeasured) ?? inView.first!
            let offset = table.rect(ofRow: keep).minY - y
            var changed = IndexSet()
            for i in missing where measure(i) { changed.insert(i) }
            noteHeights(changed)
            y = end ? most : top ? 0 : table.rect(ofRow: keep).minY - offset
        }
        if hosts.count > 64 { evict() }               // a long scroll without a pause builds many rows
        queuePrefetch()
        return CGPoint(x: point.x, y: min(max(0, y), most))
    }

    /// The rows in view measured (after a change of rows or of width), keeping the place.
    func measureShown() {
        guard width > 0, adjusting == 0, !rows.isEmpty else { return }
        for _ in 0..<8 {
            let missing = shown(at: clip.bounds.minY).filter { !isMeasured($0) }
            guard !missing.isEmpty else { return }
            let anchor = currentAnchor()
            var changed = IndexSet()
            for i in missing where measure(i) { changed.insert(i) }
            noteHeights(changed)
            restore(anchor)
        }
    }

    @objc private func boundsChanged() {
        if adjusting == 0 { measureShown() }            // a move that didn't come through scroll(to:)
        queueReport()
        queuePrefetch()
    }

    @objc private func frameChanged() {
        table.sizeLastColumnToFit()
        let w = table.bounds.width
        guard w > 0 else { return }
        if abs(w - width) > 0.5 {
            let anchor = currentAnchor()
            width = w
            measured = [:]
            stale = []
            given = [:]
            noteHeights(IndexSet(integersIn: 0..<rows.count))
            if let pending { self.pending = nil; measureShown(); pending() } else { restore(anchor); measureShown() }
        } else {
            measureShown()
        }
        queueReport()
        queuePrefetch()
    }

    /// The offset and height reported after this pass's changes.
    private func queueReport() {
        guard !reportQueued else { return }
        reportQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.reportQueued = false
            self.scrolled(self.clip.bounds.minY, self.table.frame.height)
        }
    }

    /// Rows within two screens of the view measured ahead, a few per pass while the owner isn't scrolling, nearest first.
    private func queuePrefetch() {
        guard !prefetchQueued else { return }
        prefetchQueued = true
        RunLoop.main.perform(inModes: [.default]) { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.prefetchQueued = false
                self.prefetch()
            }
        }
    }

    private func prefetch() {
        guard width > 0, !rows.isEmpty else { return }
        if Date().timeIntervalSince(lastScrollAt) < 0.12 {
            // once the owner stops scrolling (one wait at a time)
            guard !prefetchWaiting else { return }
            prefetchWaiting = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
                self?.prefetchWaiting = false
                self?.queuePrefetch()
            }
            return
        }
        let y = clip.bounds.minY
        let range = table.rows(in: CGRect(x: 0, y: y - 2 * viewHeight, width: max(1, table.bounds.width),
                                          height: 5 * viewHeight))
        let inView = Set(shown(at: y))
        let center = inView.min() ?? range.location
        let candidates = (range.location..<min(rows.count, range.location + range.length))
            .filter { !isMeasured($0) && !inView.contains($0) }
            .sorted { abs($0 - center) < abs($1 - center) }
        guard !candidates.isEmpty else { return evict() }
        let started = Date()
        var changed = IndexSet()
        let anchor = currentAnchor()
        for i in candidates {
            if measure(i) { changed.insert(i) }
            if Date().timeIntervalSince(started) > 0.006 { break }
        }
        noteHeights(changed)
        restore(anchor)
        if !changed.isEmpty { tileNow() }
        queueReport()
        queuePrefetch()
    }

    // MARK: - Moving the view (FERB's own)

    /// The newest entry, now or once the list has its width.
    func toEnd(animated: Bool) {
        guard width > 0 else { pending = { [weak self] in self?.toEnd(animated: false) }; return }
        measureEnd()
        move(to: most, animated: animated)
    }

    private func measureEnd() {
        for _ in 0..<8 {
            let missing = shown(at: most).filter { !isMeasured($0) }
            guard !missing.isEmpty else { return }
            var changed = IndexSet()
            for i in missing where measure(i) { changed.insert(i) }
            noteHeights(changed)
        }
    }

    /// Brings `target` into view: an entry or a question by its row, an item inside an entry by its marker once its row
    /// is built (`holder`: the entry drawing it).
    func reveal(_ target: String, holder: String?, anchor: UnitPoint, animated: Bool) {
        guard width > 0 else {
            pending = { [weak self] in self?.reveal(target, holder: holder, anchor: anchor, animated: false) }
            return
        }
        guard let rowKey = index[target] != nil ? target : holder, let i = index[rowKey] else { return }
        if !isMeasured(i), measure(i) { noteHeights(IndexSet(integer: i)) }
        var rect = table.rect(ofRow: i)
        rect.size.height -= rows[i].hosted ? Self.spacing : 0
        var y = anchor == .bottom ? rect.maxY - viewHeight : rect.minY
        if rowKey != target {
            // the entry in view so it is built and laid out, then the item by its marker, all before the frame is drawn
            let from = clip.bounds.minY
            set(prepare(CGPoint(x: 0, y: min(max(0, y), most))).y)
            table.layoutSubtreeIfNeeded()
            hosts[rowKey]?.controller.view.layoutSubtreeIfNeeded()
            if let mark = marks.view(target), mark.isDescendant(of: table) {
                let frame = mark.convert(mark.bounds, to: table)
                y = anchor == .bottom ? frame.maxY - viewHeight : frame.minY
            } else {
                y = clip.bounds.minY
            }
            if animated { set(from) }
        }
        move(to: y, animated: animated)
    }

    private func move(to y: CGFloat, animated: Bool) {
        let target = min(max(0, y), most)
        if animated, abs(target - clip.bounds.minY) < 2 * viewHeight {
            // the rows on the way measured first, so the animation shows them where they stand
            _ = prepare(CGPoint(x: 0, y: target))
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.3
                context.timingFunction = CAMediaTimingFunction(name: .easeOut)
                clip.animator().setBoundsOrigin(CGPoint(x: clip.bounds.minX, y: min(max(0, target), most)))
            }
            scroll.reflectScrolledClipView(clip)
        } else {
            clip.scroll(to: CGPoint(x: clip.bounds.minX, y: target))
            scroll.reflectScrolledClipView(clip)
        }
        queueReport()
    }

    // MARK: - Places, for the follow rules and the pills

    /// Where `target` is in the view's coordinates (0 is the view's top): an entry or a question by its row, an item
    /// inside a built entry by its marker, else by the entry drawing it.
    func frame(of target: String, holder: String?) -> CGRect? {
        let y = clip.bounds.minY
        if let mark = marks.view(target), mark.isDescendant(of: table) {
            return mark.convert(mark.bounds, to: table).offsetBy(dx: 0, dy: -y)
        }
        guard let key = index[target] != nil ? target : holder, let i = index[key] else { return nil }
        var rect = table.rect(ofRow: i)
        rect.size.height -= rows[i].hosted ? Self.spacing : 0
        return rect.offsetBy(dx: 0, dy: -y)
    }

    /// The entry row drawing `target` among the rows the list has now (an item FERB brings forward may be in an entry
    /// that came in after it was asked for).
    func holder(of target: String) -> String? {
        let entries = rows.compactMap { row -> TranscriptEntry? in
            if case let .entry(entry, _) = row { return entry } else { return nil }
        }
        return TranscriptTarget.entry(holding: target, in: entries).map(TranscriptTarget.entry)
    }

    /// The first entry still in view, for coming back to the same place (APP-745).
    func topEntry() -> String? {
        let y = clip.bounds.minY
        return shown(at: y).map { rows[$0].key }.first { key in
            TranscriptTarget.isEntry(key) && (index[key].map { table.rect(ofRow: $0).maxY - y > 1 } ?? false)
        }
    }

    var offset: CGFloat { clip.bounds.minY }
    var contentHeight: CGFloat { table.frame.height }

    // MARK: - Built rows

    private func root(_ row: TranscriptRow) -> AnyView {
        let key = row.key
        return AnyView(content(row)
            .environment(\.transcriptMarks, marks)
            .font(.system(size: 12))
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.bottom, Self.spacing)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { [weak self] height in
                self?.reported(key, height)
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .environment(\.self, environment))
    }

    private func host(for i: Int) -> RowHost {
        let row = rows[i]
        tick += 1
        if let host = hosts[row.key] {
            if host.row != row {
                host.row = row
                host.controller.rootView = root(row)
            }
            host.used = tick
            return host
        }
        let controller = NSHostingController(rootView: root(row))
        controller.sizingOptions = []
        if #available(macOS 13.3, *) { controller.safeAreaRegions = [] }
        let host = RowHost(controller: controller, row: row, used: tick)
        hosts[row.key] = host
        return host
    }

    private func drop(_ host: RowHost) {
        host.controller.view.removeFromSuperview()
        hosts[host.row.key] = nil
    }

    /// Rows built but far from the view are let go (their heights are kept): at most a few dozen stay built.
    private func evict() {
        guard hosts.count > 40 else { return }
        let near = Set((Range(table.rows(in: clip.bounds.insetBy(dx: 0, dy: -2 * viewHeight))) ?? 0..<0)
            .compactMap { $0 < rows.count ? rows[$0].key : nil })
        let idle = hosts.values.filter { $0.controller.view.superview == nil && !near.contains($0.row.key) }
            .sorted { $0.used < $1.used }
        for host in idle.prefix(hosts.count - 24) { drop(host) }
    }

    // MARK: - NSTableView

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat { row < rows.count ? height(row) : 1 }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard row < rows.count else { return nil }
        guard rows[row].hosted else {
            let id = NSUserInterfaceItemIdentifier("space")
            if let space = tableView.makeView(withIdentifier: id, owner: nil) { return space }
            let space = NSView()
            space.identifier = id
            return space
        }
        return host(for: row).controller.view
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { ListRowView() }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

    func selectionShouldChange(in tableView: NSTableView) -> Bool { false }

    /// Whether the rows in view are measured: after a change of rows, a layout measures them before drawing.
    fileprivate func beforeLayout() {
        guard adjusting == 0 else { return }
        remeasureReported(inLayout: true)
        measureShown()
    }
}

/// A built row: its SwiftUI host and the row it shows.
@MainActor
final class RowHost {
    let controller: NSHostingController<AnyView>
    var row: TranscriptRow
    var used: Int
    init(controller: NSHostingController<AnyView>, row: TranscriptRow, used: Int) {
        self.controller = controller
        self.row = row
        self.used = used
    }
}

/// The list's table: rows in view measured before it lays them out.
final class ListTable: NSTableView {
    weak var list: TranscriptList?
    /// While the table lays out or takes new row heights it may move the view itself, to keep what it thinks is the
    /// top row still; the list keeps the place (TranscriptList.Anchor), so those moves are refused.
    var placing = 0
    override func layout() {
        list?.beforeLayout()
        placing += 1
        super.layout()
        placing -= 1
    }
    override var acceptsFirstResponder: Bool { false }
    /// A click goes straight to what it lands on (a text to select, a field to type in): rows are never selected first.
    override func validateProposedFirstResponder(_ responder: NSResponder, for event: NSEvent?) -> Bool { true }
}

/// The list's clip view: every move to a point measures the rows shown there first (TranscriptList.prepare).
final class ListClip: NSClipView {
    weak var list: TranscriptList?
    override func scroll(to newOrigin: NSPoint) {
        if let table = documentView as? ListTable, table.placing > 0 { return }
        super.scroll(to: list?.prepare(newOrigin) ?? newOrigin)
        // the rows for the new place are put in the layout pass ahead of this frame's drawing
        (documentView as? NSTableView)?.needsLayout = true
    }
}

/// A row with nothing drawn behind or around it: the transcript is on FERB's glass.
final class ListRowView: NSTableRowView {
    override func drawBackground(in dirtyRect: NSRect) {}
    override func drawSelection(in dirtyRect: NSRect) {}
    override var isOpaque: Bool { false }
}

/// The transcript list in SwiftUI (APP-815): its rows, how each is drawn, and the offset reports.
struct TranscriptListView: NSViewRepresentable {
    let list: TranscriptList
    let rows: [TranscriptRow]
    let viewport: CGFloat
    let content: (TranscriptRow) -> AnyView
    let scrolled: (CGFloat, CGFloat) -> Void

    func makeNSView(context: Context) -> NSScrollView { list.scroll }

    func updateNSView(_ view: NSScrollView, context: Context) {
        list.content = content
        list.scrolled = scrolled
        list.viewport = viewport
        list.setEnvironment(context.environment)
        list.update(rows: rows)
    }
}
