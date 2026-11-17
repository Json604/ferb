import AppKit
import NotchCore
import SwiftUI
import UniformTypeIdentifiers
import os

/// Files and folders dragged onto FERB (owner, 2026-11-05: "When doing drag n drop file/folder, FERB chat should open
/// up"; D17, D18). Every drop target (the docked handle and its edge row, the edge pill and its resting line, the open
/// chat and its composer) reports here, so one path decides, opens and attaches:
///
/// - a drag that isn't files, folders, promised files and pictures only (text, links: FileDrop, APP-930) or comes while
///   FERB can't take files (the tour, dictation) gets no operation, so the system slides it back;
/// - held over the handle or pill for `springDelay`, the chat opens as a press opens it, whatever the hover setting;
///   the pill un-rests at once;
/// - a drop attaches every file at once (`NotchModel.drop`): the files wait in the composer until the owner sends
///   them, with or without words; nothing is sent. A picture with no file is written as a PNG and a promised file is
///   received into FERB's own folder (ReceivedFiles, APP-936), shown as arriving until it is there;
/// - a chat the spring opened with nothing dropped folds once the drag has left every target for `leaveGrace` (the
///   sticky window, which the pointer must travel to, folds only when the drag ends with nothing dropped);
/// - FERB reads the pointer itself during a file drag (`pointer`): over the closed handle or pill it counts as the drag
///   over the handle, whether or not the window server routes the drag to that window (APP-820).
@MainActor
final class FileDropSession {
    /// `pointer`: the handle's place on screen as FERB's own pointer reading finds it during a file drag (the
    /// coordinator), beside the handle's and edge row's AppKit destinations.
    enum Target: Hashable { case handle, edgeRow, pointer, chat, composer }
    /// The targets over the handle: the spring opens the chat from these.
    static let handleTargets: Set<Target> = [.handle, .edgeRow, .pointer]

    /// Each target's enter, exit and drop with the operation given (structure only: no names or paths), at notice level
    /// so the system keeps them for a later `log show`.
    static let log = Logger(subsystem: "com.ferb.FERB", category: "drops")
    static let springDelay: Duration = .milliseconds(300)
    static let leaveGrace: Duration = .milliseconds(250)

    /// FERB can take files now (NotchModel.acceptsDroppedFiles).
    var accepts: () -> Bool = { false }
    /// The chat's body shows.
    var isOpen: () -> Bool = { false }
    /// The pointer reached the handle or pill: un-rest the pill (the coordinator).
    var onReach: () -> Void = {}
    /// Open the chat by the press path, without a drop yet (the coordinator).
    var onSpring: () -> Void = {}
    /// Fold the chat the spring opened (the coordinator).
    var onFold: () -> Void = {}
    /// The pointer is over FERB's surface (the coordinator): a target can go quiet while the chat opens under a still
    /// pointer (the handle moves into the opening chat), so the fold waits until the pointer is really away.
    var pointerOver: () -> Bool = { false }
    /// The spring-opened chat folds when the drag leaves every target (false: only when the drag ends; the sticky
    /// window, which the pointer must travel to from the pill).
    var foldsWhenLeft: () -> Bool = { true }
    /// Attach what was dropped (NotchModel.drop); false when nothing was taken.
    var onDrop: (DroppedItems) -> Bool = { _ in false }
    /// How the spring delay and the leave grace pass (tests end them at once).
    var wait: @MainActor (Duration) async throws -> Void = { try await Task.sleep(for: $0) }

    /// A drag FERB can take is over one of its targets, or no longer is (the chat's green tint).
    var onTargeted: (Bool) -> Void = { _ in }

    private(set) var over: Set<Target> = [] {
        didSet { if over.isEmpty != oldValue.isEmpty { onTargeted(!over.isEmpty) } }
    }
    private(set) var springTask: Task<Void, Never>?
    private(set) var leaveTask: Task<Void, Never>?
    /// The spring opened the chat during this drag; a drop or the fold clears it.
    private(set) var springOpened = false

    // MARK: Reading a drag

    /// The promise types AppKit knows today, besides FileDrop's own list.
    static var promiseTypes: Set<String> { Set(NSFilePromiseReceiver.readableDraggedTypes) }

