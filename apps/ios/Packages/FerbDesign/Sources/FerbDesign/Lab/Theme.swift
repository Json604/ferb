// Copied from ~/Desktop/ferb-glass-lab Sources/FerbGlass/Theme.swift at 23a0fde (LabSource.commit), unchanged:
// the Mac's warm ramp (Tone), the lamps, the glass depths, FERB's mark and the Mac's controls. On the phone only
// FerbMark, LampDot, Tone and Lamp are drawn; the rest is kept as the lab wrote it so the two stay one file.

import SwiftUI

// MARK: Colour

public extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255, opacity: opacity)
    }
}

/// The neutrals: one grey ramp, built in OKLCH at hue 80 (the hue of FERB's amber dot) with a chroma of 0.004 to
/// 0.012, so every grey leans very slightly toward the brand instead of being a dead or blue-black grey.
/// Lightness steps are perceptual: 13, 17, 21.5, 26.5, 33, 38 for surfaces and 66, 80, 95.5 for text.
public enum Tone {
    public static let ink0 = Color(hex: 0x080706)
    public static let ink1 = Color(hex: 0x110F0D)
    public static let ink2 = Color(hex: 0x1B1916)
    public static let ink3 = Color(hex: 0x272521)
    public static let ink4 = Color(hex: 0x383531)
    public static let line = Color(hex: 0x45423E)

    /// Ivory, not white: FERB's own voice. 16.7:1 on ink1.
    public static let text = Color(hex: 0xF4EFE7)
    /// 10.2:1 on ink1.
    public static let textDim = Color(hex: 0xC1BDB7)
    /// 6.2:1 on ink1, 4.9:1 on ink3: the faintest text allowed.
    public static let textFaint = Color(hex: 0x95928B)

    // On glass a surface is light laid on the frost, so these three are ivory at a fixed strength, never a gradient.
    public static let fill1 = Color(hex: 0xF4EFE7, opacity: 0.055)
    public static let fill2 = Color(hex: 0xF4EFE7, opacity: 0.095)
    public static let fill3 = Color(hex: 0xF4EFE7, opacity: 0.15)
    public static let well = Color.black.opacity(0.28)
    public static let hairline = Color(hex: 0xF4EFE7, opacity: 0.08)
}

/// The only hues FERB owns. They are indicator lamps: small, exact, and never spread over a surface.
/// Amber and green sit at the same perceived lightness (OKLCH L 0.80) and red a step lower (0.70), so none shouts.
public enum Lamp {
    /// FERB's own colour, the dot of the logo. As a lamp it means one thing: FERB wants the owner. oklch(0.80 0.14 72)
    public static let amber = Color(hex: 0xF5AE4B)
    /// Done. oklch(0.80 0.16 152)
    public static let green = Color(hex: 0x60DB89)
    /// Couldn't do it, or needs attention. oklch(0.70 0.17 27)
    public static let red = Color(hex: 0xF66D62)
}

/// What FERB is doing. It decides the colour of one lamp and nothing else.
public enum Mood: String, CaseIterable {
    case idle, working, asking, done, failed, listening

    public var lamp: Color {
        switch self {
        case .idle: return Tone.textFaint
        case .working: return Tone.text
        case .asking: return Lamp.amber
        case .done: return Lamp.green
        case .failed: return Lamp.red
        case .listening: return Tone.text
        }
    }

    /// Moods that want the owner's eye light the notch's lower lip; the others stay dark.
    public var lit: Bool { self == .asking || self == .done || self == .failed }
}

// MARK: Type

public enum InkTone {
    case primary, dim, faint
    public var color: Color {
        switch self {
        case .primary: return Tone.text
        case .dim: return Tone.textDim
        case .faint: return Tone.textFaint
        }
    }
}

public extension Text {
    /// The type ramp: the system face at three strengths. Only large type is tightened.
    func ink(_ size: CGFloat, _ weight: Font.Weight = .regular, _ tone: InkTone = .primary) -> some View {
        font(.system(size: size, weight: weight))
            .tracking(size >= 17 ? -0.3 : 0)
            .foregroundStyle(tone.color)
    }
}

// MARK: Glass

/// Four depths. Only a panel is real frost; inside it, depth is a step of light.
public enum GlassLevel { case panel, card, control, inset }

