// Copied from ~/Desktop/ferb-glass-lab Sources/FerbGlass/Sky.swift at 23a0fde (LabSource.commit), unchanged: the sun,
// sunrise and sunset, the moon, the lab's skies, the owner's chosen sky (ChosenSky), the sun dot, the moon and the
// fixed stars. The phone's own additions (the place without a location, one moment of the sky) are in SkyMoment.swift.

import Foundation
import SwiftUI

// MARK: Where the sun and the moon really are

/// The sun's place in the sky for a time and a place, from the low-precision formulas of the U.S. Naval Observatory
/// and NOAA (good to about a tenth of a degree, plenty for a picture). Pure arithmetic: it runs on the phone, needs no
/// network, and the location it is given never leaves the device.
public struct SunPosition: Equatable {
    /// Degrees above the horizon (negative: below it).
    public let elevation: Double
    /// Degrees clockwise from north (90 east, 180 south, 270 west).
    public let azimuth: Double

    public init(date: Date, latitude: Double, longitude: Double) {
        let rad = Double.pi / 180
        let n = date.timeIntervalSince1970 / 86400 + 2440587.5 - 2451545.0
        let meanLongitude = (280.460 + 0.9856474 * n).truncatingRemainder(dividingBy: 360)
        let anomaly = (357.528 + 0.9856003 * n).truncatingRemainder(dividingBy: 360) * rad
        let eclipticLongitude = (meanLongitude + 1.915 * sin(anomaly) + 0.020 * sin(2 * anomaly)) * rad
        let obliquity = (23.439 - 0.0000004 * n) * rad
        let rightAscension = atan2(cos(obliquity) * sin(eclipticLongitude), cos(eclipticLongitude))
        let declination = asin(sin(obliquity) * sin(eclipticLongitude))
        let siderealHours = (18.697374558 + 24.06570982441908 * n).truncatingRemainder(dividingBy: 24)
        let hourAngle = (siderealHours * 15 + longitude) * rad - rightAscension
        let lat = latitude * rad
        let altitude = asin(sin(lat) * sin(declination) + cos(lat) * cos(declination) * cos(hourAngle))
        var azimuth = atan2(-sin(hourAngle), tan(declination) * cos(lat) - sin(lat) * cos(hourAngle)) / rad
        if azimuth < 0 { azimuth += 360 }
        self.elevation = altitude / rad
        self.azimuth = azimuth
    }
}

/// Today's sunrise and sunset, found by walking the day in minutes (the sun's disc touching the horizon, -0.833°).
public struct SunDay {
    public let sunrise: Date?
    public let sunset: Date?

    public init(around date: Date, latitude: Double, longitude: Double, calendar: Calendar = .current) {
        let start = calendar.startOfDay(for: date)
        var rise: Date?, set: Date?
        var before = SunPosition(date: start, latitude: latitude, longitude: longitude).elevation
        for minute in stride(from: 5, through: 1440, by: 5) {
            let t = start.addingTimeInterval(Double(minute) * 60)
            let now = SunPosition(date: t, latitude: latitude, longitude: longitude).elevation
            if before < -0.833, now >= -0.833, rise == nil { rise = t }
            if before >= -0.833, now < -0.833, set == nil { set = t }
            before = now
        }
        sunrise = rise
        sunset = set
    }
}

/// How much of the moon is lit and whether it is growing, from its age in the 29.53-day cycle.
public struct MoonPhase {
    /// 0 new, 0.5 full, back to 1.
    public let age: Double
    public var lit: Double { (1 - cos(age * 2 * .pi)) / 2 }
    public var waxing: Bool { age < 0.5 }

    public init(date: Date) {
        let days = date.timeIntervalSince1970 / 86400 + 2440587.5 - 2451550.1
        let cycles = days / 29.530588853
        age = cycles - floor(cycles)
    }
}

// MARK: The sky

/// The sky over the owner right now. Its colour comes from the sun's real height where the owner is; it has no
/// horizon line and no ground, only light that gets warmer toward the bottom on the sun's side. Changing `date`
/// inside an animation glides through every sky in between (dusk passes through its purples rather than cutting
/// to night). The sun is FERB's amber dot, small and exactly where the sun is; at night the moon shows tonight's
/// phase over a few fixed stars. The one place in FERB where colour is a picture rather than a lamp, because it is a
/// picture of something real.
public struct SkyView: View {
    public var date: Date
    public var latitude: Double
    public var longitude: Double
    /// The sun and the moon; off where the sky is only a band behind other things.
    public var showSun: Bool

