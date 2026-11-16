// Golden numbers for :design's sky tests, made by re-evaluating the glass lab's own formulas.
//
// The formulas below are copied by hand from ~/Desktop/ferb-glass-lab (Sources/FerbGlass/Sky.swift: SunPosition,
// SunDay, MoonPhase, SkyView.across, ChosenSky, SkyFrameLook, OKLab, SkyLook, Stars; Sources/FerbGlass/AstraSky.swift:
// SkyRGB, SkyField, SkyPalette, FerbSkyPalettes; Sources/FerbGlassLab/Weather.swift: Season.at and WeatherLayer.r;
// Sources/FerbGlassLab/Mobile.swift: SkyInk). Nothing is imported from the lab and the lab is never built: this file
// needs only Foundation. Drawing code is left out; only numbers are kept.
//
// Run: swift apps/android/tools/sky_golden.swift > apps/android/design/src/test/resources/lab/sky-golden.json
// The lab commit the formulas were copied from is written into the output (LAB_COMMIT below).

import Foundation

let LAB_COMMIT = "23a0fdeabe53a85a124c744b7117c07af58869b5"

// MARK: Sky.swift

struct SunPosition {
    let elevation: Double
    let azimuth: Double
    init(date: Date, latitude: Double, longitude: Double) {
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

struct SunDay {
    let sunrise: Date?
    let sunset: Date?
    init(around date: Date, latitude: Double, longitude: Double, calendar: Calendar) {
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

struct MoonPhase {
    let age: Double
    var lit: Double { (1 - cos(age * 2 * .pi)) / 2 }
    var waxing: Bool { age < 0.5 }
    init(date: Date) {
        let days = date.timeIntervalSince1970 / 86400 + 2440587.5 - 2451550.1
        let cycles = days / 29.530588853
        age = cycles - floor(cycles)
    }
}

func across(azimuth: Double, latitude: Double) -> Double {
    var offset = azimuth - (latitude >= 0 ? 180 : 0)
    if offset > 180 { offset -= 360 }
    if offset < -180 { offset += 360 }
    return max(-1, min(1, (latitude >= 0 ? offset : -offset) / 120))
}

struct OKLab {
    var l: Double, a: Double, b: Double
    init(hex: UInt32) {
        self.init(r: Double((hex >> 16) & 0xFF) / 255, g: Double((hex >> 8) & 0xFF) / 255, b: Double(hex & 0xFF) / 255)
    }
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
}

struct SkyLook {
    let colors: [OKLab]
    let glowLab: OKLab
    var rgbStops: [(Double, (Double, Double, Double))] {
        let at: [Double] = [0, 0.38, 0.74, 1]
        var out: [(Double, (Double, Double, Double))] = []
        for k in 0..<3 {
            for j in 0..<3 {
                let t = Double(j) / 3
                out.append((at[k] + (at[k + 1] - at[k]) * t, colors[k].mixed(colors[k + 1], t).rgb))
            }
        }
        out.append((1, colors[3].rgb))
        return out
    }
    static let skies: [String: ([UInt32], UInt32)] = [
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
    init(elevation e: Double, evening: Bool) {
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
}

// MARK: AstraSky.swift

func skyClamp(_ value: Double) -> Double { min(1, max(0, value)) }
func skyLerp(_ a: Double, _ b: Double, _ t: Double) -> Double { a + (b - a) * t }

struct SkyRGB {
    var r: Double, g: Double, b: Double
    init(hex: UInt32) {
        r = Double((hex >> 16) & 255) / 255
        g = Double((hex >> 8) & 255) / 255
        b = Double(hex & 255) / 255
    }
    init(r: Double, g: Double, b: Double) { self.r = r; self.g = g; self.b = b }
    func mixed(with other: SkyRGB, t: Double) -> SkyRGB {
        let a = lab(), b = other.lab()
        return Self.fromLab((skyLerp(a.0, b.0, t), skyLerp(a.1, b.1, t), skyLerp(a.2, b.2, t)))
    }
    private func lab() -> (Double, Double, Double) {
        func linear(_ value: Double) -> Double { value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4) }
        let r = linear(self.r), g = linear(self.g), b = linear(self.b)
        let l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
        let m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
        let s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
        return (0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
                1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
                0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s)
    }
    private static func fromLab(_ lab: (Double, Double, Double)) -> SkyRGB {
        let (lightness, a, b) = lab
        let l = pow(lightness + 0.3963377774 * a + 0.2158037573 * b, 3)
        let m = pow(lightness - 0.1055613458 * a - 0.0638541728 * b, 3)
        let s = pow(lightness - 0.0894841775 * a - 1.291485548 * b, 3)
        func channel(_ value: Double) -> Double {
            let v = skyClamp(value)
            let encoded = v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1 / 2.4) - 0.055
            return (skyClamp(encoded) * 255).rounded() / 255
        }
        return SkyRGB(r: channel(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
                      g: channel(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
                      b: channel(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s))
    }
}

struct SkyField {
    var x: Double, y: Double, rx: Double, ry: Double
    var colour: SkyRGB
    var alpha: Double
    func mixed(with other: SkyField, t: Double) -> SkyField {
        SkyField(x: skyLerp(x, other.x, t), y: skyLerp(y, other.y, t), rx: skyLerp(rx, other.rx, t), ry: skyLerp(ry, other.ry, t),
                 colour: colour.mixed(with: other.colour, t: t), alpha: skyLerp(alpha, other.alpha, t))
    }
}

struct SkyPalette {
    var colours: [SkyRGB]
    var haze: SkyField, glow: SkyField, shade: SkyField
    func mixed(with other: SkyPalette, t: Double) -> SkyPalette {
        SkyPalette(colours: zip(colours, other.colours).map { a, b in a.mixed(with: b, t: t) },
                   haze: haze.mixed(with: other.haze, t: t), glow: glow.mixed(with: other.glow, t: t),
                   shade: shade.mixed(with: other.shade, t: t))
    }
}

func F(_ x: Double, _ y: Double, _ rx: Double, _ ry: Double, _ hex: UInt32, _ alpha: Double) -> SkyField {
    SkyField(x: x, y: y, rx: rx, ry: ry, colour: SkyRGB(hex: hex), alpha: alpha)
}
func P(_ c: [UInt32], _ haze: SkyField, _ glow: SkyField, _ shade: SkyField) -> SkyPalette {
    SkyPalette(colours: c.map { SkyRGB(hex: $0) }, haze: haze, glow: glow, shade: shade)
}

enum Astra {
    static let positions: [Double] = [0, 0.24, 0.48, 0.67, 0.82, 0.93, 1]
    static let frames: [(String, Double, SkyPalette)] = [
        ("dawn", 5.4, P([0x304359, 0x536A80, 0x8996A5, 0xB9ADB6, 0xDAB6B7, 0xEAC5B8, 0xEED1BC],
                        F(0.18, 0.59, 1.0, 0.34, 0x99BBC7, 0.13), F(0.73, 1.04, 0.85, 0.56, 0xFFE2BF, 0.32), F(0.05, 0.06, 1.1, 0.98, 0x15233D, 0.15))),
        ("sunrise", 6.5, P([0x384F70, 0x5F7D9B, 0x9BAEBB, 0xC8C4C0, 0xE6C5AD, 0xF2C590, 0xF6D39A],
                           F(0.12, 0.72, 0.95, 0.28, 0xE2AC96, 0.14), F(0.78, 0.97, 0.58, 0.47, 0xFFF0C5, 0.72), F(0.02, 0.04, 0.92, 0.92, 0x203855, 0.12))),
        ("morning", 9.0, P([0x397CB4, 0x5C9AC8, 0x8AB7D7, 0xB5D2E2, 0xD3E0E5, 0xE4E8E4, 0xEEECE0],
                           F(0.22, 0.84, 1.1, 0.32, 0xE2F0EF, 0.14), F(0.93, 0.62, 0.88, 0.85, 0xFFF8DF, 0.39), F(0.05, 0.05, 1.05, 0.95, 0x23578B, 0.06))),
        ("noon", 12.5, P([0x1765A4, 0x267DB8, 0x4398C8, 0x72B6D8, 0xA0CEDF, 0xC3E0E7, 0xD6E9EA],
                         F(0.45, 1.06, 1.3, 0.4, 0xE6F8F4, 0.16), F(0.85, -0.16, 0.86, 1.05, 0xEAF9FF, 0.29), F(0.03, 0.03, 0.85, 1.0, 0x064078, 0.08))),
        ("golden-hour", 16.7, P([0x5C788F, 0x8A9FA9, 0xB6BAB5, 0xD8C8B0, 0xEAD0A7, 0xF1CF96, 0xEFC388],
                                F(0.12, 0.82, 0.94, 0.34, 0xD89B7E, 0.2), F(0.88, 0.88, 0.7, 0.67, 0xFFE9B1, 0.68), F(0.02, 0.03, 0.98, 0.93, 0x364A65, 0.12))),
        ("sunset", 18.1, P([0x3D496B, 0x626484, 0x92839A, 0xBE969F, 0xDEA693, 0xEDA87B, 0xEFB477],
                           F(0.85, 0.74, 0.88, 0.29, 0xD98B98, 0.2), F(0.2, 1.02, 0.68, 0.49, 0xFFD89B, 0.65), F(0.95, 0.01, 1.05, 0.94, 0x263653, 0.17))),
        ("dusk", 18.85, P([0x0B2137, 0x1C3B53, 0x3D5D76, 0x687D8D, 0x96959D, 0xBF9B93, 0xD5A68E],
                          F(0.74, 0.68, 1.05, 0.3, 0x7896B0, 0.1), F(0.15, 1.16, 0.82, 0.43, 0xF7BF96, 0.41), F(0.92, 0.04, 1.05, 1.08, 0x081B31, 0.18))),
        ("night", 22.0, P([0x070D19, 0x0C1728, 0x142438, 0x1C3044, 0x263B4D, 0x2D4454, 0x344B58],
                          F(0.3, 1.06, 1.2, 0.41, 0x68899B, 0.07), F(0.79, 0.2, 0.61, 0.7, 0x839EB9, 0.1), F(0.02, 0.03, 1.0, 0.98, 0x020811, 0.23))),
    ]
    static func fixed(_ phase: String) -> SkyPalette { frames.first(where: { $0.0 == phase })?.2 ?? frames[7].2 }
    static func at(hour: Double) -> SkyPalette {
        let night = frames[7].2
        let anchors: [(String, Double, SkyPalette)] = [("night", 0, night), ("night", 4.1, night)] + frames + [("night", 24, night)]
        for index in 0..<(anchors.count - 1) {
            let a = anchors[index], b = anchors[index + 1]
            if hour >= a.1 && hour <= b.1 {
                let t = skyClamp((hour - a.1) / (b.1 - a.1))
                let smooth = t * t * (3 - 2 * t)
                return a.2.mixed(with: b.2, t: smooth)
            }
        }
        return night
    }
}

// MARK: SkyFrameLook and ChosenSky (Sky.swift)

struct FrameLook {
    static let grid: [Double] = (0...24).map { Double($0) / 24 }
    var colors: [OKLab]
    var fields: [SkyField]
    var sunGlow: OKLab
    var sunGlowStrength: Double
    var lightTop: Double { colors[0...6].map(\.l).reduce(0, +) / 7 }
    var lightLow: Double { colors[7...15].map(\.l).reduce(0, +) / 9 }

    init(colors: [OKLab], fields: [SkyField], sunGlow: OKLab, sunGlowStrength: Double) {
        self.colors = colors; self.fields = fields; self.sunGlow = sunGlow; self.sunGlowStrength = sunGlowStrength
    }
    init(mine look: SkyLook, elevation: Double) {
        let stops = look.rgbStops
        colors = FrameLook.grid.map { FrameLook.sample(stops, at: $0) }
        fields = []
        sunGlow = look.glowLab
        sunGlowStrength = max(0, 0.8 - abs(elevation - 1) / 14)
    }
    init(astra p: SkyPalette) {
        let stops = zip(Astra.positions, p.colours).map { ($0, ($1.r, $1.g, $1.b)) }
        colors = FrameLook.grid.map { FrameLook.sample(stops, at: $0) }
        fields = [p.haze, p.glow, p.shade]
        sunGlow = OKLab(hex: 0xFFFFFF)
        sunGlowStrength = 0
    }
    static func sample(_ stops: [(Double, (Double, Double, Double))], at x: Double) -> OKLab {
        var i = 0
        while i < stops.count - 2, x > stops[i + 1].0 { i += 1 }
        let (xa, a) = stops[i], (xb, b) = stops[i + 1]
        let t = max(0, min(1, (x - xa) / max(0.0001, xb - xa)))
        return OKLab(r: a.0 + (b.0 - a.0) * t, g: a.1 + (b.1 - a.1) * t, b: a.2 + (b.2 - a.2) * t)
    }
    func mixed(_ o: FrameLook, _ t: Double) -> FrameLook {
        let mine = fields.isEmpty ? o.fields.map { var f = $0; f.alpha = 0; return f } : fields
        let theirs = o.fields.isEmpty ? fields.map { var f = $0; f.alpha = 0; return f } : o.fields
        return FrameLook(colors: zip(colors, o.colors).map { $0.mixed($1, t) },
                         fields: zip(mine, theirs).map { $0.mixed(with: $1, t: t) },
                         sunGlow: sunGlow.mixed(o.sunGlow, t),
                         sunGlowStrength: sunGlowStrength + (o.sunGlowStrength - sunGlowStrength) * t)
    }
}

enum Chosen {
    enum Pick { case mine(Double), astra(String) }
    static let rising: [(Double, Pick)] = [(-90, .mine(-90)), (-14, .mine(-14)), (-4.5, .mine(-4.5)), (0.5, .astra("sunrise")),
                                           (15, .mine(15)), (40, .astra("noon")), (90, .astra("noon"))]
    static let setting: [(Double, Pick)] = [(-90, .mine(-90)), (-14, .mine(-14)), (-5, .astra("dusk")), (0, .astra("sunset")),
                                            (7, .astra("golden-hour")), (40, .astra("noon")), (90, .astra("noon"))]
    static func look(elevation e: Double, across: Double) -> FrameLook {
        let w = smoother((across + 0.14) / 0.28)
        if w <= 0 { return side(e, evening: false) }
        if w >= 1 { return side(e, evening: true) }
        return side(e, evening: false).mixed(side(e, evening: true), w)
    }
    static func side(_ e: Double, evening: Bool) -> FrameLook {
        let anchors = evening ? setting : rising
        var i = 0
        while i < anchors.count - 2, e > anchors[i + 1].0 { i += 1 }
        let (ea, a) = anchors[i], (eb, b) = anchors[i + 1]
        if case .mine = a, case .mine = b { return FrameLook(mine: SkyLook(elevation: e, evening: evening), elevation: e) }
        return frame(a, evening).mixed(frame(b, evening), smoother((e - ea) / (eb - ea)))
    }
    static func smoother(_ x: Double) -> Double {
        let t = max(0, min(1, x))
        return t * t * t * (t * (t * 6 - 15) + 10)
    }
    static func frame(_ pick: Pick, _ evening: Bool) -> FrameLook {
        switch pick {
        case .mine(let e): return FrameLook(mine: SkyLook(elevation: e, evening: evening), elevation: e)
        case .astra(let phase): return FrameLook(astra: Astra.fixed(phase))
        }
    }
}

// MARK: Weather.swift: Season.at and the layer's random numbers; Stars (Sky.swift)

func season(latitude: Double, longitude: Double, month: Int) -> String {
    if (5...35).contains(latitude), (60...100).contains(longitude), (6...9).contains(month) { return "monsoon" }
    let m = latitude >= 0 ? month : (month + 5) % 12 + 1
    switch m {
    case 3...5: return "spring"
    case 6...8: return "summer"
    case 9...11: return "autumn"
    default: return "winter"
    }
}

func r(_ i: Int, _ k: Int) -> Double {
    let x = sin(Double(i) * 12.9898 + Double(k) * 78.233) * 43758.5453
    return x - floor(x)
}

func stars(count: Int, seed: UInt64) -> [[Double]] {
    var state = seed
    func next() -> Double {
        state = state &* 6364136223846793005 &+ 1442695040888963407
        return Double(state >> 33) / Double(1 << 31)
    }
    var out: [[Double]] = []
    for _ in 0..<count {
        let x = next(), y = next(), s = 0.6 + next() * 1.3, a = 0.35 + next() * 0.6
        out.append([x, y, s, a])
    }
    return out
}

// MARK: Output

func lab(_ c: OKLab) -> [Double] { [c.l, c.a, c.b] }
func rgb(_ c: SkyRGB) -> [Double] { [c.r, c.g, c.b] }
func field(_ f: SkyField) -> [Double] { [f.x, f.y, f.rx, f.ry, f.colour.r, f.colour.g, f.colour.b, f.alpha] }
func look(_ l: FrameLook) -> [String: Any] {
    ["colors": l.colors.map(lab), "fields": l.fields.map(field), "sunGlow": lab(l.sunGlow),
     "sunGlowStrength": l.sunGlowStrength, "lightTop": l.lightTop, "lightLow": l.lightLow,
     "inkTopBlack": l.lightTop > 0.68, "inkLowBlack": l.lightLow > 0.68]
}

let places: [(String, Double, Double, String)] = [
    ("Delhi", 28.61, 77.21, "Asia/Kolkata"), ("London", 51.51, -0.13, "Europe/London"),
    ("San Francisco", 37.77, -122.42, "America/Los_Angeles"), ("Sydney", -33.87, 151.21, "Australia/Sydney"),
    ("Tromso", 69.65, 18.96, "Europe/Oslo"), ("Quito", -0.18, -78.47, "America/Guayaquil"),
]
// Seconds since 1970 (UTC): equinox, solstices, the lab's commit day, a leap day and a far date.
let instants: [Double] = [1774000800, 1782043200, 1797811200, 1763108400, 1763130000, 1763152200, 1709208000, 2000000000, 0, 1500000000]

var sun: [[String: Any]] = []
var sunDays: [[String: Any]] = []
for p in places {
    var cal = Calendar(identifier: .gregorian)
    cal.timeZone = TimeZone(identifier: p.3)!
    for t in instants {
        let d = Date(timeIntervalSince1970: t)
        let s = SunPosition(date: d, latitude: p.1, longitude: p.2)
        sun.append(["place": p.0, "lat": p.1, "lon": p.2, "t": t, "elevation": s.elevation, "azimuth": s.azimuth,
                    "across": across(azimuth: s.azimuth, latitude: p.1)])
        let day = SunDay(around: d, latitude: p.1, longitude: p.2, calendar: cal)
        sunDays.append(["place": p.0, "lat": p.1, "lon": p.2, "zone": p.3, "t": t,
                        "sunrise": day.sunrise.map { $0.timeIntervalSince1970 } as Any? ?? NSNull(),
                        "sunset": day.sunset.map { $0.timeIntervalSince1970 } as Any? ?? NSNull()])
    }
}

var moons: [[String: Any]] = []
for t in instants + [1763600000, 1764800000, 1766000000] {
    let m = MoonPhase(date: Date(timeIntervalSince1970: t))
    moons.append(["t": t, "age": m.age, "lit": m.lit, "waxing": m.waxing])
}

var labSkies: [[String: Any]] = []
for e in [-90.0, -30, -18, -15, -12, -9, -6, -4, -2.5, 0, 1, 4, 7, 12, 20, 45, 90] {
    for evening in [false, true] {
        let s = SkyLook(elevation: e, evening: evening)
        labSkies.append(["elevation": e, "evening": evening, "colors": s.colors.map(lab), "glow": lab(s.glowLab),
                         "rgbStops": s.rgbStops.map { [$0.0, $0.1.0, $0.1.1, $0.1.2] }])
    }
}

var chosen: [[String: Any]] = []
for e in [-60.0, -20, -14, -10, -5, -4.5, -3, -1, 0, 0.5, 3, 7, 10, 15, 25, 40, 60] {
    for a in [-0.9, -0.3, -0.14, -0.05, 0, 0.07, 0.14, 0.5, 1] {
        var entry = look(Chosen.look(elevation: e, across: a))
        entry["elevation"] = e
        entry["across"] = a
        chosen.append(entry)
    }
}

var astraAt: [[String: Any]] = []
for h in [0.0, 3, 4.1, 4.8, 5.4, 6, 6.5, 7.5, 9, 11, 12.5, 14, 16.7, 17.5, 18.1, 18.5, 18.85, 20, 22, 23.5, 24] {
    let p = Astra.at(hour: h)
    astraAt.append(["hour": h, "colours": p.colours.map(rgb), "haze": field(p.haze), "glow": field(p.glow), "shade": field(p.shade)])
}

var seasons: [[String: Any]] = []
for p in places {
    for month in 1...12 { seasons.append(["lat": p.1, "lon": p.2, "month": month, "season": season(latitude: p.1, longitude: p.2, month: month)]) }
}
for (lat, lon) in [(5.0, 60.0), (35.0, 100.0), (4.99, 77.0), (20.0, 100.01)] {
    for month in [5, 6, 9, 10] { seasons.append(["lat": lat, "lon": lon, "month": month, "season": season(latitude: lat, longitude: lon, month: month)]) }
}

var randoms: [[Double]] = []
for i in [0, 1, 2, 7, 50, 51, 100, 149, 299] { for k in [1, 2, 3, 4, 11, 12, 40] { randoms.append([Double(i), Double(k), r(i, k)]) } }

let out: [String: Any] = [
    "labCommit": LAB_COMMIT,
    "sun": sun, "sunDays": sunDays, "moons": moons, "labSkies": labSkies, "chosen": chosen,
    "astraAt": astraAt, "seasons": seasons, "randoms": randoms, "stars": stars(count: 38, seed: 7),
]
let data = try! JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write("\n".data(using: .utf8)!)