public struct GlassBackground<S: InsettableShape>: View {
    public let shape: S
    public var level: GlassLevel = .card

    public init(shape: S, level: GlassLevel = .card) {
        self.shape = shape
        self.level = level
    }

    public var body: some View {
        switch level {
        case .panel:
            ZStack {
                shape.fill(.ultraThinMaterial)
                shape.fill(Tone.ink1.opacity(0.56))
                // One edge: light on the top lip, gone a third of the way down. No outline around the rest.
                shape.strokeBorder(LinearGradient(stops: [.init(color: .white.opacity(0.26), location: 0),
                                                          .init(color: .white.opacity(0.05), location: 0.3),
                                                          .init(color: .white.opacity(0.03), location: 1)],
                                                  startPoint: .top, endPoint: .bottom), lineWidth: 1)
            }
        case .card:
            shape.fill(Tone.fill1)
        case .control:
            shape.fill(Tone.fill2)
        case .inset:
            shape.fill(Tone.well)
                .overlay { shape.strokeBorder(LinearGradient(colors: [.black.opacity(0.4), .clear],
                                                             startPoint: .top, endPoint: .center), lineWidth: 1) }
        }
    }
}

public extension View {
    func glass<S: InsettableShape>(_ shape: S, _ level: GlassLevel = .card) -> some View {
        background { GlassBackground(shape: shape, level: level) }
    }

    func glassCard(_ radius: CGFloat = 14) -> some View {
        glass(RoundedRectangle(cornerRadius: radius, style: .continuous), .card)
    }

    /// The two shadows a floating panel casts: a wide soft one for height, a tight one where it meets the page.
    func panelShadow() -> some View {
        shadow(color: .black.opacity(0.34), radius: 36, y: 20).shadow(color: .black.opacity(0.3), radius: 3, y: 1)
    }
}

// MARK: Marks

/// A lamp: a small dot of colour with a tight halo. The only thing in FERB that glows.
public struct LampDot: View {
    public var color: Color
    public var size: CGFloat = 6

    public init(color: Color, size: CGFloat = 6) {
        self.color = color
        self.size = size
    }

    public var body: some View {
        Circle().fill(color).frame(width: size, height: size).shadow(color: color.opacity(0.7), radius: size * 0.45)
    }
}

/// FERB's logo: the ivory "f" and its amber dot. A logo, not a lamp: its dot is always amber.
public struct FerbMark: View {
    public var size: CGFloat = 64

    public init(size: CGFloat = 64) {
        self.size = size
    }

    public var body: some View {
        ZStack {
            Tile(size: size)
            Letter().fill(Tone.text)
            LampDot(color: Lamp.amber, size: size / 6)
                .position(x: size * (82.0 - 16) / 96, y: size * (85.0 - 16) / 96)
        }
        .frame(width: size, height: size)
    }

    /// The dark tile with its light edge (the launch draws the same, kb PHN-632).
    struct Tile: View {
        var size: CGFloat
        var body: some View {
            ZStack {
                RoundedRectangle(cornerRadius: size * 0.23, style: .continuous)
                    .fill(LinearGradient(colors: [Tone.ink3, Tone.ink0], startPoint: .top, endPoint: .bottom))
                RoundedRectangle(cornerRadius: size * 0.23, style: .continuous)
                    .strokeBorder(LinearGradient(colors: [.white.opacity(0.22), .white.opacity(0.02)],
                                                 startPoint: .top, endPoint: .center), lineWidth: max(0.75, size / 96))
            }
        }
    }

    /// The "f" of ferb-mark.svg, in that file's 96-point tile (its 128 box less the 16 margin).
    struct Letter: Shape {
        func path(in rect: CGRect) -> Path {
            let k = rect.width / 96
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: (x - 16) * k, y: (y - 16) * k) }
            var path = Path()
            path.move(to: p(49.969, 58.772))
            path.addLine(to: p(49.969, 90)); path.addLine(to: p(60.477, 90)); path.addLine(to: p(60.477, 58.772))
            path.addLine(to: p(67.729, 58.772)); path.addLine(to: p(67.729, 51.742)); path.addLine(to: p(60.477, 51.742))
            path.addLine(to: p(60.477, 49.448))
            path.addQuadCurve(to: p(61.402, 46.081), control: p(60.477, 47.08))
            path.addQuadCurve(to: p(64.473, 45.082), control: p(62.327, 45.082))
            path.addQuadCurve(to: p(68.321, 45.304), control: p(66.471, 45.082))
            path.addLine(to: p(68.321, 37.46))
            path.addQuadCurve(to: p(65.583, 37.275), control: p(66.989, 37.386))
            path.addQuadCurve(to: p(62.771, 37.164), control: p(64.177, 37.164))
            path.addQuadCurve(to: p(53.151, 40.42), control: p(56.333, 37.164))
            path.addQuadCurve(to: p(49.969, 48.782), control: p(49.969, 43.676))
            path.addLine(to: p(49.969, 51.742)); path.addLine(to: p(43.679, 51.742)); path.addLine(to: p(43.679, 58.772))
            path.closeSubpath()
            return path
        }
    }
}