    public init(date: Date, latitude: Double, longitude: Double, showSun: Bool = true) {
        self.date = date
        self.latitude = latitude
        self.longitude = longitude
        self.showSun = showSun
    }

    public var body: some View {
        let sun = SunPosition(date: date, latitude: latitude, longitude: longitude)
        SkyPaint(elevation: sun.elevation, across: Self.across(azimuth: sun.azimuth, latitude: latitude),
                 moon: MoonPhase(date: date), evening: sun.azimuth > 180, showSun: showSun)
    }

    /// Facing the equator, the sun rises on the left and sets on the right: -1 far left, 0 the middle, 1 far right.
    public static func across(azimuth: Double, latitude: Double) -> Double {
        var offset = azimuth - (latitude >= 0 ? 180 : 0)
        if offset > 180 { offset -= 360 }
        if offset < -180 { offset += 360 }
        return max(-1, min(1, (latitude >= 0 ? offset : -offset) / 120))
    }
}

/// The painting itself, animatable by the sun's height and place so a change of time glides.
struct SkyPaint: View, Animatable {
    var elevation: Double
    var across: Double
    let moon: MoonPhase
    let evening: Bool
    let showSun: Bool
    /// The owner's chosen sky; off only on the choice table, which shows the lab's own sky alone.
    var chosen = true

    var animatableData: AnimatablePair<Double, Double> {
        get { AnimatablePair(elevation, across) }
        set { elevation = newValue.first; across = newValue.second }
    }

    var body: some View {
        let look = chosen ? ChosenSky.look(elevation: elevation, across: across)
            : SkyFrameLook(mine: SkyLook(elevation: elevation, evening: evening), elevation: elevation)
        GeometryReader { geo in
            let w = geo.size.width, h = geo.size.height
            let sunX = w * CGFloat(0.5 + across * 0.42)
            let sunY = h * (0.94 - CGFloat(max(elevation, -3) / 70) * 0.82)
            ZStack {
                Canvas(opaque: true, colorMode: .nonLinear) { g, size in
                    look.paint(in: g, size: size, sunX: sunX)
                }
                Stars(count: 38, seed: 7)
                    .opacity(min(1, max(0, (-elevation - 7) / 8)))
                    .mask(LinearGradient(colors: [.white, .white.opacity(0)], startPoint: .top, endPoint: UnitPoint(x: 0.5, y: 0.65)))
                if showSun, elevation > -2 {
                    SunDisc(elevation: elevation).position(x: sunX, y: sunY)
                        .opacity(min(1, (elevation + 2) / 3))
                }
                if showSun, elevation < -5 {
                    MoonDisc(phase: moon, size: 15)
                        .position(x: w * 0.8, y: h * 0.17)
                        .opacity(min(1, (-elevation - 5) / 6))
                }
            }
            .drawingGroup()
        }
        .clipped()
    }
}

// MARK: The sky the owner chose

/// The owner picked, for each time of day, the lab's sky or Astra's (sky/choices.json, 2026-11-14). The app's sky
/// follows the real sun through those picks: by the sun's height, rising on one side of noon and setting on the
/// other. Between two of the lab's own skies it is the lab's sky itself; between two picks of different origin, the
/// two are blended in OKLab with a smooth step, so the sky glides.
enum ChosenSky {
    enum Pick { case mine(Double), astra(FerbSkyPhase) }

    /// The owner's picks, by the sun's height. Morning: night, dawn, sunrise, morning, noon. Evening: noon, golden
    /// hour, sunset, dusk, night.
    static let rising: [(Double, Pick)] = [(-90, .mine(-90)), (-14, .mine(-14)), (-4.5, .mine(-4.5)), (0.5, .astra(.sunrise)),
                                           (15, .mine(15)), (40, .astra(.noon)), (90, .astra(.noon))]
    static let setting: [(Double, Pick)] = [(-90, .mine(-90)), (-14, .mine(-14)), (-5, .astra(.dusk)), (0, .astra(.sunset)),
                                            (7, .astra(.goldenHour)), (40, .astra(.noon)), (90, .astra(.noon))]

