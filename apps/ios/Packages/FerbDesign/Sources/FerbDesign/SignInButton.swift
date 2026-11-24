import SwiftUI

/// Who a sign-in button signs in with (kb APP-1140, APP-1141).
public enum SignInProvider: Sendable { case google, apple }

/// "Continue with Google" / "Continue with Apple" with the provider's own logo (owner kb APP-1140; how, kb APP-1141),
/// the same on the iPhone, Android (SignInButton.kt) and FERB.app (FERBNotch SignInButton.swift). Everything is in
/// proportion to the button's height, from the two providers' own 44-point buttons:
/// - Google's iOS button (Sign in with Google branding guidelines): the "G" 20 of 44 high, 16 from the leading edge,
///   12 to the words. The Apple logo at the height of Apple's medium logo (Human Interface Guidelines, Sign in with
///   Apple: the logo file as high as the button; its glyph is 19 of 44), centred on the same line as the G (Apple:
///   "inset the logo if necessary" to line it up with other sign-in logos).
/// - The words stay centred in the whole button, kept clear of the logo on both sides (16 + 20 + 12 of 44).
/// - On a light surface Google's dark theme for both buttons (fill #131314, a 1-point #8E918F line inside, words
///   #E3E3E3), the Apple logo white; on FERB's dark surface (`onFerb`, as Pill turns white there) Google's light
///   theme (#FFFFFF, #747775, #1F1F1F), the Apple logo black: Apple's black and white styles, each on the background
///   Apple allows it on.
/// The logos are decorative: the button says only its words. Pressed and waiting (`onClick` nil) look like Pill's,
/// logo included.
public struct SignInButton: View {
    let provider: SignInProvider
    let title: String
    var height: CGFloat = 52
    var onClick: (() -> Void)?

    public init(provider: SignInProvider, title: String, height: CGFloat = 52, onClick: (() -> Void)? = nil) {
        self.provider = provider
        self.title = title
        self.height = height
        self.onClick = onClick
    }

    @Environment(\.onFerb) private var onFerb

    public var body: some View {
        let form = onFerb ? SignInForm.light : SignInForm.dark
        let unit = height / 44
        let shape = RoundedRectangle(cornerRadius: 16, style: .continuous)
        ZStack {
            Text(title).font(.system(size: 16, weight: .semibold)).foregroundStyle(form.words)
                .multilineTextAlignment(.center).lineLimit(2)
                .padding(.horizontal, unit * 48)
            HStack(spacing: 0) {
                Group {
                    switch provider {
                    case .google: GoogleG(size: unit * 20)
                    case .apple:
                        // the system's own Apple logo, as high as the glyph of Apple's medium logo
                        Image(systemName: "apple.logo").resizable().scaledToFit()
                            .frame(height: unit * 19).foregroundStyle(form.appleLogo)
                    }
                }
                .frame(width: unit * 20)
                .accessibilityHidden(true)
                Spacer(minLength: 0)
            }
            .padding(.leading, unit * 16)
        }
        .frame(maxWidth: .infinity, minHeight: height)
        .background {
            shape.fill(form.fill)
            shape.strokeBorder(form.line, lineWidth: 1)
        }
        .contentShape(shape)
        .pressable(onClick)
    }
}

private struct SignInForm {
    let fill: Color, line: Color, words: Color, appleLogo: Color

    static let dark = SignInForm(fill: Color(red: 0x13 / 255, green: 0x13 / 255, blue: 0x14 / 255),
                                 line: Color(red: 0x8E / 255, green: 0x91 / 255, blue: 0x8F / 255),
                                 words: Color(red: 0xE3 / 255, green: 0xE3 / 255, blue: 0xE3 / 255), appleLogo: .white)
    static let light = SignInForm(fill: .white, line: Color(red: 0x74 / 255, green: 0x77 / 255, blue: 0x75 / 255),
                                  words: Color(red: 0x1F / 255, green: 0x1F / 255, blue: 0x1F / 255), appleLogo: .black)
}

