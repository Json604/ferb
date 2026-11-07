import AppKit
import OSLog
import SwiftUI

/// FERB's glass blurs what is behind it the way the owner's reference does (APP-785, owner 2026-11-07, tried locally
/// first): a heavy smear at a surface's outer edge, so what is under it spreads into colour, fading to a lighter blur
/// toward its content. Small surfaces (the composer, pills, bubbles) get the heavy smear evenly.
///
/// macOS draws an NSVisualEffectView's blur with a backdrop layer's `gaussianBlur` filter (radius 30 for the materials
/// FERB uses). This puts the private `variableBlur` filter in its place, with a mask whose white is the full radius.
/// Resolved defensively, like FERB's other private calls (APP-563): when the layer or the filter isn't there, or Reduce
/// Transparency is on, the view keeps macOS's own frost.
enum ProgressiveBlur {
    static let log = Logger(subsystem: "com.ferb.FERB", category: "blur")
    /// The smear at the outer edge, in points (macOS's frost is 30).
    static let edgeRadius: CGFloat = 64
    /// The blur toward the content, as a share of the edge's: still frosted there (APP-551), never clear.
    static let innerShare: CGFloat = 0.28
    /// The even smear of a small surface.
    static let evenRadius: CGFloat = 48

    /// How strong the blur is at `t` (0 at the outer edge, 1 at the far edge): full at the edge, easing down to
    /// `innerShare` by the far edge (a smoothstep, so there is no visible band where it changes).
    static func strength(at t: CGFloat, innerShare: CGFloat = innerShare) -> CGFloat {
        let x = min(1, max(0, t))
        let eased = x * x * (3 - 2 * x)
        return 1 - (1 - innerShare) * eased
    }

    /// The mask the filter stretches over the layer: white (full radius) at the top, `strength` below. Top-down rows.
    static func mask(rows: Int = 128) -> CGImage? {
        let width = 4
        var bytes = [UInt8](repeating: 0, count: width * rows)
        for row in 0..<rows {
            let value = UInt8((strength(at: CGFloat(row) / CGFloat(max(1, rows - 1))) * 255).rounded())
            for column in 0..<width { bytes[row * width + column] = value }
        }
        guard let provider = CGDataProvider(data: Data(bytes) as CFData) else { return nil }
        return CGImage(width: width, height: rows, bitsPerComponent: 8, bitsPerPixel: 8, bytesPerRow: width,
                       space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGBitmapInfo(rawValue: 0),
                       provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    @MainActor private static var cachedMask: CGImage?
    @MainActor private static var unavailable = false

    /// A new `variableBlur` filter, or nil where macOS has none.
    @MainActor static func filter(radius: CGFloat, mask: CGImage?) -> NSObject? {
        guard !unavailable, let type = NSClassFromString("CAFilter") as? NSObject.Type,
              type.responds(to: NSSelectorFromString("filterWithType:")),
              let made = type.perform(NSSelectorFromString("filterWithType:"), with: "variableBlur")?
                .takeUnretainedValue() as? NSObject else {
            if !unavailable { log.info("variable blur unavailable: macOS's frost stays") }
            unavailable = true
            return nil
        }
        made.setValue(radius, forKey: "inputRadius")
        if let mask { made.setValue(mask, forKey: "inputMaskImage") }
        made.setValue(true, forKey: "inputNormalizeEdges")
        return made
    }

    /// Puts the progressive (`even` false) or even blur on `view`'s backdrop layer in place of macOS's gaussian blur,
    /// keeping its other filters (colour saturation). Again after macOS rebuilt them. True when it is on.
    @MainActor @discardableResult
    static func apply(to view: NSVisualEffectView, even: Bool) -> Bool {
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency,
              let backdrop = backdropLayer(in: view.layer) else { return false }
        var filters = backdrop.filters ?? []
        if filters.contains(where: { isOurs($0) }) { return true }
        guard let index = filters.firstIndex(where: { String(describing: $0) == "gaussianBlur" }) else { return false }
        if cachedMask == nil { cachedMask = mask() }
        // even: an all-white mask, one radius over the whole surface
        guard let blur = filter(radius: even ? evenRadius : edgeRadius, mask: even ? evenMask : cachedMask) else {
            return false
        }
        blur.setValue(true, forKey: "ferbProgressive")
        filters[index] = blur
        backdrop.filters = filters
        return true
    }

    @MainActor private static let evenMask: CGImage? = {
        guard let provider = CGDataProvider(data: Data([UInt8](repeating: 255, count: 16)) as CFData) else { return nil }
        return CGImage(width: 4, height: 4, bitsPerComponent: 8, bitsPerPixel: 8, bytesPerRow: 4,
                       space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGBitmapInfo(rawValue: 0),
                       provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }()

    private static func isOurs(_ filter: Any) -> Bool {
        (filter as? NSObject)?.value(forKey: "ferbProgressive") as? Bool == true
    }

    /// The backdrop layer macOS blurs through (a CABackdropLayer), anywhere under `layer`.
    static func backdropLayer(in layer: CALayer?) -> CALayer? {
        guard let layer else { return nil }
        if String(describing: type(of: layer)).contains("Backdrop") { return layer }
        for sub in layer.sublayers ?? [] { if let found = backdropLayer(in: sub) { return found } }
        return nil
    }
}

/// An NSVisualEffectView with FERB's blur (`ProgressiveBlur`): applied once its backdrop exists and again whenever
/// macOS rebuilds it (layout, window, appearance, Reduce Transparency changes).
class FerbGlassView: NSVisualEffectView {
    var even = false { didSet { if oldValue != even { refresh() } } }

    override func layout() {
        super.layout()
        refresh()
    }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        refresh()
    }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        refresh()
    }
    override func updateLayer() {
        super.updateLayer()
        refresh()
    }

    func refresh() {
        guard window != nil else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        ProgressiveBlur.apply(to: self, even: even)
        CATransaction.commit()
    }
}

/// FERB's glass as a SwiftUI background: behind the window (a whole surface) or within it (a pill over FERB's own
/// glass, as SwiftUI's materials blur), progressive or even.
struct FerbGlass: NSViewRepresentable {
    var material: NSVisualEffectView.Material = .hudWindow
    var blending: NSVisualEffectView.BlendingMode = .withinWindow
    var even = true

    func makeNSView(context: Context) -> FerbGlassView {
        let view = FerbGlassView()
        view.material = material
        view.blendingMode = blending
        view.state = .active
        view.even = even
        return view
    }
    func updateNSView(_ view: FerbGlassView, context: Context) {
        if view.material != material { view.material = material }
        if view.blendingMode != blending { view.blendingMode = blending }
        view.even = even
        view.refresh()
    }
}

extension View {
    /// FERB's glass behind a small surface in `shape` (APP-785): the even heavy smear in place of SwiftUI's
    /// material, within FERB's window as the material was.
    func ferbGlass<S: Shape>(in shape: S, material: NSVisualEffectView.Material = .hudWindow) -> some View {
        background { FerbGlass(material: material).clipShape(shape) }
    }
}
