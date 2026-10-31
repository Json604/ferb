import AppKit
import QuickLookUI
import SwiftUI

/// Keep the preview open while crossing the short gap from its file row.
@MainActor
final class ArtifactHoverState: ObservableObject {
    @Published var isPresented = false
    @Published private(set) var isHovered = false
    private var anchorInside = false
    private var previewInside = false
    private var dismissal: Task<Void, Never>?

    func anchorHovered(_ inside: Bool) { if isHovered != inside { isHovered = inside }; anchorInside = inside; update() }
    func previewHovered(_ inside: Bool) { previewInside = inside; update() }
    func dismiss() {
        dismissal?.cancel()
        dismissal = nil
        if isHovered { isHovered = false }
        anchorInside = false
        previewInside = false
        isPresented = false
    }
    private func update() {
        dismissal?.cancel()
        if anchorInside || previewInside { isPresented = true; return }
        dismissal = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(200)) } catch { return }
            self?.isPresented = false
        }
    }
    deinit { dismissal?.cancel() }
}

/// The trigger and disclosure are one hover region, including a brief crossing grace.
@MainActor
final class SitesHoverState: ObservableObject {
    @Published var isPresented = false
    @Published private(set) var isHovered = false
    private var anchorInside = false
    private var previewInside = false
    private var keyboardOpened = false
    private var dismissal: Task<Void, Never>?
    func anchorHovered(_ inside: Bool) {
        isHovered = inside
        anchorInside = inside
        update()
    }
    func previewHovered(_ inside: Bool) { previewInside = inside; update() }
    func activate(keyboard: Bool) {
        keyboardOpened = keyboard
        dismissal?.cancel()
        isPresented = true
    }
    private func update() {
        dismissal?.cancel()
        if anchorInside || previewInside || keyboardOpened { isPresented = true; return }
        dismissal = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(200)) } catch { return }
            self?.isPresented = false
        }
    }
    func dismiss() {
        dismissal?.cancel()
        dismissal = nil
        anchorInside = false
        previewInside = false
        keyboardOpened = false
        isHovered = false
        isPresented = false
    }
    deinit { dismissal?.cancel() }
}

/// SwiftUI hover tracking may only be active for a key window. The notch is
/// deliberately non-key, so anchors and popovers use explicit activeAlways areas.
/// This background view never participates in hit testing: buttons and file drags
/// keep their existing native event route.
struct NativePreviewHover: NSViewRepresentable {
    var pointingHand = false
    let changed: (Bool) -> Void
    func makeNSView(context: Context) -> PreviewHoverTrackingView {
        let view = PreviewHoverTrackingView()
        view.changed = changed
        view.pointingHand = pointingHand
        return view
    }
    func updateNSView(_ view: PreviewHoverTrackingView, context: Context) {
        view.changed = changed
        view.pointingHand = pointingHand
    }
    static func dismantleNSView(_ view: PreviewHoverTrackingView, coordinator: ()) {
        let changed = view.changed
        view.changed = nil
        view.clearHover()
        DispatchQueue.main.async { changed?(false) }
    }
}

/// SwiftUI can leave AppKit's automatic tracking region stale at the last row of
/// a scroll view. Reconcile the same native anchors from FERB's existing pointer
/// monitors, using their current geometry rather than waiting for a missed enter.
@MainActor
final class PreviewHoverRouter {
    static let shared = PreviewHoverRouter()
    private let anchors = NSHashTable<PreviewHoverTrackingView>.weakObjects()
    private var scheduled = false

    func register(_ view: PreviewHoverTrackingView) { anchors.add(view); schedule() }
    func unregister(_ view: PreviewHoverTrackingView) { anchors.remove(view) }
    func routeCurrentPointer() {
        let point = NSEvent.mouseLocation
        route(point: point,
              frontmostWindowNumber: NSWindow.windowNumber(at: point, belowWindowWithWindowNumber: 0))
    }
    func route(point: NSPoint, frontmostWindowNumber: Int) {
        let hits = anchors.allObjects.map { ($0, $0.containsPointer(point: point, frontmostWindowNumber: frontmostWindowNumber)) }
        // Clear the previous hand before a new anchor claims it. Weak-table order
        // is undefined, so interleaving exits and enters can leave the arrow set.
        for (view, hit) in hits where !hit { view.clearHover() }
        for (view, hit) in hits where hit { view.acceptHover() }
    }
    func schedule() {
        guard !scheduled, anchors.allObjects.contains(where: { $0.window?.isVisible == true }) else { return }
        scheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.scheduled = false
            self.routeCurrentPointer()
        }
    }
}

