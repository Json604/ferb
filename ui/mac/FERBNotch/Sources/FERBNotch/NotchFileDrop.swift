import AppKit
import NotchCore
import SwiftUI
import UniformTypeIdentifiers
import os

/// Files and folders dragged onto FERB (owner, 2026-11-05: "When doing drag n drop file/folder, FERB chat should open
/// up"; D17, D18). Every drop target (the docked handle and its edge row, the edge pill and its resting line, the open
/// chat and its composer) reports here, so one path decides, opens and attaches:
///
/// - a drag that isn't files and folders only (text, links, promised files) or comes while FERB can't take files (the
///   tour, dictation) gets no operation, so the system slides it back;
/// - held over the handle or pill for `springDelay`, the chat opens as a press opens it, whatever the hover setting;
///   the pill un-rests at once;
/// - a drop attaches every file at once (`NotchModel.dropFiles`): the files wait in the composer until the owner sends
///   them, with or without words; nothing is sent;
/// - a chat the spring opened with nothing dropped folds once the drag has left every target for `leaveGrace`.
@MainActor
final class FileDropSession {
    enum Target: Hashable { case handle, edgeRow, chat, composer }

    /// Each target's enter, exit and drop with the operation given (structure only: no names or paths).
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
    /// Attach dropped files (NotchModel.dropFiles); false when nothing was taken.
    var onDrop: ([URL]) -> Bool = { _ in false }
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

    /// The dragged files and folders, or nil when the drag is anything else (D18).
    static func files(on pasteboard: NSPasteboard) -> [URL]? {
        guard FileDrop.accepts(itemTypes(pasteboard), promises: promiseTypes) else { return nil }
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
        guard let urls, !urls.isEmpty else { return nil }
        return urls
    }

    /// The operation a drag over FERB gets: copy for files and folders FERB can take now, none otherwise.
    func operation(_ pasteboard: NSPasteboard) -> NSDragOperation {
        accepts() && FileDrop.accepts(Self.itemTypes(pasteboard), promises: Self.promiseTypes) ? .copy : []
    }

    // MARK: Targets

    func entered(_ target: Target, _ pasteboard: NSPasteboard) -> NSDragOperation {
        let operation = operation(pasteboard)
        if !over.contains(target) {
            Self.log.info("Drag entered \(String(describing: target), privacy: .public): operation \(operation.rawValue, privacy: .public)")
        }
        guard !operation.isEmpty else { return operation }
        over.insert(target)
        leaveTask?.cancel()
        leaveTask = nil
        if target == .handle || target == .edgeRow {
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
        Self.log.info("Drag left \(String(describing: target), privacy: .public); still over \(self.over.count, privacy: .public)")
        if !overHandle {
            springTask?.cancel()
            springTask = nil
        }
        guard over.isEmpty, springOpened else { return }
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

    /// A drop on any target: every file at once.
    func drop(_ pasteboard: NSPasteboard) -> Bool {
        let urls = accepts() ? Self.files(on: pasteboard) : nil
        Self.log.info("Drop: \(urls?.count ?? 0, privacy: .public) files taken")
        reset()
        guard let urls else { return false }
        return onDrop(urls)
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

    private var overHandle: Bool { over.contains(.handle) || over.contains(.edgeRow) }
}

/// The open chat as a drop target (SwiftUI): the same session as the handle. SwiftUI's DropInfo doesn't carry the
/// pasteboard, so the drag pasteboard (`NSPasteboard.Name.drag`, what AppKit's NSDraggingInfo reads too) is read.
struct ChatFileDrop: DropDelegate {
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