public struct Spinner: View {
    public var size: CGFloat = 16
    public var width: CGFloat = 1.8

    public init(size: CGFloat = 16, width: CGFloat = 1.8) {
        self.size = size
        self.width = width
    }

    public var body: some View {
        TimelineView(.animation) { context in
            let turn = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 0.9) / 0.9
            Circle()
                .trim(from: 0.05, to: 0.8)
                .stroke(AngularGradient(colors: [Tone.text.opacity(0), Tone.text], center: .center,
                                        startAngle: .degrees(0), endAngle: .degrees(290)),
                        style: StrokeStyle(lineWidth: width, lineCap: .round))
                .rotationEffect(.degrees(turn * 360))
                .frame(width: size, height: size)
        }
    }
}

// MARK: Controls

public struct FerbButton: ButtonStyle {
    public enum Kind { case primary, secondary, quiet }
    public var kind: Kind = .secondary
    public var compact: Bool = false

    public init(kind: Kind = .secondary, compact: Bool = false) {
        self.kind = kind
        self.compact = compact
    }

    public func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration, style: self)
    }

    private struct Face: View {
        let configuration: ButtonStyleConfiguration
        let style: FerbButton
        @State private var hover = false

        var body: some View {
            let shape = RoundedRectangle(cornerRadius: style.compact ? 7 : 9, style: .continuous)
            configuration.label
                .font(.system(size: style.compact ? 11.5 : 12.5, weight: .medium))
                .foregroundStyle(style.kind == .primary ? Tone.ink0 : (style.kind == .quiet && !hover ? Tone.textDim : Tone.text))
                .padding(.horizontal, style.compact ? 9 : 13)
                .frame(height: style.compact ? 24 : 30)
                .background {
                    switch style.kind {
                    // The one action that matters is solid ivory with dark ink: emphasis without a hue.
                    case .primary: shape.fill(hover ? Color.white : Tone.text)
                    case .secondary: shape.fill(hover ? Tone.fill3 : Tone.fill2)
                    case .quiet: shape.fill(hover ? Tone.fill1 : .clear)
                    }
                }
                .scaleEffect(configuration.isPressed ? 0.97 : 1)
                .opacity(configuration.isPressed ? 0.85 : 1)
                .animation(.spring(duration: 0.2, bounce: 0.15), value: configuration.isPressed)
                .animation(.easeOut(duration: 0.12), value: hover)
                .onHover { hover = $0 }
                .contentShape(shape)
        }
    }
}

/// An icon in a small neutral tile: it names a thing (a schedule, a permission, a kind of question).
/// Always one colour. The lamp, not the icon, carries state.
public struct IconTile: View {
    public let symbol: String
    public var size: CGFloat = 30
    public var dim: Bool = false

    public init(symbol: String, size: CGFloat = 30, dim: Bool = false) {
        self.symbol = symbol
        self.size = size
        self.dim = dim
    }

    public var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.44, weight: .medium))
            .foregroundStyle(dim ? Tone.textFaint : Tone.text)
            .frame(width: size, height: size)
            .background(Tone.fill2, in: RoundedRectangle(cornerRadius: size * 0.3, style: .continuous))
    }
}

/// A round icon button.
public struct IconButton: View {
    public let symbol: String
    public var size: CGFloat = 28
    public var prominent: Bool = false
    @State private var hover = false

    public init(symbol: String, size: CGFloat = 28, prominent: Bool = false) {
        self.symbol = symbol
        self.size = size
        self.prominent = prominent
    }