/// Google's standard colour "G", `size` square, drawn from Google's own artwork (signin-assets.zip, "Theme=Dark,
/// Show text=No, Shape=Square, Platform=Android+Web.svg", the G in its 20-unit box at 10,10 of the 40-unit file): the G
/// path as the mask, under it the conic gradient (its colours and angles as the file has them, its centre and its 45°
/// turn from the file's transform) and the six soft ellipses (centre, radii, turn, colour, 1-unit blur). Not drawn:
/// the 1-unit blur the file also puts on the gradient layer, which only softens it where the blue ellipses cover it
/// and at the circle's edge, 2.6 units outside the G. Android draws the same numbers (SignInButton.kt).
public struct GoogleG: View {
    let size: CGFloat

    public init(size: CGFloat) { self.size = size }

    public var body: some View {
        Canvas { context, canvas in
            let unit = canvas.width / 20
            context.drawLayer { layer in
                layer.scaleBy(x: unit, y: unit)
                layer.translateBy(x: -10, y: -10)
                layer.clip(to: GoogleArtwork.g)
                let c = GoogleArtwork.conicCentre
                var conic = layer
                conic.translateBy(x: c.x, y: c.y)
                conic.rotate(by: .degrees(GoogleArtwork.conicTurn))
                conic.translateBy(x: -c.x, y: -c.y)
                conic.fill(Path(ellipseIn: GoogleArtwork.circle), with: .conicGradient(GoogleArtwork.conic, center: c))
                for e in GoogleArtwork.ellipses {
                    var soft = layer
                    soft.addFilter(.blur(radius: GoogleArtwork.blur))
                    soft.translateBy(x: e.cx, y: e.cy)
                    soft.rotate(by: .degrees(e.turn))
                    soft.fill(Path(ellipseIn: CGRect(x: -e.rx, y: -e.ry, width: 2 * e.rx, height: 2 * e.ry)),
                              with: .color(Color(red: e.rgb.0 / 255, green: e.rgb.1 / 255, blue: e.rgb.2 / 255)))
                }
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// Google's G artwork, the numbers exactly as Google's file has them (in its 40-unit coordinates).
enum GoogleArtwork {
    static let g = SVGPath.parse(
        "M29.3987 18.1814H19.9849V22.0445H25.3598C25.1286 23.294 24.4294 24.3596 23.3676 25.0712C22.4746 25.6716 " +
        "21.3266 26.0211 19.9849 26.0211C17.3864 26.0211 15.1823 24.2666 14.3947 21.9004C14.1952 21.2989 14.0853 20.6599 " +
        "14.0853 19.9983C14.0853 19.3367 14.1952 18.6966 14.3947 18.0962C15.1823 15.7311 17.3864 13.9755 19.9849 13.9755C21.4524 " +
        "13.9755 22.767 14.4816 23.8039 15.4713L26.6653 12.6057C24.936 10.9908 22.6786 10 19.9849 10C16.0832 10 12.705 12.2414 " +
        "11.0618 15.5076C10.383 16.8592 10 18.3834 10 19.9994C10 21.6155 10.383 23.1396 11.0618 24.4913C12.705 27.7597 16.0832 " +
        "30 19.9849 30C22.6797 30 24.9485 29.1137 26.6018 27.5861C28.4887 25.8452 29.5732 23.2702 29.5732 20.2275C29.5732 " +
        "19.5182 29.5131 18.835 29.3987 18.1825V18.1814Z")

    /// The gradient's circle: M7.25922 19.7927 C7.25922 12.6759 13.0209 6.90668 20.1283 6.90668 … 32.9973, 32.6786.
    static let circle = CGRect(x: 7.25922, y: 6.90668, width: 32.9973 - 7.25922, height: 32.6786 - 6.90668)
    /// The file's matrix(0.00804129 -0.00805186 0.00804128 0.00805186 19.6819 19.7927): a quarter-turn's half back.
    static let conicCentre = CGPoint(x: 19.6819, y: 19.7927)
    static let conicTurn = -45.0
    /// conic-gradient(from 90deg, …): the file's "from 90deg" is 3 o'clock, where SwiftUI's conic gradient starts.
    static let conic = Gradient(stops: [
        (0, 255, 70, 65), (4.14555, 255, 70, 65), (39.154, 49, 134, 255), (72.0044, 49, 134, 255),
        (96.7463, 0, 165, 183), (120.897, 14, 188, 95), (154.722, 14, 188, 95), (179.136, 108, 196, 0),
        (203.588, 255, 204, 0), (226.915, 255, 211, 20), (251.688, 255, 204, 0), (273.129, 255, 106, 43),
        (289.305, 253, 70, 65), (359.593, 255, 70, 65), (360, 255, 70, 65),
    ].map { (deg: Double, r: Double, g: Double, b: Double) in
        Gradient.Stop(color: Color(red: r / 255, green: g / 255, blue: b / 255), location: deg / 360)
    })

    struct Ellipse { let cx, cy, rx, ry, turn: Double; let rgb: (Double, Double, Double) }

    static let ellipses = [
        Ellipse(cx: 20.0496, cy: 20.2413, rx: 5.39634, ry: 2.83537, turn: 24.4473, rgb: (0x31, 0x86, 0xFF)),
        Ellipse(cx: 33.3538, cy: 18.2155, rx: 7.43918, ry: 3.09357, turn: 0, rgb: (0x31, 0x86, 0xFF)),
        Ellipse(cx: 25.2744, cy: 16.2195, rx: 7.40854, ry: 2.37805, turn: 0, rgb: (0xFF, 0x46, 0x41)),
        Ellipse(cx: 29.5427, cy: 12.9268, rx: 7.40854, ry: 2.37805, turn: 0, rgb: (0xFF, 0x5B, 0x8B)),
        Ellipse(cx: 24.4817, cy: 19.878, rx: 8.5061, ry: 3.10976, turn: 0, rgb: (0x31, 0x86, 0xFF)),
        Ellipse(cx: 25.1842, cy: 14.0197, rx: 4.53882, ry: 2.37805, turn: -28.6599, rgb: (0xFF, 0x46, 0x41)),
    ]

    /// feGaussianBlur stdDeviation="1", in the file's units (the context is scaled to them).
    static let blur: CGFloat = 1
}

/// SVG path data with absolute M, L, H, V, C and Z (all Google's G uses), so the artwork stays as its file has it.
enum SVGPath {
    static func parse(_ data: String) -> Path {
        var path = Path()
        var command: Character = "M"
        var numbers: [Double] = []
        var current = CGPoint.zero
        func flush() {
            var i = 0
            func take(_ n: Int) -> [Double]? {
                guard i + n <= numbers.count else { return nil }
                defer { i += n }
                return Array(numbers[i..<i + n])
            }
            repeat {
                switch command {
                case "M", "L":
                    guard let p = take(2) else { break }
                    current = CGPoint(x: p[0], y: p[1])
                    if command == "M" { path.move(to: current); command = "L" } else { path.addLine(to: current) }
                case "H":
                    guard let p = take(1) else { break }
                    current.x = p[0]
                    path.addLine(to: current)
                case "V":
                    guard let p = take(1) else { break }
                    current.y = p[0]
                    path.addLine(to: current)
                case "C":
                    guard let p = take(6) else { break }
                    current = CGPoint(x: p[4], y: p[5])
                    path.addCurve(to: current, control1: CGPoint(x: p[0], y: p[1]), control2: CGPoint(x: p[2], y: p[3]))
                case "Z":
                    path.closeSubpath()
                default: break
                }
            } while i < numbers.count && command != "Z"
            numbers = []
        }
        var token = ""
        func endNumber() {
            if let n = Double(token) { numbers.append(n) }
            token = ""
        }
        for ch in data {
            if "MLHVCZ".contains(ch) {
                endNumber()
                if !numbers.isEmpty || command == "Z" { flush() }
                command = ch
                if ch == "Z" { flush() }
            } else if ch == " " || ch == "," {
                endNumber()
            } else if ch == "-" && !token.isEmpty {
                endNumber()
                token = "-"
            } else {
                token.append(ch)
            }
        }
        endNumber()
        if !numbers.isEmpty { flush() }
        return path
    }
}
