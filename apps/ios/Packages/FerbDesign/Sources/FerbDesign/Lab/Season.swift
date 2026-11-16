// Ported from ~/Desktop/ferb-glass-lab Sources/FerbGlassLab/Weather.swift at 23a0fde (LabSource.commit): the season
// layer only. Weather is paused (kb PHN-412), so WeatherLayer is SeasonLayer, drawn as the lab draws it under a clear
// sky: autumn's leaves, spring's petals, the fireflies of a summer or monsoon night, nothing otherwise. Removed: the
// weather (rain, drops on the glass, snow and its crystals, fog, storms and lightning, wind's gusts, `weathered`).
// Season.at is in SkyMoment.swift. Changed: the time comes from the motion clock (Motion.swift) instead of the lab's
// `animate` switch, which drew t = 1000 when off. Every number and colour inside is the lab's.

import SwiftUI

// Weather and seasons on the phone: the sky dulls under cloud, fog and storm, and a light layer of rain, snow,
// leaves, petals or fireflies drifts over the home and chat screens. All slow and sparse; off when the owner turns
// animations off (and, in the app, under Reduce Motion).

/// What drifts over the screen: the weather when there is some, else the season's own.
public struct SeasonLayer: View {
    let season: String
    let night: Bool

    public init(season: Season, night: Bool) {
        self.season = season.rawValue
        self.night = night
    }

    /// True when the season draws anything at this hour (autumn, spring, or a summer or monsoon night).
    public static func hasLayer(_ season: Season, night: Bool) -> Bool {
        switch season {
        case .autumn, .spring: return true
        case .summer, .monsoon: return night
        case .winter: return false
        }
    }