    public var body: some View {
        Button {} label: {
            Image(systemName: symbol)
                .font(.system(size: size * 0.42, weight: .semibold))
                .foregroundStyle(prominent ? Tone.ink0 : Tone.textDim)
                .frame(width: size, height: size)
                .background(prominent ? (hover ? Color.white : Tone.text) : (hover ? Tone.fill3 : Tone.fill2), in: Circle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

public struct Chip: View {
    public let text: String
    public var symbol: String? = nil
    public var lamp: Color? = nil

    public init(text: String, symbol: String? = nil, lamp: Color? = nil) {
        self.text = text
        self.symbol = symbol
        self.lamp = lamp
    }

    public var body: some View {
        HStack(spacing: 5) {
            if let lamp { LampDot(color: lamp, size: 5) }
            if let symbol { Image(systemName: symbol).font(.system(size: 9.5, weight: .medium)) }
            Text(text).font(.system(size: 11, weight: .medium))
        }
        .foregroundStyle(Tone.textDim)
        .padding(.horizontal, 7)
        .frame(height: 21)
        .glass(RoundedRectangle(cornerRadius: 6, style: .continuous), .control)
    }
}

/// A switch. "On" takes the owner's own accent colour, because it is the owner's choice.
public struct GlassSwitch: View {
    @Binding public var isOn: Bool

    public init(isOn: Binding<Bool>) {
        self._isOn = isOn
    }

    public var body: some View {
        ZStack(alignment: isOn ? .trailing : .leading) {
            Capsule().fill(isOn ? Color.accentColor : Tone.fill2)
            Circle().fill(Tone.text).shadow(color: .black.opacity(0.3), radius: 1.5, y: 1).padding(2)
        }
        .frame(width: 36, height: 21)
        .contentShape(Capsule())
        .onTapGesture { withAnimation(.spring(duration: 0.25, bounce: 0.2)) { isOn.toggle() } }
    }
}

public struct Keycap: View {
    public let label: String

    public init(label: String) {
        self.label = label
    }

    public var body: some View {
        Text(label)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(Tone.textDim)
            .padding(.horizontal, 6)
            .frame(minWidth: 22, minHeight: 21)
            .background {
                // A key has a body: a darker step below its face.
                RoundedRectangle(cornerRadius: 5.5, style: .continuous).fill(Color.black.opacity(0.35)).offset(y: 1.5)
                RoundedRectangle(cornerRadius: 5.5, style: .continuous).fill(Tone.ink4)
            }
    }
}

/// A status mark for one step of a task. Each state has its own shape, so colour is never the only signal.
public struct StepGlyph: View {
    public enum Kind { case done, running, waiting, asking, failed }
    public let kind: Kind
    public var size: CGFloat = 18

    public init(kind: Kind, size: CGFloat = 18) {
        self.kind = kind
        self.size = size
    }

    public var body: some View {
        ZStack {
            switch kind {
            case .done: mark("checkmark", Lamp.green)
            case .asking: mark("questionmark", Lamp.amber)
            case .failed: mark("exclamationmark", Lamp.red)
            case .running: Spinner(size: size - 4)
            case .waiting:
                Circle().strokeBorder(Tone.textFaint.opacity(0.7), style: StrokeStyle(lineWidth: 1.2, dash: [2.5, 2.5])).padding(2)
            }
        }
        .frame(width: size, height: size)
    }

    private func mark(_ symbol: String, _ color: Color) -> some View {
        Circle().fill(color.opacity(0.16)).padding(1)
            .overlay { Image(systemName: symbol).font(.system(size: size * 0.44, weight: .bold)).foregroundStyle(color) }
    }
}

public struct ProgressRing: View {
    public var value: Double
    public var width: CGFloat = 2.5

    public init(value: Double, width: CGFloat = 2.5) {
        self.value = value
        self.width = width
    }

    public var body: some View {
        ZStack {
            Circle().stroke(Tone.fill2, lineWidth: width)
            Circle().trim(from: 0, to: value)
                .stroke(Tone.text, style: StrokeStyle(lineWidth: width, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
    }
}

public struct ProgressBar: View {
    public var value: Double

    public init(value: Double) {
        self.value = value
    }

    public var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Tone.fill2)
                Capsule().fill(Tone.text).frame(width: geo.size.width * value)
            }
        }
        .frame(height: 3)
    }
}
