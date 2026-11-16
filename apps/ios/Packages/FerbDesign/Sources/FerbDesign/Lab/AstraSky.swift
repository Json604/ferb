// Copied from ~/Desktop/ferb-glass-lab Sources/FerbGlass/AstraSky.swift at 23a0fde (LabSource.commit), unchanged.

// From the owner's ferb-sky-backgrounds.zip (made by Astra, 2026-11-14); the palettes the owner researched.
// Kept as delivered except that its palette types are internal instead of private, so the lab's sky can blend the
// owner's picks (sky/choices.json) with its own.

import SwiftUI
import Foundation

// Requires macOS 12+ / iOS 15+ (Canvas and TimelineView).
// The embedded palettes below are generated from sky-presets.json, version 1.
// Example: ChatView().background(FerbSkyBackground(phase: .sunset, scrim: 0.18))
// Or: FerbSkyBackground(phase: .now), which updates once a minute.
// Clock anchors are illustrative art direction, not solar events or live weather.

public enum FerbSkyPhase: String, CaseIterable, Identifiable {
    case now, dawn, sunrise, morning, noon
    case goldenHour = "golden-hour"
    case sunset, dusk, night

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .now: return "Now"
        case .dawn: return "Dawn"
        case .sunrise: return "Sunrise"
        case .morning: return "Morning"
        case .noon: return "Noon"
        case .goldenHour: return "Golden hour"
        case .sunset: return "Sunset"
        case .dusk: return "Dusk"
        case .night: return "Night"
        }
    }
}

@available(macOS 12.0, iOS 15.0, *)
public struct FerbSkyBackground: View {
    public var phase: FerbSkyPhase
    public var scrim: Double
    public var date: Date?
    public var calendar: Calendar

    /// `date` fixes the clock for a reproducible `.now` preview; nil follows local time.
    /// `scrim` is independent of the palette and defaults to no darkening.
    /// The host owns text colours and contrast; this view does not set a foreground style.
    public init(
        phase: FerbSkyPhase = .now,
        scrim: Double = 0,
        date: Date? = nil,
        calendar: Calendar = .current
    ) {
        self.phase = phase
        self.scrim = skyClamp(scrim)
        self.date = date
        self.calendar = calendar
    }