    public var body: some View {
        MotionClock { t in
            Canvas { g, size in draw(g, size, t) }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    func r(_ i: Int, _ k: Int) -> Double {
        let x = sin(Double(i) * 12.9898 + Double(k) * 78.233) * 43758.5453
        return x - floor(x)
    }

    /// Things are drawn at three depths, as a camera sees them: far (small, slow, a little soft), the plane in focus,
    /// and near (large, fast, out of focus).
    private enum Depth: CaseIterable {
        case far, mid, near
        var scale: Double { self == .far ? 0.5 : self == .mid ? 0.85 : 1.35 }
        /// Only the far layer is softened, and barely: a whole-layer blur on near shapes smudges them rather than
        /// looking like a lens. Near snow is drawn as soft discs instead.
        var blur: Double { self == .far ? 0.6 : 0 }
        var alpha: Double { self == .far ? 0.65 : self == .mid ? 1 : 0.8 }
    }

    private func layer(_ g: GraphicsContext, _ d: Depth, _ body: (inout GraphicsContext) -> Void) {
        g.drawLayer { l in
            if d.blur > 0 { l.addFilter(.blur(radius: d.blur)) }
            l.opacity = d.alpha
            body(&l)
        }
    }

    private func draw(_ g: GraphicsContext, _ size: CGSize, _ t: Double) {
        let W = size.width, H = size.height
        seasonal(g, W, H, t, windy: false)
    }

    // MARK: Leaves, petals, fireflies

    private func seasonal(_ g: GraphicsContext, _ W: Double, _ H: Double, _ t: Double, windy: Bool) {
        let gust = windy ? 2.6 : 1
        switch season {
        case "autumn":
            let tones: [(UInt32, UInt32, UInt32)] = [(0xE39A3F, 0xB5532A, 0x6B2F14), (0xEDC155, 0xC48A2E, 0x7A5218),
                                                     (0xCF5E31, 0x93321B, 0x4E1A0C), (0xD9A04A, 0x9D5E28, 0x5A3312),
                                                     (0xB9472C, 0x7C2616, 0x3F120A), (0xC9A64A, 0x8E7A2C, 0x4F4517)]
            for (k, d) in Depth.allCases.enumerated() {
                layer(g, d) { l in
                    for j in 0..<[5, 6, 1][k] {
                        let i = j + k * 50
                        fall(l, i, W, H, t, gust, d, kind: i % 3 == 0 ? .maple : .oval, tone: tones[i % tones.count], size: 26 + 8 * r(i, 3))
                    }
                }
            }
        case "spring":
            for (k, d) in Depth.allCases.enumerated() {
                layer(g, d) { l in
                    for j in 0..<[7, 8, 1][k] {
                        let i = j + k * 50
                        fall(l, i, W, H, t, gust, d, kind: .petal, tone: (0xFFF6F8, 0xF9D0DA, 0xE58CA5), size: 15 + 4 * r(i, 3))
                    }
                }
            }
        case "summer" where night, "monsoon" where night:
            for (k, d) in Depth.allCases.enumerated() {
                layer(g, d) { l in
                    for j in 0..<[7, 6, 2][k] { firefly(l, j + k * 50, W, H, t, d.scale) }
                }
            }
        default:
            break
        }
    }

    private enum Shape { case oval, maple, petal }

    /// How a leaf really falls: it swings from side to side like a pendulum, gliding down fastest through the middle
    /// of each swing and almost stopping at the ends, tilting into the swing, and turning over now and then.
    private func fall(_ g: GraphicsContext, _ i: Int, _ W: Double, _ H: Double, _ t: Double, _ gust: Double, _ d: Depth,
                      kind: Shape, tone: (UInt32, UInt32, UInt32), size: Double) {
        let petal = kind == .petal
        let period = (petal ? 3.4 : 2.9) + 1.4 * r(i, 11)
        let s = t / period * 2 * .pi + r(i, 12) * 6
        let amp = (petal ? 46 : 38) * d.scale * (0.7 + 0.6 * r(i, 13))
        let v = (petal ? 22 : 30) * d.scale * (0.8 + 0.4 * r(i, 2))
        let span = H + 120
        let yRaw = r(i, 4) * span + v * t + v * period / (2 * .pi) * 0.45 * sin(2 * s)
        let y = yRaw.truncatingRemainder(dividingBy: span) - 60
        let xRaw = r(i, 1) * W + amp * sin(s) + t * (petal ? 9 : 3) * gust * d.scale
        let x = (xRaw.truncatingRemainder(dividingBy: W + 80) + W + 80).truncatingRemainder(dividingBy: W + 80) - 40
        let tilt = 0.75 * cos(s) + r(i, 7) * 6.28 + sin(t * 0.13 + r(i, 14) * 6) * 0.6
        let turn = cos(t * (0.55 + 0.5 * r(i, 8)) + r(i, 9) * 6)
        var l = g
        l.translateBy(x: x, y: y)
        l.rotate(by: .radians(tilt))
        l.scaleBy(x: 1, y: 0.55 + 0.45 * abs(turn))
        // The side facing the light is brighter; the underside duller.
        let lit = turn > 0 ? 1.0 : 0.78
        let sz = size * d.scale
        switch kind {
        case .oval: ovalLeaf(l, sz, tone, lit, seed: i)
        case .maple: mapleLeaf(l, sz, tone, lit, seed: i)
        case .petal: blossom(l, sz, tone, lit)
        }
    }

    /// A leaf as a tree drops it: a rounded base, widest a third of the way up, narrowing to a soft point, a little
    /// lopsided and bent along its midrib, with a finely toothed edge, patchy colour, a browned rim, pale veins and a
    /// short stalk that tapers from the base.
    private func ovalLeaf(_ l: GraphicsContext, _ L: Double, _ tone: (UInt32, UInt32, UInt32), _ lit: Double, seed: Int) {
        let w = L * (0.3 + 0.06 * r(seed, 15))
        let bend = L * 0.06 * (r(seed, 16) - 0.5) * 2
        let n = 40
        func edge(_ u: Double, _ side: Double) -> CGPoint {
            // u runs from the base (0) to the tip (1); the half-width is round at the base and tapers to the tip
            let profile = pow(sin(.pi * pow(u, 0.62)), 0.9) * (1 - 0.18 * u)
            let half = w * profile * (side > 0 ? 1 : 0.9)
            let tooth = 1 + 0.035 * (k(u) % 2 == 0 ? 1 : -1) * min(1, u * 3) * (1 - u)
            return CGPoint(x: -L / 2 + L * u, y: side * half * tooth + bend * sin(.pi * u))
        }
        func k(_ u: Double) -> Int { Int(u * Double(n)) }
        var leaf = Path()
        leaf.move(to: CGPoint(x: -L / 2, y: 0))
        for i in 1...n { leaf.addLine(to: edge(Double(i) / Double(n), -1)) }
        for i in stride(from: n - 1, through: 0, by: -1) { leaf.addLine(to: edge(Double(i) / Double(n), 1)) }
        leaf.closeSubpath()
        // the stalk: short, thick where it joins, under the leaf
        var stalk = Path()
        stalk.move(to: CGPoint(x: -L / 2 + 1, y: -1.1))
        stalk.addQuadCurve(to: CGPoint(x: -L * 0.64, y: bend * 0.6), control: CGPoint(x: -L * 0.58, y: -0.6))
        stalk.addQuadCurve(to: CGPoint(x: -L / 2 + 1, y: 1.1), control: CGPoint(x: -L * 0.58, y: 1.4))
        stalk.closeSubpath()
        l.fill(stalk, with: .color(Color(hex: tone.2)))
        paintLeaf(l, leaf, tone, lit, span: L, seed: seed)
        var veins = Path()
        veins.move(to: CGPoint(x: -L / 2, y: 0))
        veins.addQuadCurve(to: CGPoint(x: L * 0.46, y: bend * 0.2), control: CGPoint(x: 0, y: bend * 1.2))
        for f in [-0.32, -0.16, 0.0, 0.16, 0.3] {
            for side in [-1.0, 1.0] {
                let from = CGPoint(x: L * f, y: bend * sin(.pi * (f + 0.5)))
                veins.move(to: from)
                veins.addQuadCurve(to: CGPoint(x: L * (f + 0.14), y: from.y + side * w * 0.75),
                                   control: CGPoint(x: L * (f + 0.03), y: from.y + side * w * 0.45))
            }
        }
        var inside = l
        inside.clip(to: leaf)
        inside.stroke(veins, with: .color(Color(hex: tone.0).opacity(0.5 * lit)), style: StrokeStyle(lineWidth: 0.55, lineCap: .round))
    }

    /// A maple leaf: five toothed lobes, veins from the stem to each lobe.
    private func mapleLeaf(_ l: GraphicsContext, _ s: Double, _ tone: (UInt32, UInt32, UInt32), _ lit: Double, seed: Int) {
        let R = s * 0.55
        let lobes: [(Double, Double)] = [(0, 1.0), (58, 0.92), (-58, 0.92), (112, 0.66), (-112, 0.66)]
        var pts: [CGPoint] = []
        let n = 90
        for k in 0..<n {
            let deg = -180 + 360 * Double(k) / Double(n)
            if abs(deg) > 168 { pts.append(CGPoint(x: 0, y: R * 0.12)); continue }   // the notch where the stem joins
            var reach = 0.46
            for (c, len) in lobes {
                let d = abs(deg - c)
                reach = max(reach, len * max(0, 1 - pow(d / 38, 2.2)))
            }
            reach *= 1 + 0.06 * (k % 2 == 0 ? 1 : -1)                                   // the teeth
            let a = (deg - 90) * .pi / 180
            pts.append(CGPoint(x: cos(a) * R * reach, y: sin(a) * R * reach))
        }
        var leaf = Path()
        leaf.addLines(pts)
        leaf.closeSubpath()
        paintLeaf(l, leaf, tone, lit, span: s, seed: seed)
        var veins = Path()
        for (deg, len) in lobes {
            let a = (deg - 90) * .pi / 180
            veins.move(to: CGPoint(x: 0, y: R * 0.1))
            veins.addLine(to: CGPoint(x: cos(a) * R * len * 0.85, y: sin(a) * R * len * 0.85))
        }
        var inside = l
        inside.clip(to: leaf)
        inside.stroke(veins, with: .color(Color(hex: tone.0).opacity(0.55 * lit)), style: StrokeStyle(lineWidth: 0.6, lineCap: .round))
        var stalk = Path()
        stalk.move(to: CGPoint(x: -1.1, y: R * 0.08))
        stalk.addQuadCurve(to: CGPoint(x: R * 0.05, y: R * 0.42), control: CGPoint(x: -0.6, y: R * 0.3))
        stalk.addQuadCurve(to: CGPoint(x: 1.1, y: R * 0.08), control: CGPoint(x: 1.4, y: R * 0.3))
        stalk.closeSubpath()
        l.fill(stalk, with: .color(Color(hex: tone.2)))
    }

    /// A leaf's colour: a light-to-dark fade across it, a couple of soft patches, and a browned rim.
    private func paintLeaf(_ l: GraphicsContext, _ leaf: Path, _ tone: (UInt32, UInt32, UInt32), _ lit: Double, span: Double, seed: Int) {
        let light = Color(hex: tone.0), mid = Color(hex: tone.1), dark = Color(hex: tone.2)
        l.fill(leaf, with: .linearGradient(Gradient(colors: [light, mid, dark]),
                                           startPoint: CGPoint(x: -span / 2, y: -span / 3), endPoint: CGPoint(x: span / 2, y: span / 3)))
        var inside = l
        inside.clip(to: leaf)
        for k in 0..<2 {
            let c = CGPoint(x: (r(seed, 40 + k) - 0.5) * span * 0.7, y: (r(seed, 42 + k) - 0.5) * span * 0.3)
            let rad = span * (0.12 + 0.12 * r(seed, 44 + k))
            inside.fill(Path(ellipseIn: CGRect(x: c.x - rad, y: c.y - rad, width: rad * 2, height: rad * 2)),
                        with: .radialGradient(Gradient(colors: [(k == 0 ? dark : light).opacity(0.45), .clear]),
                                              center: c, startRadius: 0, endRadius: rad))
        }
        inside.stroke(leaf, with: .color(dark.opacity(0.6)), lineWidth: span * 0.06)
        if lit < 1 { inside.fill(leaf, with: .color(.black.opacity(1 - lit))) }
    }

    /// A cherry blossom petal: cupped, notched at the tip, near white at the base and pinker outward, with fine lines.
    private func blossom(_ l: GraphicsContext, _ s: Double, _ tone: (UInt32, UInt32, UInt32), _ lit: Double) {
        var p = Path()
        p.move(to: CGPoint(x: 0, y: s * 0.5))
        p.addCurve(to: CGPoint(x: -s * 0.46, y: -s * 0.12), control1: CGPoint(x: -s * 0.3, y: s * 0.42), control2: CGPoint(x: -s * 0.5, y: s * 0.16))
        p.addCurve(to: CGPoint(x: -s * 0.07, y: -s * 0.5), control1: CGPoint(x: -s * 0.44, y: -s * 0.42), control2: CGPoint(x: -s * 0.2, y: -s * 0.52))
        p.addQuadCurve(to: CGPoint(x: s * 0.07, y: -s * 0.5), control: CGPoint(x: 0, y: -s * 0.36))
        p.addCurve(to: CGPoint(x: s * 0.46, y: -s * 0.12), control1: CGPoint(x: s * 0.2, y: -s * 0.52), control2: CGPoint(x: s * 0.44, y: -s * 0.42))
        p.addCurve(to: CGPoint(x: 0, y: s * 0.5), control1: CGPoint(x: s * 0.5, y: s * 0.16), control2: CGPoint(x: s * 0.3, y: s * 0.42))
        p.closeSubpath()
        l.fill(p, with: .radialGradient(Gradient(colors: [Color(hex: tone.0), Color(hex: tone.1), Color(hex: tone.2)]),
                                        center: CGPoint(x: 0, y: s * 0.55), startRadius: 0, endRadius: s * 1.05))
        var inside = l
        inside.clip(to: p)
        // the cup: a soft shadow down one side, a sheen down the other
        inside.fill(Path(ellipseIn: CGRect(x: -s * 0.6, y: -s * 0.5, width: s * 0.5, height: s)), with: .color(Color(hex: tone.2).opacity(0.18)))
        inside.fill(Path(ellipseIn: CGRect(x: s * 0.05, y: -s * 0.35, width: s * 0.22, height: s * 0.6)), with: .color(.white.opacity(0.35)))
        var lines = Path()
        for f in [-0.18, 0.0, 0.18] {
            lines.move(to: CGPoint(x: 0, y: s * 0.45))
            lines.addQuadCurve(to: CGPoint(x: s * f * 1.6, y: -s * 0.3), control: CGPoint(x: s * f, y: s * 0.05))
        }
        inside.stroke(lines, with: .color(Color(hex: tone.2).opacity(0.25)), lineWidth: 0.4)
        if lit < 1 { inside.fill(p, with: .color(.black.opacity((1 - lit) * 0.6))) }
    }

    /// A firefly: a dark body and a glowing tail that brightens and fades, wandering slowly.
    private func firefly(_ g: GraphicsContext, _ i: Int, _ W: Double, _ H: Double, _ t: Double, _ scale: Double) {
        let x = r(i, 1) * W + sin(t * 0.3 + r(i, 5) * 7) * 30 + sin(t * 1.1 + r(i, 8) * 4) * 4
        let y = H * (0.4 + 0.55 * r(i, 4)) + cos(t * 0.25 + r(i, 6) * 5) * 24
        let glow = pow((sin(t * 1.3 + r(i, 7) * 9) + 1) / 2, 2)
        let warm = Color(hex: 0xFFE58A)
        let halo = 14 * scale
        g.fill(Path(ellipseIn: CGRect(x: x - halo, y: y - halo, width: halo * 2, height: halo * 2)),
               with: .radialGradient(Gradient(colors: [warm.opacity(0.35 * glow), warm.opacity(0)]), center: CGPoint(x: x, y: y), startRadius: 0, endRadius: halo))
        g.fill(Path(ellipseIn: CGRect(x: x - 2.2 * scale, y: y - 1.4 * scale, width: 4.4 * scale, height: 2.8 * scale)), with: .color(warm.opacity(0.35 + 0.65 * glow)))
        g.fill(Path(ellipseIn: CGRect(x: x - 4.2 * scale, y: y - 1.1 * scale, width: 2.6 * scale, height: 2.2 * scale)), with: .color(Color(hex: 0x2A2418).opacity(0.8)))
    }
}