    /// `across` is where the sun is across the sky: below zero it is rising, above zero setting. Around midday the
    /// rising and setting skies are blended, so a place where the sun never climbs high (London in winter) has no
    /// seam at noon. At midnight both sides are the same night.
    static func look(elevation e: Double, across: Double) -> SkyFrameLook {
        let w = smoother((across + 0.14) / 0.28)
        if w <= 0 { return side(e, evening: false) }
        if w >= 1 { return side(e, evening: true) }
        return side(e, evening: false).mixed(side(e, evening: true), w)
    }

    private static func side(_ e: Double, evening: Bool) -> SkyFrameLook {
        let anchors = evening ? setting : rising
        var i = 0
        while i < anchors.count - 2, e > anchors[i + 1].0 { i += 1 }
        let (ea, a) = anchors[i], (eb, b) = anchors[i + 1]
        if case .mine = a, case .mine = b { return SkyFrameLook(mine: SkyLook(elevation: e, evening: evening), elevation: e) }
        return frame(a, evening).mixed(frame(b, evening), smoother((e - ea) / (eb - ea)))
    }

    /// A smoother step (no jolt in the speed of change at either end), so one sky eases into the next.
    static func smoother(_ x: Double) -> Double {
        let t = max(0, min(1, x))
        return t * t * t * (t * (t * 6 - 15) + 10)
    }

    private static func frame(_ pick: Pick, _ evening: Bool) -> SkyFrameLook {
        switch pick {
        case .mine(let e): return SkyFrameLook(mine: SkyLook(elevation: e, evening: evening), elevation: e)
        case .astra(let phase): return SkyFrameLook(astra: FerbSkyPalettes.fixed(phase))
        }
    }
}

/// One sky in a form both kinds share: colours down the screen on a fine grid, Astra's three soft fields, and the
/// lab's glow on the sun's side.
public struct SkyFrameLook {
    static let grid: [Double] = (0...24).map { Double($0) / 24 }
    var colors: [OKLab]
    var fields: [SkyField]
    var sunGlow: OKLab
    var sunGlowStrength: Double

    /// How light the top and the middle are (OKLab L): what decides black or white text over them.
    public var lightTop: Double { colors[0...6].map(\.l).reduce(0, +) / 7 }
    public var lightLow: Double { colors[7...15].map(\.l).reduce(0, +) / 9 }

    public init(date: Date, latitude: Double, longitude: Double) {
        let sun = SunPosition(date: date, latitude: latitude, longitude: longitude)
        self = ChosenSky.look(elevation: sun.elevation, across: SkyView.across(azimuth: sun.azimuth, latitude: latitude))
    }

    init(colors: [OKLab], fields: [SkyField], sunGlow: OKLab, sunGlowStrength: Double) {
        self.colors = colors; self.fields = fields; self.sunGlow = sunGlow; self.sunGlowStrength = sunGlowStrength
    }

    /// The lab's sky: its stops, sampled the way SwiftUI blends between them (in sRGB).
    init(mine look: SkyLook, elevation: Double) {
        let stops = look.rgbStops
        colors = SkyFrameLook.grid.map { SkyFrameLook.sample(stops, at: $0) }
        fields = []
        sunGlow = look.glowLab
        sunGlowStrength = max(0, 0.8 - abs(elevation - 1) / 14)
    }

    /// Astra's sky: its seven stops sampled the same way, and its fields.
    init(astra p: SkyPalette) {
        let stops = zip(FerbSkyPalettes.positions, p.colours).map { (CGFloat($0), ($1.r, $1.g, $1.b)) }
        colors = SkyFrameLook.grid.map { SkyFrameLook.sample(stops, at: $0) }
        fields = [p.haze, p.glow, p.shade]
        sunGlow = OKLab(hex: 0xFFFFFF)
        sunGlowStrength = 0
    }

    private static func sample(_ stops: [(CGFloat, (Double, Double, Double))], at x: Double) -> OKLab {
        var i = 0
        while i < stops.count - 2, x > Double(stops[i + 1].0) { i += 1 }
        let (xa, a) = stops[i], (xb, b) = stops[i + 1]
        let t = max(0, min(1, (x - Double(xa)) / max(0.0001, Double(xb - xa))))
        return OKLab(r: a.0 + (b.0 - a.0) * t, g: a.1 + (b.1 - a.1) * t, b: a.2 + (b.2 - a.2) * t)
    }