    /// Each dragged item's pasteboard types.
    static func itemTypes(_ pasteboard: NSPasteboard) -> [[String]] {
        (pasteboard.pasteboardItems ?? []).map { $0.types.map(\.rawValue) }
    }

    /// The types every drop target registers for: file URLs, the promise types and the pictures FERB reads.
    static var dragTypes: [NSPasteboard.PasteboardType] {
        let raw = [FileDrop.fileURLType] + ClipboardContent.imageTypes + FileDrop.promiseTypes.sorted()
            + NSFilePromiseReceiver.readableDraggedTypes
        var seen = Set<String>()
        return raw.filter { seen.insert($0).inserted }.map { NSPasteboard.PasteboardType($0) }
    }

    /// The drag is one FERB takes when it can: files, promised files and pictures only (APP-930, APP-931).
    static func takes(_ pasteboard: NSPasteboard) -> Bool {
        FileDrop.accepts(itemTypes(pasteboard), promises: promiseTypes)
    }

    /// What a drag carries, read at the drop (a promise must be read then), or nil when the drag is anything else.
    static func items(on pasteboard: NSPasteboard) -> DroppedItems? {
        guard takes(pasteboard) else { return nil }
        var dropped = DroppedItems()
        var promised = 0
        for item in pasteboard.pasteboardItems ?? [] {
            let types = item.types.map(\.rawValue)
            switch FileDrop.carried(types, promises: promiseTypes) {
            case .file:
                if let text = item.string(forType: .fileURL), let url = URL(string: text), url.isFileURL {
                    dropped.files.append(url.standardizedFileURL)
                } else {
                    dropped.unreadable += 1
                }
            case .image:
                if let png = ComposerPaste.png(types: types, data: { item.data(forType: NSPasteboard.PasteboardType($0)) }) {
                    dropped.images.append(png)
                } else {
                    dropped.unreadable += 1
                }
            case .promise:
                promised += 1
            case nil:
                return nil
            }
        }
        if promised > 0 {
            let receivers = pasteboard.readObjects(forClasses: [NSFilePromiseReceiver.self]) as? [NSFilePromiseReceiver] ?? []
            dropped.promises = receivers.map(SystemPromise.init)
            dropped.unreadable += max(0, promised - receivers.count)
        }
        return dropped
    }

    /// The operation a drag over FERB gets: copy for what FERB can take now, none otherwise.
    func operation(_ pasteboard: NSPasteboard) -> NSDragOperation {
        accepts() && Self.takes(pasteboard) ? .copy : []
    }

    // MARK: Targets

    func entered(_ target: Target, _ pasteboard: NSPasteboard) -> NSDragOperation {
        let operation = operation(pasteboard)
        if !over.contains(target) {
            Self.log.notice("Drag entered \(String(describing: target), privacy: .public): operation \(operation.rawValue, privacy: .public)")
        }
        guard !operation.isEmpty else { return operation }
        over.insert(target)
        leaveTask?.cancel()
        leaveTask = nil
        if Self.handleTargets.contains(target) {
            onReach()
            if !isOpen(), springTask == nil {
                springTask = Task { [weak self, wait] in
                    do { try await wait(Self.springDelay) } catch { return }
                    guard let self, !Task.isCancelled else { return }
                    self.springTask = nil
                    guard self.accepts(), !self.isOpen(), self.overHandle else { return }
                    self.springOpened = true
                    self.onSpring()
                }
            }
        }
        return operation
    }

    func updated(_ target: Target, _ pasteboard: NSPasteboard) -> NSDragOperation {
        over.contains(target) ? operation(pasteboard) : entered(target, pasteboard)
    }

    func exited(_ target: Target) {
        guard over.remove(target) != nil else { return }
        Self.log.notice("Drag left \(String(describing: target), privacy: .public); still over \(self.over.count, privacy: .public)")
        if !overHandle {
            springTask?.cancel()
            springTask = nil
        }
        guard over.isEmpty, springOpened, foldsWhenLeft() else { return }
        leaveTask?.cancel()
        leaveTask = Task { [weak self, wait] in
            while true {
                do { try await wait(Self.leaveGrace) } catch { return }
                guard let self, !Task.isCancelled, self.over.isEmpty, self.springOpened else { return }
                if self.pointerOver() { continue }
                self.leaveTask = nil
                self.springOpened = false
                self.onFold()
                return
            }
        }
    }