class PreviewHoverTrackingView: NSView {
    var changed: ((Bool) -> Void)?
    var pointingHand = false {
        didSet { if oldValue != pointingHand { window?.invalidateCursorRects(for: self) } }
    }
    private var inside = false
    private var area: NSTrackingArea?
    private var clipObserver: NSObjectProtocol?
    // SwiftUI's platform host can return the entire scroll viewport here, with
    // an origin outside this small representable. Normalize the NSView contract
    // itself so AppKit can maintain an automatic inVisibleRect tracking area as
    // scrolling/reveal moves the anchor, rather than snapshotting a tracking rect.
    override var visibleRect: NSRect { clippedToBounds(super.visibleRect) }
    var hoverRect: NSRect { clippedToBounds(visibleRect) }
    private func clippedToBounds(_ visible: NSRect) -> NSRect {
        func isUsable(_ rect: NSRect) -> Bool {
            !rect.isEmpty && rect.origin.x.isFinite && rect.origin.y.isFinite
                && rect.size.width.isFinite && rect.size.height.isFinite
        }
        guard isUsable(bounds), isUsable(visible) else { return .zero }
        let clipped = bounds.intersection(visible)
        // CGRect.null has an infinite origin, which AppKit rejects for cursors.
        return isUsable(clipped) ? clipped : .zero
    }
    override func resetCursorRects() {
        super.resetCursorRects()
        let rect = hoverRect
        if pointingHand, !rect.isEmpty { addCursorRect(rect, cursor: .pointingHand) }
    }
    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        window?.invalidateCursorRects(for: self)
        reconcileTrackingArea()
        PreviewHoverRouter.shared.schedule()
    }
    override func setFrameOrigin(_ newOrigin: NSPoint) {
        super.setFrameOrigin(newOrigin)
        window?.invalidateCursorRects(for: self)
        reconcileTrackingArea()
        PreviewHoverRouter.shared.schedule()
    }
    // macOS 27.2 no longer calls updateTrackingAreas when a view joins a window, moves, resizes or scrolls into view
    // (measured 2026-10-31 in a test window, plain NSView too). The inVisibleRect area follows the visible part by
    // itself once it exists, so only its existence is reconciled here: added when the pill becomes visible,
    // removed when it can't be seen.
    private func reconcileTrackingArea() {
        guard window != nil, (area == nil) != hoverRect.isEmpty else { return }
        updateTrackingAreas()
    }
    override func cursorUpdate(with event: NSEvent) { updatePointer(with: event) }
    override func mouseMoved(with event: NSEvent) { updatePointer(with: event) }
    // Active-always tracking can receive events behind another non-key popover.
    // Only the topmost window under the event may own hover or change its cursor.
    func pointerBelongsToWindow(_ event: NSEvent) -> Bool {
        guard let window else { return false }
        guard window.isVisible else { return true } // offscreen layout/test hosts
        let point = window.convertPoint(toScreen: event.locationInWindow)
        return NSWindow.windowNumber(at: point, belowWindowWithWindowNumber: 0) == window.windowNumber
    }
    func containsPointer(point: NSPoint, frontmostWindowNumber: Int) -> Bool {
        guard let window, window.windowNumber == frontmostWindowNumber, !isHiddenOrHasHiddenAncestor else { return false }
        return hoverRect.contains(convert(window.convertPoint(fromScreen: point), from: nil))
    }
    private func updatePointer(with event: NSEvent) {
        if window?.isVisible == true {
            // Queued enter/exit coordinates can refer to an earlier scroll position.
            // A stale exit must not undo the monitor's current hit result.
            PreviewHoverRouter.shared.routeCurrentPointer()
            return
        }
        guard hoverRect.contains(convert(event.locationInWindow, from: nil)),
              pointerBelongsToWindow(event) else { clearHover(); return }
        acceptHover()
    }
    func acceptHover() {
        guard !inside else { return }
        applyCursor()
        inside = true
        changed?(true)
    }
    private func applyCursor() {
        if pointingHand { applyPointerCursor(.pointingHand) }
    }
    func applyPointerCursor(_ cursor: NSCursor) {
        if window?.isVisible == true, cursor !== NSCursor.arrow || NSCursor.current == .pointingHand { cursor.set() }
    }
    func clearHover() {
        guard inside else { return }
        if inside, pointingHand { applyPointerCursor(.arrow) }
        inside = false
        changed?(false)
        window?.invalidateCursorRects(for: self)
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override func updateTrackingAreas() {
        if let area { removeTrackingArea(area) }
        area = nil
        let rect = hoverRect
        if !rect.isEmpty {
            let area = NSTrackingArea(rect: .zero,
                options: [.activeAlways, .inVisibleRect, .mouseEnteredAndExited, .mouseMoved, .cursorUpdate], owner: self)
            addTrackingArea(area)
            self.area = area
        } else {
            clearHover()
        }
        super.updateTrackingAreas()
        PreviewHoverRouter.shared.schedule()
    }
    override func mouseEntered(with event: NSEvent) { updatePointer(with: event) }
    override func mouseExited(with event: NSEvent) {
        if window?.isVisible == true { updatePointer(with: event) }
        else { clearHover() }
    }
    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if window !== newWindow, inside {
            let changed = changed
            self.changed = nil
            clearHover()
            self.changed = changed
            DispatchQueue.main.async { [weak self] in
                guard self?.inside != true else { return }
                changed?(false)
            }
        }
        super.viewWillMove(toWindow: newWindow)
    }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let clipObserver { NotificationCenter.default.removeObserver(clipObserver) }
        clipObserver = nil
        if window == nil { PreviewHoverRouter.shared.unregister(self) }
        else {
            PreviewHoverRouter.shared.register(self)
            if let clip = enclosingScrollView?.contentView {
                clip.postsBoundsChangedNotifications = true
                clipObserver = NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification,
                    object: clip, queue: .main) { [weak self] _ in
                        MainActor.assumeIsolated {
                            self?.reconcileTrackingArea()
                            PreviewHoverRouter.shared.schedule()
                        }
                    }
            }
        }
        if window == nil { clearHover() }
        else {
            window?.invalidateCursorRects(for: self)
            reconcileTrackingArea()
        }
    }
    deinit { if let clipObserver { NotificationCenter.default.removeObserver(clipObserver) } }
}

