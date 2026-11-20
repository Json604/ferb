import AppKit
import NotchCore

/// FERB's menu bar icon (APP-960..APP-963): the logo itself (site/logo.svg, Resources/ferb-mark.svg), drawn in code
/// from the logo's own path as a template image (black with alpha only) so macOS tints it for a light or dark menu
/// bar, its highlight and the pill-hidden status colour (APP-747). `MenuBarMark.style` picks the app icon's tile with
/// the f and dot cut out, or the bare letter with its dot after it; the dot says the state (MenuBarState.mark). The
/// stem, baseline, crossbar and top are snapped to the device pixels it is drawn at, so it is crisp at 1x and 2x.
/// Nothing animates: a still image costs the menu bar nothing between states.
enum MenuBarIcon {
    /// The image's size in points: the square a status item of squareLength centres in a 22-point bar.
    static let size = NSSize(width: 18, height: 18)
    /// The letter's opacity when FERB isn't doing anything for the owner (paused, stopped, not running).
    static let dimmedAlpha: CGFloat = 0.38

    /// The menu bar's image for a state, its accessibility description the state's line.
    static func image(for state: MenuBarState) -> NSImage { image(state.mark, description: state.statusLine) }

    static func image(_ mark: MenuBarMark, description: String,
                      style: MenuBarMark.Style = MenuBarMark.style) -> NSImage {
        let image = NSImage(size: size, flipped: false) { rect in
            let scale = NSGraphicsContext.current.map { abs($0.cgContext.userSpaceToDeviceSpaceTransform.a) } ?? 2
            draw(mark, style: style, in: rect, scale: max(1, scale))
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = description
        return image
    }

    /// The image drawn into a bitmap at a backing scale (tests and the contact sheet).
    static func bitmap(_ mark: MenuBarMark, scale: CGFloat,
                       style: MenuBarMark.Style = MenuBarMark.style) -> NSBitmapImageRep {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale),
                                   pixelsHigh: Int(size.height * scale), bitsPerSample: 8, samplesPerPixel: 4,
                                   hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0,
                                   bitsPerPixel: 0)!
        rep.size = size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        draw(mark, style: style, in: NSRect(origin: .zero, size: size), scale: scale)
        NSGraphicsContext.restoreGraphicsState()
        return rep
    }

    // MARK: - Drawing

    /// The logo in its own units (its 128-unit box, y down): the f's path as site/logo.svg writes it, its dot and the
    /// tile it sits on.
    enum Logo {
        enum Op { case move(CGFloat, CGFloat), line(CGFloat, CGFloat), quad(CGFloat, CGFloat, CGFloat, CGFloat) }
        static let f: [Op] = [
            .move(49.969, 58.772), .line(49.969, 90), .line(60.477, 90), .line(60.477, 58.772), .line(67.729, 58.772),
            .line(67.729, 51.742), .line(60.477, 51.742), .line(60.477, 49.448), .quad(60.477, 47.08, 61.402, 46.081),
            .quad(62.327, 45.082, 64.473, 45.082), .quad(66.471, 45.082, 68.321, 45.304), .line(68.321, 37.46),
            .quad(66.989, 37.386, 65.583, 37.275), .quad(64.177, 37.164, 62.771, 37.164),
            .quad(56.333, 37.164, 53.151, 40.42), .quad(49.969, 43.676, 49.969, 48.782), .line(49.969, 51.742),
            .line(43.679, 51.742), .line(43.679, 58.772),
        ]
        static let stemLeft: CGFloat = 49.969, stem: CGFloat = 10.508, baseline: CGFloat = 90
        /// Edges that land on whole pixels: the crossbar's ends and the terminal (x), the crossbar and the top (y).
        static let pixelX: Set<CGFloat> = [43.679, 67.729, 68.321]
        static let pixelY: Set<CGFloat> = [51.742, 58.772, 37.164]
        static let dot = CGPoint(x: 82, y: 85), dotRadius: CGFloat = 8
        static let tile = CGRect(x: 16, y: 16, width: 96, height: 96), tileCorner: CGFloat = 22
    }

    /// The logo placed in the canvas at a backing scale, y up: `unit` points per logo unit, the logo point `anchor` at
    /// the canvas's centre, the stem a whole number of pixels wide on whole pixels. `stemPoints` makes the stem that
    /// heavy whatever the unit (a bolder weight of the same letter): only the stem widens, everything left of it and
    /// right of it keeps the logo's proportions.
    private struct Placement {
        let scale: CGFloat, unit: CGFloat, kx: CGFloat, ox: CGFloat, oy: CGFloat, anchor: CGPoint

        init(scale: CGFloat, unit: CGFloat, anchor: CGPoint, stemPoints: CGFloat? = nil) {
            self.scale = scale
            self.unit = unit
            self.anchor = anchor
            kx = max(1, ((stemPoints ?? Logo.stem * unit) * scale).rounded()) / scale / Logo.stem
            ox = (((9 + (Logo.stemLeft - anchor.x) * unit) * scale).rounded()) / scale
            oy = (((9 - (Logo.baseline - anchor.y) * unit) * scale).rounded()) / scale
        }

        func px(_ v: CGFloat) -> CGFloat { (v * scale).rounded() / scale }
        func x(_ v: CGFloat) -> CGFloat {
            let stemRight = Logo.stemLeft + Logo.stem
            let x = v < Logo.stemLeft ? ox + (v - Logo.stemLeft) * unit
                : v <= stemRight ? ox + (v - Logo.stemLeft) * kx
                : ox + Logo.stem * kx + (v - stemRight) * unit
            return Logo.pixelX.contains(v) ? px(x) : x
        }
        func y(_ v: CGFloat) -> CGFloat {
            let y = oy + (Logo.baseline - v) * unit
            return Logo.pixelY.contains(v) ? px(y) : y
        }
        func point(_ px: CGFloat, _ py: CGFloat) -> CGPoint { CGPoint(x: x(px), y: y(py)) }

        var letter: NSBezierPath {
            let path = NSBezierPath()
            for op in Logo.f {
                switch op {
                case .move(let a, let b): path.move(to: point(a, b))
                case .line(let a, let b): path.line(to: point(a, b))
                case .quad(let cx, let cy, let a, let b): path.curve(to: point(a, b), controlPoint: point(cx, cy))
                }
            }
            path.close()
            return path
        }

        /// The tile, the logo's rounded square, on whole pixels.
        var tile: NSRect {
            let side = px(Logo.tile.width * unit)
            return NSRect(x: px(9 - (anchor.x - Logo.tile.minX) * unit), y: px(9 - (Logo.tile.maxY - anchor.y) * unit),
                          width: side, height: side)
        }
    }

    /// The bare letter, level with the bar's SF Symbols (APP-966): the headphones' ink runs from 2 to 15.75 points of
    /// this canvas centred in the bar, so the hook's top is at 15.75 (13.25 points above a baseline at 2.45) and the
    /// dot's round bottom just under 2. The f and its dot are centred across.
    private static let letterUnit: CGFloat = 13.25 / 52.836
    private static let letterBaseline: CGFloat = 2.45
    private static let letterAnchor = CGPoint(x: (43.679 + letterDotLeft + 2 * letterDotRadius) / 2,
                                              y: Logo.baseline - (9 - letterBaseline) / letterUnit)
    /// The letter's stem stays as heavy as the owner approved it at 15 points (3 points: 6 px at 2x, 3 at 1x).
    private static let letterStem: CGFloat = 3
    /// The bare letter's dot is bigger than the logo's (radius 11.5 units, not 8: about 5.5 points across) so it is
    /// what the eye finds; it keeps the logo's gap after the letter (its left edge at 74) and sits as low (bottom 93).
    private static let letterDotRadius: CGFloat = 11.5, letterDotLeft: CGFloat = 74
    /// The tile: 15 points square (room for a badge on its corner), centred.
    private static let tileUnit: CGFloat = 15 / 96
    private static let tileAnchor = CGPoint(x: 64, y: 64)

    /// The f's stem in points at a backing scale (tests read the drawn stem against it).
    static func stemWidth(_ style: MenuBarMark.Style = MenuBarMark.style, scale: CGFloat) -> CGFloat {
        placement(style, scale: scale).kx * Logo.stem
    }

    private static func placement(_ style: MenuBarMark.Style, scale: CGFloat) -> Placement {
        switch style {
        case .tile: return Placement(scale: scale, unit: tileUnit, anchor: tileAnchor)
        case .letter: return Placement(scale: scale, unit: letterUnit, anchor: letterAnchor, stemPoints: letterStem)
        }
    }

    static func draw(_ mark: MenuBarMark, style: MenuBarMark.Style = MenuBarMark.style, in rect: NSRect,
                     scale: CGFloat) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let p = placement(style, scale: scale)
        // Round shapes sit on the pixel grid: centred on a pixel's centre with an odd number of pixels across, a ring's
        // line in whole pixels inside that edge, so a 1x bar shows a clean disc and ring rather than a smudge.
        func centre(_ v: CGFloat) -> CGFloat { ((v * scale).rounded(.down) + 0.5) / scale }
        func edge(_ radius: CGFloat) -> CGFloat { (max(0, (radius * scale - 0.5).rounded()) + 0.5) / scale }
        func line(_ width: CGFloat) -> CGFloat { max(1, (width * scale).rounded()) / scale }
        let dotRadius = style == .letter ? letterDotRadius : Logo.dotRadius
        let dot = style == .letter ? CGPoint(x: letterDotLeft + letterDotRadius, y: 93 - letterDotRadius) : Logo.dot
        let r = edge(dotRadius * p.unit)
        let c = CGPoint(x: centre(p.x(dot.x)), y: centre(p.y(dot.y)))
        func disc(_ radius: CGFloat, at o: CGPoint? = nil) -> NSBezierPath {
            let o = o ?? c
            return NSBezierPath(ovalIn: NSRect(x: o.x - radius, y: o.y - radius, width: 2 * radius, height: 2 * radius))
        }
        func ring(outer: CGFloat, width: CGFloat) -> NSBezierPath {
            let w = line(width)
            let path = disc(edge(outer) - w / 2)
            path.lineWidth = w
            return path
        }
        // What stands where the dot is: filled shapes, plus a ring to stroke.
        func dotShapes() -> (fill: [NSBezierPath], stroke: [NSBezierPath]) {
            switch mark.dot {
            case .solid, .badge: return ([disc(r)], [])
            case .lit:
                // The tile's ring goes round the dot; the letter's takes the dot's own place, a touch wider (a ring
                // looks smaller than a disc of its size), with a dot in it.
                return style == .tile ? ([disc(edge(r * 0.6))], [ring(outer: r * 1.75, width: 0.9)])
                    : ([disc(edge(r * 0.45))], [ring(outer: r * 1.12, width: r * 0.36)])
            case .hollow:
                switch style {
                case .tile: return ([], [ring(outer: r + 0.3, width: mark.dimmed ? 1.3 : 0.9)])
                case .letter: return ([], [ring(outer: r * 1.12, width: r * 0.45)])
                }
            case .pause:
                let w = line(r * (style == .letter ? 0.62 : 0.75)), h = p.px(2 * r + (style == .letter ? 0 : 0.6))
                let gap = p.px(r * 0.6)
                let x0 = p.px(c.x - gap / 2 - w), y0 = p.px(c.y - h / 2)
                return ([NSBezierPath(rect: NSRect(x: x0, y: y0, width: w, height: h)),
                         NSBezierPath(rect: NSRect(x: x0 + w + gap, y: y0, width: w, height: h))], [])
            case .stop:
                let side = p.px(2 * r * (style == .letter ? 0.88 : 1))
                let x0 = p.px(c.x - side / 2), y0 = p.px(c.y - side / 2)
                return ([NSBezierPath(roundedRect: NSRect(x: x0, y: y0, width: side, height: side),
                                      xRadius: side * 0.15, yRadius: side * 0.15)], [])
            }
        }
        func paint(_ shapes: (fill: [NSBezierPath], stroke: [NSBezierPath])) {
            shapes.fill.forEach { $0.fill() }
            shapes.stroke.forEach { $0.stroke() }
        }
        NSColor.black.setFill()
        NSColor.black.setStroke()
        context.saveGState()
        switch style {
        case .tile:
            let tile = p.tile
            let corner = Logo.tileCorner * p.unit
            context.beginTransparencyLayer(auxiliaryInfo: nil)
            if mark.dimmed {
                // Quiet: the tile's outline, the f and the state's shape drawn inside it.
                let w = line(1.2)
                let outline = NSBezierPath(roundedRect: tile.insetBy(dx: w / 2, dy: w / 2),
                                           xRadius: corner - w / 2, yRadius: corner - w / 2)
                outline.lineWidth = w
                outline.stroke()
                p.letter.fill()
                paint(dotShapes())
            } else {
                // The app icon in one colour: the tile, the f and the dot cut out of it.
                NSBezierPath(roundedRect: tile, xRadius: corner, yRadius: corner).fill()
                context.setBlendMode(.clear)
                p.letter.fill()
                paint(dotShapes())
                if mark.dot == .badge {
                    let badge = badgeCentre(tile: tile, p: p)
                    disc(badgeRadius(p) + 0.9, at: badge).fill()
                    context.setBlendMode(.normal)
                    disc(badgeRadius(p), at: badge).fill()
                }
                context.setBlendMode(.normal)
            }
            context.endTransparencyLayer()
        case .letter:
            context.setAlpha(mark.dimmed ? dimmedAlpha : 1)
            p.letter.fill()
            context.setAlpha(1)
            if mark.dot == .badge {
                // The dot, as big, rises to the hook's height, still to the right of the letter.
                disc(r, at: CGPoint(x: c.x, y: centre(p.oy + 41 * p.unit))).fill()
            } else {
                paint(dotShapes())
            }
        }
        context.restoreGState()
    }

    private static func badgeRadius(_ p: Placement) -> CGFloat { (2.3 * p.scale).rounded(.down) / p.scale }

    /// The tile's badge: on its top-right corner, inside the canvas.
    private static func badgeCentre(tile: NSRect, p: Placement) -> CGPoint {
        let r = badgeRadius(p)
        func down(_ v: CGFloat) -> CGFloat { (v * p.scale).rounded(.down) / p.scale }
        return CGPoint(x: down(min(tile.maxX - 1.2, 17.5 - r)), y: down(min(tile.maxY - 1.2, 17.5 - r)))
    }
}