    public var body: some View {
        Group {
            if phase == .now && date == nil {
                TimelineView(.periodic(from: Date(), by: 60)) { timeline in
                    skyCanvas(FerbSkyPalettes.at(timeline.date, calendar: calendar))
                }
            } else {
                skyCanvas(phase == .now
                    ? FerbSkyPalettes.at(date ?? Date(), calendar: calendar)
                    : FerbSkyPalettes.fixed(phase))
            }
        }
        .overlay(FerbSkyScrim(opacity: scrim))
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    // No animation loop for fixed presets, and no implicit motion on clock updates.
    private func skyCanvas(_ palette: SkyPalette) -> some View {
        GeometryReader { geometry in
            Canvas(opaque: true, colorMode: .nonLinear) { context, size in
                guard size.width > 0, size.height > 0 else { return }
                let bounds = Path(CGRect(origin: .zero, size: size))
                let stops = zip(palette.colours, FerbSkyPalettes.positions).map { colour, position in
                    Gradient.Stop(color: colour.color(), location: CGFloat(position))
                }
                context.fill(bounds, with: .linearGradient(
                    Gradient(stops: stops),
                    startPoint: .zero,
                    endPoint: CGPoint(x: 0, y: size.height)
                ))
                // Source-over order matches CSS's reversed background-image stack.
                Self.paint(palette.haze, in: context, size: size)
                Self.paint(palette.glow, in: context, size: size)
                Self.paint(palette.shade, in: context, size: size)
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
        }
    }

    private static func paint(_ field: SkyField, in context: GraphicsContext, size: CGSize) {
        var layer = context
        // The radius is a fraction of the full canvas, independently on each axis.
        layer.translateBy(x: CGFloat(field.x) * size.width, y: CGFloat(field.y) * size.height)
        layer.scaleBy(x: CGFloat(field.rx) * size.width, y: CGFloat(field.ry) * size.height)
        let cover = Path(CGRect(
            x: CGFloat(-field.x / field.rx),
            y: CGFloat(-field.y / field.ry),
            width: CGFloat(1 / field.rx),
            height: CGFloat(1 / field.ry)
        ))
        let falloff: [(Double, Double)] = [(0, 1), (0.30, 0.65), (0.65, 0.18), (1, 0)]
        let stops = falloff.map { radius, strength in
            Gradient.Stop(color: field.colour.color(opacity: field.alpha * strength), location: CGFloat(radius))
        }
        layer.fill(cover, with: .radialGradient(
            Gradient(stops: stops), center: .zero, startRadius: 0, endRadius: 1
        ))
    }
}

/// Optional independent dark overlay. Place behind chat content, above the sky.
/// For example, use `.background(FerbSkyBackground(phase: .noon, scrim: 0.22))`.
/// Choose its strength alongside the actual text and panel surfaces; it is not a contrast guarantee.
public struct FerbSkyScrim: View {
    public var opacity: Double

    public init(opacity: Double = 0.18) {
        self.opacity = skyClamp(opacity)
    }

    public var body: some View {
        Color.black.opacity(skyClamp(opacity))
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

func skyClamp(_ value: Double) -> Double {
    min(1, max(0, value))
}

func skyLerp(_ a: Double, _ b: Double, _ t: Double) -> Double {
    a + (b - a) * t
}

struct SkyRGB {
    var r: Double
    var g: Double
    var b: Double

    init(hex: UInt32) {
        r = Double((hex >> 16) & 255) / 255
        g = Double((hex >> 8) & 255) / 255
        b = Double(hex & 255) / 255
    }

    init(r: Double, g: Double, b: Double) {
        self.r = r
        self.g = g
        self.b = b
    }

    func color(opacity: Double = 1) -> Color {
        Color(.sRGB, red: r, green: g, blue: b, opacity: opacity)
    }

    // Temporal interpolation matches ferb-sky.js; spatial gradients remain sRGB.
    func mixed(with other: SkyRGB, t: Double) -> SkyRGB {
        let a = lab(), b = other.lab()
        return Self.fromLab((
            skyLerp(a.0, b.0, t),
            skyLerp(a.1, b.1, t),
            skyLerp(a.2, b.2, t)
        ))
    }

    private func lab() -> (Double, Double, Double) {
        func linear(_ value: Double) -> Double {
            value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4)
        }
        let r = linear(self.r), g = linear(self.g), b = linear(self.b)
        let l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
        let m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
        let s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
        return (
            0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
            1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
            0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s
        )
    }

    private static func fromLab(_ lab: (Double, Double, Double)) -> SkyRGB {
        let (lightness, a, b) = lab
        let l = pow(lightness + 0.3963377774 * a + 0.2158037573 * b, 3)
        let m = pow(lightness - 0.1055613458 * a - 0.0638541728 * b, 3)
        let s = pow(lightness - 0.0894841775 * a - 1.291485548 * b, 3)
        func channel(_ value: Double) -> Double {
            let v = skyClamp(value)
            let encoded = v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1 / 2.4) - 0.055
            // Match the JS helper's 8-bit output rounding.
            return (skyClamp(encoded) * 255).rounded() / 255
        }
        return SkyRGB(
            r: channel(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
            g: channel(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
            b: channel(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s)
        )
    }
}

struct SkyField {
    var x: Double
    var y: Double
    var rx: Double
    var ry: Double
    var colour: SkyRGB
    var alpha: Double

    func mixed(with other: SkyField, t: Double) -> SkyField {
        SkyField(
            x: skyLerp(x, other.x, t), y: skyLerp(y, other.y, t),
            rx: skyLerp(rx, other.rx, t), ry: skyLerp(ry, other.ry, t),
            colour: colour.mixed(with: other.colour, t: t),
            alpha: skyLerp(alpha, other.alpha, t)
        )
    }
}

struct SkyPalette {
    var colours: [SkyRGB]
    var haze: SkyField
    var glow: SkyField
    var shade: SkyField

    func mixed(with other: SkyPalette, t: Double) -> SkyPalette {
        SkyPalette(
            colours: zip(colours, other.colours).map { a, b in a.mixed(with: b, t: t) },
            haze: haze.mixed(with: other.haze, t: t),
            glow: glow.mixed(with: other.glow, t: t),
            shade: shade.mixed(with: other.shade, t: t)
        )
    }
}

struct SkyFrame {
    let phase: FerbSkyPhase
    let hour: Double
    let palette: SkyPalette
}

enum FerbSkyPalettes {
    static let positions: [Double] = [0, 0.24, 0.48, 0.67, 0.82, 0.93, 1]

    static let frames: [SkyFrame] = [
        .init(phase: .dawn, hour: 5.4, palette: .init(
            colours: [.init(hex: 0x304359), .init(hex: 0x536A80), .init(hex: 0x8996A5), .init(hex: 0xB9ADB6), .init(hex: 0xDAB6B7), .init(hex: 0xEAC5B8), .init(hex: 0xEED1BC)],
            haze: .init(x: 0.18, y: 0.59, rx: 1.0, ry: 0.34, colour: .init(hex: 0x99BBC7), alpha: 0.13),
            glow: .init(x: 0.73, y: 1.04, rx: 0.85, ry: 0.56, colour: .init(hex: 0xFFE2BF), alpha: 0.32),
            shade: .init(x: 0.05, y: 0.06, rx: 1.1, ry: 0.98, colour: .init(hex: 0x15233D), alpha: 0.15)
        )),
        .init(phase: .sunrise, hour: 6.5, palette: .init(
            colours: [.init(hex: 0x384F70), .init(hex: 0x5F7D9B), .init(hex: 0x9BAEBB), .init(hex: 0xC8C4C0), .init(hex: 0xE6C5AD), .init(hex: 0xF2C590), .init(hex: 0xF6D39A)],
            haze: .init(x: 0.12, y: 0.72, rx: 0.95, ry: 0.28, colour: .init(hex: 0xE2AC96), alpha: 0.14),
            glow: .init(x: 0.78, y: 0.97, rx: 0.58, ry: 0.47, colour: .init(hex: 0xFFF0C5), alpha: 0.72),
            shade: .init(x: 0.02, y: 0.04, rx: 0.92, ry: 0.92, colour: .init(hex: 0x203855), alpha: 0.12)
        )),
        .init(phase: .morning, hour: 9.0, palette: .init(
            colours: [.init(hex: 0x397CB4), .init(hex: 0x5C9AC8), .init(hex: 0x8AB7D7), .init(hex: 0xB5D2E2), .init(hex: 0xD3E0E5), .init(hex: 0xE4E8E4), .init(hex: 0xEEECE0)],
            haze: .init(x: 0.22, y: 0.84, rx: 1.1, ry: 0.32, colour: .init(hex: 0xE2F0EF), alpha: 0.14),
            glow: .init(x: 0.93, y: 0.62, rx: 0.88, ry: 0.85, colour: .init(hex: 0xFFF8DF), alpha: 0.39),
            shade: .init(x: 0.05, y: 0.05, rx: 1.05, ry: 0.95, colour: .init(hex: 0x23578B), alpha: 0.06)
        )),
        .init(phase: .noon, hour: 12.5, palette: .init(
            colours: [.init(hex: 0x1765A4), .init(hex: 0x267DB8), .init(hex: 0x4398C8), .init(hex: 0x72B6D8), .init(hex: 0xA0CEDF), .init(hex: 0xC3E0E7), .init(hex: 0xD6E9EA)],
            haze: .init(x: 0.45, y: 1.06, rx: 1.3, ry: 0.4, colour: .init(hex: 0xE6F8F4), alpha: 0.16),
            glow: .init(x: 0.85, y: -0.16, rx: 0.86, ry: 1.05, colour: .init(hex: 0xEAF9FF), alpha: 0.29),
            shade: .init(x: 0.03, y: 0.03, rx: 0.85, ry: 1.0, colour: .init(hex: 0x064078), alpha: 0.08)
        )),
        .init(phase: .goldenHour, hour: 16.7, palette: .init(
            colours: [.init(hex: 0x5C788F), .init(hex: 0x8A9FA9), .init(hex: 0xB6BAB5), .init(hex: 0xD8C8B0), .init(hex: 0xEAD0A7), .init(hex: 0xF1CF96), .init(hex: 0xEFC388)],
            haze: .init(x: 0.12, y: 0.82, rx: 0.94, ry: 0.34, colour: .init(hex: 0xD89B7E), alpha: 0.2),
            glow: .init(x: 0.88, y: 0.88, rx: 0.7, ry: 0.67, colour: .init(hex: 0xFFE9B1), alpha: 0.68),
            shade: .init(x: 0.02, y: 0.03, rx: 0.98, ry: 0.93, colour: .init(hex: 0x364A65), alpha: 0.12)
        )),
        .init(phase: .sunset, hour: 18.1, palette: .init(
            colours: [.init(hex: 0x3D496B), .init(hex: 0x626484), .init(hex: 0x92839A), .init(hex: 0xBE969F), .init(hex: 0xDEA693), .init(hex: 0xEDA87B), .init(hex: 0xEFB477)],
            haze: .init(x: 0.85, y: 0.74, rx: 0.88, ry: 0.29, colour: .init(hex: 0xD98B98), alpha: 0.2),
            glow: .init(x: 0.2, y: 1.02, rx: 0.68, ry: 0.49, colour: .init(hex: 0xFFD89B), alpha: 0.65),
            shade: .init(x: 0.95, y: 0.01, rx: 1.05, ry: 0.94, colour: .init(hex: 0x263653), alpha: 0.17)
        )),
        .init(phase: .dusk, hour: 18.85, palette: .init(
            colours: [.init(hex: 0x0B2137), .init(hex: 0x1C3B53), .init(hex: 0x3D5D76), .init(hex: 0x687D8D), .init(hex: 0x96959D), .init(hex: 0xBF9B93), .init(hex: 0xD5A68E)],
            haze: .init(x: 0.74, y: 0.68, rx: 1.05, ry: 0.3, colour: .init(hex: 0x7896B0), alpha: 0.1),
            glow: .init(x: 0.15, y: 1.16, rx: 0.82, ry: 0.43, colour: .init(hex: 0xF7BF96), alpha: 0.41),
            shade: .init(x: 0.92, y: 0.04, rx: 1.05, ry: 1.08, colour: .init(hex: 0x081B31), alpha: 0.18)
        )),
        .init(phase: .night, hour: 22.0, palette: .init(
            colours: [.init(hex: 0x070D19), .init(hex: 0x0C1728), .init(hex: 0x142438), .init(hex: 0x1C3044), .init(hex: 0x263B4D), .init(hex: 0x2D4454), .init(hex: 0x344B58)],
            haze: .init(x: 0.3, y: 1.06, rx: 1.2, ry: 0.41, colour: .init(hex: 0x68899B), alpha: 0.07),
            glow: .init(x: 0.79, y: 0.2, rx: 0.61, ry: 0.7, colour: .init(hex: 0x839EB9), alpha: 0.1),
            shade: .init(x: 0.02, y: 0.03, rx: 1.0, ry: 0.98, colour: .init(hex: 0x020811), alpha: 0.23)
        ))
    ]

    static func fixed(_ phase: FerbSkyPhase) -> SkyPalette {
        frames.first(where: { $0.phase == phase })?.palette ?? frames[7].palette
    }

    static func at(_ date: Date, calendar: Calendar) -> SkyPalette {
        let parts = calendar.dateComponents([.hour, .minute, .second], from: date)
        let hour = Double(parts.hour ?? 0)
            + Double(parts.minute ?? 0) / 60
            + Double(parts.second ?? 0) / 3600
        let night = frames[7].palette
        let anchors = [
            SkyFrame(phase: .night, hour: 0, palette: night),
            SkyFrame(phase: .night, hour: 4.1, palette: night)
        ] + frames + [SkyFrame(phase: .night, hour: 24, palette: night)]
        for index in 0..<(anchors.count - 1) {
            let a = anchors[index], b = anchors[index + 1]
            if hour >= a.hour && hour <= b.hour {
                let t = skyClamp((hour - a.hour) / (b.hour - a.hour))
                let smooth = t * t * (3 - 2 * t)
                return a.palette.mixed(with: b.palette, t: smooth)
            }
        }
        return night
    }
}