struct ArtifactRow: View {
    let url: URL
    @ObservedObject var model: NotchModel
    func openPreview() { model.onQuickLook?(url) }
    var body: some View {
        Button(action: openPreview) {
            Label(url.lastPathComponent, systemImage: "doc").lineLimit(1)
        }
        .buttonStyle(FerbButtonStyle(quiet: true))
        .modifier(ArtifactPreviewPresentation(url: url, model: model))
    }
}

struct ComposerAttachmentCard: View {
    let url: URL
    @ObservedObject var model: NotchModel
    var body: some View {
        Button { model.onQuickLook?(url) } label: {
            VStack(alignment: .leading, spacing: 7) {
                Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                    .resizable().scaledToFit().frame(width: 30, height: 32)
                    .frame(maxWidth: .infinity)
                Text(url.lastPathComponent).font(.system(size: 11))
                    .lineLimit(1).truncationMode(.middle)
            }
            .padding(10).frame(width: 116, height: 84)
            .background(.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 10))
            .contentShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Preview \(url.lastPathComponent)")
        .modifier(ArtifactPreviewPresentation(url: url, model: model))
        .overlay(alignment: .topTrailing) {
            Button { model.removeAttachment(url) } label: {
                Image(systemName: "xmark.circle.fill")
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.secondary, .background)
                    .font(.system(size: 14)).frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(url.lastPathComponent)")
            .help("Remove attachment")
        }
    }
}