    func mixed(_ o: SkyFrameLook, _ t: Double) -> SkyFrameLook {
        // A sky without fields borrows the other's shapes at no strength, so they fade in rather than jump.
        let mine = fields.isEmpty ? o.fields.map { var f = $0; f.alpha = 0; return f } : fields
        let theirs = o.fields.isEmpty ? fields.map { var f = $0; f.alpha = 0; return f } : o.fields
        return SkyFrameLook(colors: zip(colors, o.colors).map { $0.mixed($1, t) },
                            fields: zip(mine, theirs).map { $0.mixed(with: $1, t: t) },
                            sunGlow: sunGlow.mixed(o.sunGlow, t),
                            sunGlowStrength: sunGlowStrength + (o.sunGlowStrength - sunGlowStrength) * t)
    }

    func paint(in context: GraphicsContext, size: CGSize, sunX: CGFloat) {
        let bounds = Path(CGRect(origin: .zero, size: size))
        context.fill(bounds, with: .linearGradient(
            Gradient(stops: zip(colors, SkyFrameLook.grid).map { Gradient.Stop(color: $0.color, location: CGFloat($1)) }),
            startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height)))
        for field in fields where field.alpha > 0.001 { SkyFrameLook.paint(field, in: context, size: size) }
        if sunGlowStrength > 0.001 {
            var layer = context
            layer.blendMode = .screen
            let glow = sunGlow.color
            layer.fill(bounds, with: .radialGradient(
                Gradient(stops: [.init(color: glow.opacity(sunGlowStrength), location: 0),
                                 .init(color: glow.opacity(sunGlowStrength * 0.35), location: 0.5),
                                 .init(color: glow.opacity(0), location: 1)]),
                center: CGPoint(x: sunX, y: size.height * 1.05), startRadius: 0, endRadius: size.width * 0.9))
        }
    }

    /// Astra's soft field: an ellipse sized as a fraction of the screen on each axis, as its own renderer draws it.
    private static func paint(_ field: SkyField, in context: GraphicsContext, size: CGSize) {
        var layer = context
        layer.translateBy(x: CGFloat(field.x) * size.width, y: CGFloat(field.y) * size.height)
        layer.scaleBy(x: CGFloat(field.rx) * size.width, y: CGFloat(field.ry) * size.height)
        let cover = Path(CGRect(x: CGFloat(-field.x / field.rx), y: CGFloat(-field.y / field.ry),
                                width: CGFloat(1 / field.rx), height: CGFloat(1 / field.ry)))
        let falloff: [(Double, Double)] = [(0, 1), (0.30, 0.65), (0.65, 0.18), (1, 0)]
        layer.fill(cover, with: .radialGradient(
            Gradient(stops: falloff.map { Gradient.Stop(color: field.colour.color(opacity: field.alpha * $0.1), location: CGFloat($0.0)) }),
            center: .zero, startRadius: 0, endRadius: 1))
    }
}

// MARK: Colour, mixed in OKLab so a sky never goes muddy between two others

struct OKLab {
    var l: Double, a: Double, b: Double

    init(hex: UInt32) {
        self.init(r: Double((hex >> 16) & 0xFF) / 255, g: Double((hex >> 8) & 0xFF) / 255, b: Double(hex & 0xFF) / 255)
    }

    /// From sRGB values, 0 to 1.
    init(r red: Double, g green: Double, b blue: Double) {
        func lin(_ c: Double) -> Double { c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4) }
        let r = lin(red), g = lin(green), bl = lin(blue)
        let L = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * bl)
        let M = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * bl)
        let S = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * bl)
        l = 0.2104542553 * L + 0.7936177850 * M - 0.0040720468 * S
        a = 1.9779984951 * L - 2.4285922050 * M + 0.4505937099 * S
        b = 0.0259040371 * L + 0.7827717662 * M - 0.8086757660 * S
    }

    init(l: Double, a: Double, b: Double) { self.l = l; self.a = a; self.b = b }

    func mixed(_ o: OKLab, _ t: Double) -> OKLab { OKLab(l: l + (o.l - l) * t, a: a + (o.a - a) * t, b: b + (o.b - b) * t) }

    var rgb: (Double, Double, Double) {
        let L = pow(l + 0.3963377774 * a + 0.2158037573 * b, 3)
        let M = pow(l - 0.1055613458 * a - 0.0638541728 * b, 3)
        let S = pow(l - 0.0894841775 * a - 1.2914855480 * b, 3)
        func gam(_ c: Double) -> Double { max(0, min(1, c <= 0.0031308 ? 12.92 * c : 1.055 * pow(c, 1 / 2.4) - 0.055)) }
        return (gam(4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S),
                gam(-1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S),
                gam(-0.0041960863 * L - 0.7034186147 * M + 1.7076147010 * S))
    }

    var color: Color {
        let L = pow(l + 0.3963377774 * a + 0.2158037573 * b, 3)
        let M = pow(l - 0.1055613458 * a - 0.0638541728 * b, 3)
        let S = pow(l - 0.0894841775 * a - 1.2914855480 * b, 3)
        func gam(_ c: Double) -> Double { max(0, min(1, c <= 0.0031308 ? 12.92 * c : 1.055 * pow(c, 1 / 2.4) - 0.055)) }
        return Color(.sRGB, red: gam(4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S),
                     green: gam(-1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S),
                     blue: gam(-0.0041960863 * L - 0.7034186147 * M + 1.7076147010 * S))
    }
}