    /// A drop on any target: everything at once.
    func drop(_ pasteboard: NSPasteboard) -> Bool {
        let items = accepts() ? Self.items(on: pasteboard) : nil
        Self.log.notice("""
            Drop: \(items?.files.count ?? 0, privacy: .public) files, \(items?.promises.count ?? 0, privacy: .public) \
            promised, \(items?.images.count ?? 0, privacy: .public) pictures, \(items?.unreadable ?? 0, privacy: .public) unreadable
            """)
        reset()
        guard let items else { return false }
        return onDrop(items)
    }

    /// The mouse button came up: every target counts as left, and a chat the spring opened with nothing dropped folds.
    func dragEnded() {
        for target in over { exited(target) }
        guard springOpened else { return }       // the drag is over and nothing was dropped
        leaveTask?.cancel()
        leaveTask = nil
        springOpened = false
        onFold()
    }

    /// The chat closed: forget this drag.
    func reset() {
        springTask?.cancel()
        springTask = nil
        leaveTask?.cancel()
        leaveTask = nil
        over = []
        springOpened = false
    }

    private var overHandle: Bool { !over.isDisjoint(with: Self.handleTargets) }
}

/// The open chat as a drop target (SwiftUI): the same session as the handle. SwiftUI's DropInfo doesn't carry the
/// pasteboard, so the drag pasteboard (`NSPasteboard.Name.drag`, what AppKit's NSDraggingInfo reads too) is read.
struct ChatFileDrop: DropDelegate {
    /// What the chat's SwiftUI drop takes to its delegate: every item, so a promise or a picture reaches validateDrop,
    /// which reads the drag pasteboard and refuses text and links (FileDropSession.operation).
    static let types: [UTType] = [.fileURL, .image, .item]
    let session: FileDropSession
    var pasteboard: () -> NSPasteboard = { NSPasteboard(name: .drag) }

    func validateDrop(info: DropInfo) -> Bool { !session.operation(pasteboard()).isEmpty }
    func dropEntered(info: DropInfo) { _ = session.entered(.chat, pasteboard()) }
    func dropUpdated(info: DropInfo) -> DropProposal? {
        session.updated(.chat, pasteboard()).isEmpty ? DropProposal(operation: .forbidden) : DropProposal(operation: .copy)
    }
    func dropExited(info: DropInfo) { session.exited(.chat) }
    func performDrop(info: DropInfo) -> Bool { session.drop(pasteboard()) }
}

/// What one drop carries (FileDropSession.items): files on disk, pictures as PNG data, promised files still to come, and
/// how many items couldn't be read at all (said in the chat, never lost silently).
struct DroppedItems {
    var files: [URL] = []
    var images: [Data] = []
    var promises: [PromisedFile] = []
    var unreadable = 0
    var isEmpty: Bool { files.isEmpty && images.isEmpty && promises.isEmpty && unreadable == 0 }
}

/// A file its source writes when asked (APP-936): AppKit's NSFilePromiseReceiver, or a test's stand-in.
@MainActor
protocol PromisedFile: AnyObject {
    /// The name the source gave, when it gave one (shown while the file arrives).
    var name: String? { get }
    /// Asks the source to write the file into `folder`; `arrived` runs on the main actor once per file written, or with
    /// the error that stopped it.
    func receive(into folder: URL, arrived: @escaping @MainActor (Result<URL, Error>) -> Void)
}

/// A promise read from a drag pasteboard.
@MainActor
final class SystemPromise: PromisedFile {
    /// The queue the sources' writes are awaited on, off the main thread.
    private static let queue: OperationQueue = {
        let queue = OperationQueue()
        queue.name = "FERB promised files"
        queue.qualityOfService = .userInitiated
        return queue
    }()
    let receiver: NSFilePromiseReceiver
    init(_ receiver: NSFilePromiseReceiver) { self.receiver = receiver }
    var name: String? { receiver.fileNames.first }
    func receive(into folder: URL, arrived: @escaping @MainActor (Result<URL, Error>) -> Void) {
        receiver.receivePromisedFiles(atDestination: folder, options: [:], operationQueue: Self.queue) { url, error in
            let result: Result<URL, Error> = error.map { .failure($0) } ?? .success(url)
            Task { @MainActor in arrived(result) }
        }
    }
}