/// Used by reply files and composer cards alike.
struct ArtifactPreviewPresentation: ViewModifier {
    let url: URL
    @ObservedObject var model: NotchModel
    @StateObject private var hover = ArtifactHoverState()
    func body(content: Content) -> some View {
        content
        .onDrag { hover.dismiss(); return NSItemProvider(object: url as NSURL) }
        .contextMenu {
            Button("Open in default app") { model.onOpenDocument?(url) }
            Button("Quick Look") { hover.dismiss(); model.onQuickLook?(url) }
            Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
        }
        .overlay {
            RoundedRectangle(cornerRadius: 10).fill(Color.accentColor.opacity(hover.isHovered ? 0.12 : 0))
                .allowsHitTesting(false)
        }
        .background(NativePreviewHover(pointingHand: true, changed: hover.anchorHovered))
        .popover(isPresented: $hover.isPresented, arrowEdge: .top) {
            VStack(alignment: .leading, spacing: 8) {
                Text(url.lastPathComponent).font(.headline).lineLimit(2)
                if url.isFileURL && FileManager.default.isReadableFile(atPath: url.path) {
                    ArtifactQuickLook(url: url).frame(width: 320, height: 220)
                } else {
                    Label("This file is no longer available.", systemImage: "doc.badge.ellipsis")
                        .foregroundStyle(.secondary).frame(width: 320, height: 100)
                }
            }
            .padding(12)
            .background(NativePreviewHover(changed: hover.previewHovered))
            .background(TaskPreviewWindow(model: model))
        }
        .onChange(of: model.expanded) { _, expanded in if !expanded { hover.dismiss() } }
        .onChange(of: url) { _, _ in hover.dismiss() }
        .onChange(of: model.previewDismissalRevision) { _, _ in hover.dismiss() }
        .onDisappear { hover.dismiss() }
    }
}

/// Quick Look owns format support and loading; dismantling clears the old item,
/// so recycled rows cannot retain another artifact's preview or playback.
struct ArtifactQuickLook: NSViewRepresentable {
    let url: URL
    func makeNSView(context: Context) -> QLPreviewView {
        let view = QLPreviewView(frame: .zero, style: .compact)!
        view.autostarts = false
        return view
    }
    func updateNSView(_ view: QLPreviewView, context: Context) {
        if (view.previewItem?.previewItemURL ?? nil) != url { view.previewItem = url as NSURL }
    }
    static func dismantleNSView(_ view: QLPreviewView, coordinator: ()) {
        view.previewItem = nil
        view.close()
    }
}

struct VisitedSites {
    let all: [String]
    init(_ source: String) {
        all = source.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
    }
    var inline: String { all.first ?? "" }
    var remainingCount: Int { max(0, all.count - 1) }
}

struct VisitedSitesView: View {
    let sites: VisitedSites
    @ObservedObject var model: NotchModel
    @StateObject private var hover = SitesHoverState()
    var body: some View {
        Button {
            let type = NSApp.currentEvent?.type
            hover.activate(keyboard: type != .leftMouseUp && type != .leftMouseDown)
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "globe")
                Text(sites.inline).lineLimit(1).truncationMode(.middle)
                if sites.remainingCount > 0 { Text("+\(sites.remainingCount)").fixedSize() }
            }
            .font(.system(size: 10))
            .foregroundStyle(hover.isHovered ? Color.accentColor : Palette.secondary)
            .padding(.horizontal, 8).padding(.vertical, 4)
            .background(hover.isHovered ? Color.accentColor.opacity(0.16) : Color.primary.opacity(0.09), in: Capsule())
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Show all \(sites.all.count) visited sites")
        .background(NativePreviewHover(pointingHand: true, changed: hover.anchorHovered))
        .popover(isPresented: $hover.isPresented) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Text("Sites visited").font(.headline).padding(.bottom, 12)
                    ForEach(Array(sites.all.enumerated()), id: \.offset) { index, host in
                        if index > 0 { Divider().padding(.vertical, 10) }
                        Label(host, systemImage: "globe")
                            .font(.body).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading).padding(16)
            }
            .frame(width: 300, height: min(300, CGFloat(sites.all.count) * 42 + 58))
            .background(NativePreviewHover(changed: hover.previewHovered))
            .background(TaskPreviewWindow(model: model))
        }
        .onChange(of: hover.isPresented) { _, visible in if !visible { hover.dismiss() } }
        .padding(.leading, 10)
        .onChange(of: model.expanded) { _, visible in if !visible { hover.dismiss() } }
        .onChange(of: model.previewDismissalRevision) { _, _ in hover.dismiss() }
        .onChange(of: sites.all) { _, _ in hover.dismiss() }
        .onDisappear { hover.dismiss() }
    }
}