/// The sky for a height of the sun: four colours from the top to the horizon and the light the low sun throws.
/// Dawn is cooler (rose and peach), dusk warmer (apricot into violet).
public struct SkyLook {
    let colors: [OKLab]
    let glowLab: OKLab

    /// How light the top and the lower half are (OKLab L, 0 to 1): what decides black or white text over them.
    public var lightTop: Double { (colors[0].l + colors[1].l) / 2 }
    public var lightLow: Double { (colors[1].l + colors[2].l) / 2 }
    public var glow: Color { glowLab.color }

    /// The same stops as `stops`, as sRGB values.
    var rgbStops: [(CGFloat, (Double, Double, Double))] {
        let at: [Double] = [0, 0.38, 0.74, 1]
        var out: [(CGFloat, (Double, Double, Double))] = []
        for k in 0..<3 {
            for j in 0..<3 {
                let t = Double(j) / 3
                out.append((CGFloat(at[k] + (at[k + 1] - at[k]) * t), colors[k].mixed(colors[k + 1], t).rgb))
            }
        }
        out.append((1, colors[3].rgb))
        return out
    }

    /// Nine stops from the four colours, mixed in OKLab, so SwiftUI's own blending between them stays clean.
    public var stops: [Gradient.Stop] {
        let at: [Double] = [0, 0.38, 0.74, 1]
        var out: [Gradient.Stop] = []
        for k in 0..<3 {
            for j in 0..<3 {
                let t = Double(j) / 3
                out.append(.init(color: colors[k].mixed(colors[k + 1], t).color, location: at[k] + (at[k + 1] - at[k]) * t))
            }
        }
        out.append(.init(color: colors[3].color, location: 1))
        return out
    }

    private static let skies: [String: ([UInt32], UInt32)] = [
        "night": ([0x02040A, 0x060A18, 0x0B1226, 0x121A33], 0x000000),
        "astro": ([0x040917, 0x0A1331, 0x18204A, 0x262A57], 0x2A1E4A),
        "blueM": ([0x081332, 0x18295E, 0x3C4E8E, 0x7C7CAE], 0x8E7FB8),
        "blueE": ([0x0A1131, 0x1E2560, 0x4D3F88, 0x9A6A9E], 0xB0628E),
        "dawn": ([0x0F2250, 0x2C4787, 0x8492C0, 0xEBC3B6], 0xF6C6B4),
        "dusk": ([0x14204F, 0x3B4489, 0xA07AAE, 0xF2AE8E], 0xF79A72),
        "riseM": ([0x1B3B78, 0x4C70AE, 0xAFBBD8, 0xF7D9C2], 0xFBD7BC),
        "riseE": ([0x1C3674, 0x55639F, 0xC49BB5, 0xF6BE95], 0xFFB37A),
        "goldM": ([0x21529C, 0x5A88C6, 0xB9CDE6, 0xF3E2CF], 0xFBE3C8),
        "goldE": ([0x204E96, 0x5B7FBE, 0xBDB7D6, 0xF4D3AE], 0xFFD19A),
        "day": ([0x1D5DC6, 0x4A8BE1, 0x9CC2F1, 0xE2ECFB], 0xFFFFFF),
        "high": ([0x1957C6, 0x3E84E4, 0x8DB9F2, 0xDAE7FB], 0xFFFFFF),
    ]

    public init(elevation e: Double, evening: Bool) {
        let stops: [(Double, String)] = [(-90, "night"), (-18, "night"), (-12, "astro"), (-6, evening ? "blueE" : "blueM"),
                                         (-2.5, evening ? "dusk" : "dawn"), (1, evening ? "riseE" : "riseM"),
                                         (7, evening ? "goldE" : "goldM"), (20, "day"), (90, "high")]
        var i = 0
        while i < stops.count - 2, e > stops[i + 1].0 { i += 1 }
        let (ea, a) = stops[i], (eb, b) = stops[i + 1]
        let raw = max(0, min(1, (e - ea) / (eb - ea)))
        let t = raw * raw * (3 - 2 * raw)
        let A = SkyLook.skies[a]!, B = SkyLook.skies[b]!
        colors = (0..<4).map { OKLab(hex: A.0[$0]).mixed(OKLab(hex: B.0[$0]), t) }
        glowLab = OKLab(hex: A.1).mixed(OKLab(hex: B.1), t)
    }

    /// The sky over a place at a time.
    public init(date: Date, latitude: Double, longitude: Double) {
        let sun = SunPosition(date: date, latitude: latitude, longitude: longitude)
        self.init(elevation: sun.elevation, evening: sun.azimuth > 180)
    }
}

/// The sun is FERB's amber dot: white high in the sky, amber as it nears the horizon.
struct SunDisc: View {
    let elevation: Double
    var body: some View {
        let core = elevation < 10 ? Lamp.amber : Color(hex: 0xFFF6E5)
        ZStack {
            Circle().fill(core.opacity(0.3)).frame(width: 60, height: 60).blur(radius: 16)
            Circle().fill(core).frame(width: 18, height: 18).shadow(color: core.opacity(0.8), radius: 7)
        }
    }
}
/// The moon, lit as much as it really is tonight.
struct MoonDisc: View {
    let phase: MoonPhase
    let size: CGFloat
    var body: some View {
        ZStack {
            Circle().fill(Color(hex: 0x2A2C33))
            MoonLight(lit: phase.lit, waxing: phase.waxing).fill(Color(hex: 0xEDE8DC))
        }
        .frame(width: size, height: size)
        .shadow(color: Color(hex: 0xEDE8DC).opacity(0.35 * phase.lit), radius: 8)
    }
}

/// The lit part of the moon: half a circle on the sunlit side plus an ellipse that grows or eats into it.
struct MoonLight: Shape {
    let lit: Double
    let waxing: Bool
    func path(in rect: CGRect) -> Path {
        let r = rect.width / 2, c = CGPoint(x: rect.midX, y: rect.midY)
        let k = CGFloat(1 - 2 * lit)   // 1 new, 0 half, -1 full
        var p = Path()
        let side: CGFloat = waxing ? 1 : -1
        p.move(to: CGPoint(x: c.x, y: c.y - r))
        for i in 0...40 {
            let a = -Double.pi / 2 + Double.pi * Double(i) / 40
            p.addLine(to: CGPoint(x: c.x + side * r * CGFloat(cos(a)), y: c.y + r * CGFloat(sin(a))))
        }
        for i in 0...40 {
            let a = Double.pi / 2 - Double.pi * Double(i) / 40
            p.addLine(to: CGPoint(x: c.x + side * k * r * CGFloat(cos(a)), y: c.y + r * CGFloat(sin(a))))
        }
        p.closeSubpath()
        return p
    }
}

/// Fixed stars: the same sky every night, so nothing twinkles or moves on its own.
struct Stars: View {
    let count: Int
    let seed: UInt64
    var body: some View {
        Canvas { context, size in
            var state = seed
            func next() -> Double {
                state = state &* 6364136223846793005 &+ 1442695040888963407
                return Double(state >> 33) / Double(1 << 31)
            }
            for _ in 0..<count {
                let x = next() * size.width, y = next() * size.height, s = 0.6 + next() * 1.3, a = 0.35 + next() * 0.6
                context.fill(Path(ellipseIn: CGRect(x: x, y: y, width: s, height: s)), with: .color(.white.opacity(a)))
            }
        }
    }
}

/// A sky for a given height of the sun, with no sun or moon: the background alone, for comparing. `chosen` false
/// is the lab's own sky; true is the sky the app uses (the owner's picks).
public struct SkyBackgroundOnly: View {
    let elevation: Double
    let evening: Bool
    let chosen: Bool

    public init(elevation: Double, evening: Bool, chosen: Bool = false) {
        self.elevation = elevation
        self.evening = evening
        self.chosen = chosen
    }

    public var body: some View {
        SkyPaint(elevation: elevation, across: evening ? 0.6 : -0.6, moon: MoonPhase(date: Date()), evening: evening, showSun: false, chosen: chosen)
    }
}
