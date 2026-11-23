import SwiftUI

// NOT FROM THE LAB. The temporary chat (the owner, 2026-11-23: "the same small icon that turned it on turns it off",
// as ChatGPT's and Claude's apps): one chat the Mac holds in memory only, with no prior context. The same dashed
// bubble on FERB.app and both phones, in the chat tab's top bar beside the profile button. Words are the app's.

/// A chat bubble with its tail at the lower left, drawn as one outline: stroked with dashes it is the temporary chat's
/// mark. FERB.app's DashedBubbleShape, point for point (the same shape on the Mac and both phones): the body is the
/// frame's full width and 0.8 of its height with corners of 0.38 of its shorter side; the tail leaves the body's foot
/// at 0.42 of its width, reaches the frame's foot at 0.14 and comes back at 0.2. Drawn in a frame 0.9 as high as wide.
public struct DashedBubbleShape: Shape {
    public init() {}
    public func path(in rect: CGRect) -> Path {
        let body = CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: rect.height * 0.8)
        let radius = min(body.width, body.height) * 0.38
        var path = Path()
        path.move(to: CGPoint(x: body.minX + radius, y: body.minY))
        path.addLine(to: CGPoint(x: body.maxX - radius, y: body.minY))
        path.addArc(center: CGPoint(x: body.maxX - radius, y: body.minY + radius), radius: radius,
                    startAngle: .degrees(-90), endAngle: .degrees(0), clockwise: false)
        path.addLine(to: CGPoint(x: body.maxX, y: body.maxY - radius))
        path.addArc(center: CGPoint(x: body.maxX - radius, y: body.maxY - radius), radius: radius,
                    startAngle: .degrees(0), endAngle: .degrees(90), clockwise: false)
        // the tail, down to the lower left
        path.addLine(to: CGPoint(x: body.minX + body.width * 0.42, y: body.maxY))
        path.addLine(to: CGPoint(x: body.minX + body.width * 0.14, y: rect.maxY))
        path.addLine(to: CGPoint(x: body.minX + body.width * 0.2, y: body.maxY))
        path.addLine(to: CGPoint(x: body.minX + radius, y: body.maxY))
        path.addArc(center: CGPoint(x: body.minX + radius, y: body.maxY - radius), radius: radius,
                    startAngle: .degrees(90), endAngle: .degrees(180), clockwise: false)
        path.addLine(to: CGPoint(x: body.minX, y: body.minY + radius))
        path.addArc(center: CGPoint(x: body.minX + radius, y: body.minY + radius), radius: radius,
                    startAngle: .degrees(180), endAngle: .degrees(270), clockwise: false)
        path.closeSubpath()
        return path
    }
}

/// The dashed bubble at `size` wide (FERB.app's DashedBubbleGlyph): the outline alone while off; filled, its dashes
/// still showing over the fill, while on. `ink` draws the dashes, `fill` fills it.
public struct DashedBubble: View {
    let on: Bool
    let size: CGFloat
    let ink: Color
    let fill: Color
    public init(on: Bool, size: CGFloat = 20, ink: Color = Ink.white, fill: Color = Ink.black) {
        self.on = on
        self.size = size
        self.ink = ink
        self.fill = fill
    }
    public static let dash: [CGFloat] = [2.6, 2.2]
    public var body: some View {
        ZStack {
            if on { DashedBubbleShape().fill(fill) }
            DashedBubbleShape().stroke(ink, style: StrokeStyle(lineWidth: 1.6, lineCap: .round, dash: Self.dash))
        }
        .frame(width: size, height: size * 0.9)
    }
}

/// The temporary chat's switch for the top bar: off, the dashed outline on the bar's dark circle; on, the circle takes
/// the bar's accent (white, as the chosen tab's pill) and the bubble is filled black with its white dashes showing.
public struct TemporaryToggle {
    public let on: Bool
    /// "Temporary chat" while off, "End temporary chat" while on (the app's strings).
    public let label: String
    public let busy: Bool
    /// The folded bar's menu button's words ("More", kb PHN-820): the bar folds only where this bubble is.
    public let menuLabel: String
    public let action: () -> Void

    public init(on: Bool, label: String, busy: Bool = false, menuLabel: String = "More", action: @escaping () -> Void) {
        self.on = on
        self.label = label
        self.busy = busy
        self.menuLabel = menuLabel
        self.action = action
    }
}

/// The dashed bubble's button (42 round, the profile button's size).
public struct TemporaryButton: View {
    let on: Bool
    public init(on: Bool) { self.on = on }

    public var body: some View {
        DashedBubble(on: on, size: 20)
            .frame(width: 42, height: 42)
            .background(on ? Ink.white : Ink.darkFill, in: Circle())
    }
}

/// The line at the top of a temporary chat: what it is, on the black capsule of the app's status line.
public struct TemporaryBanner: View {
    let text: String
    public init(_ text: String) { self.text = text }
    public var body: some View {
        HStack(spacing: 8) {
            DashedBubble(on: false, size: 15)
            Text(text).type(13, .medium, Ink.white).lineSpacing(1).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.leading, 11).padding(.trailing, 13).padding(.vertical, 7)
        .background(Ink.black, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 16)
        .accessibilityElement(children: .combine)
    }
}

/// An empty temporary chat: one quiet line where the conversation will be.
public struct TemporaryEmpty: View {
    let text: String
    @Environment(\.skyInk) private var ink
    public init(_ text: String) { self.text = text }
    public var body: some View {
        Text(text).type(15, .regular, ink.low).multilineTextAlignment(.center)
            .frame(maxWidth: .infinity).padding(.horizontal, 40)
    }
}

// MARK: the top bar giving way (the owner, 2026-11-23, kb PHN-820: "jsut pack them in a good looking hamburger icon if
// theres no space ... adjust based on that, contracat if really no space")

/// How the top bar fits its width, in order of giving way: `normal` (the chosen tab's name, the other tabs at least 36,
/// the temporary bubble on the chat tab, the profile button); `folded` (the bubble and the profile button in one menu
/// button); `contracted` (the chosen tab drops its name too). A pure function of the bar's width (the window less the
/// safe area's sides and the bar's margins) and the chosen tab's measured name, as Android's.
public struct TopBarLayout: Equatable, Sendable {
    public enum Mode: Sendable { case normal, folded, contracted }
    public let mode: Mode
    /// Each unchosen tab's width: the lab's 46 at most; under 36 only where nothing more can give way.
    public let tabWidth: CGFloat
    /// The chosen tab shows its name.
    public var named: Bool { mode != .contracted }
    /// The round buttons are folded into the menu button.
    public let menu: Bool

    public static let minTab: CGFloat = 36
    public static let maxTab: CGFloat = 46
    /// A round button and the gap before it.
    public static let button: CGFloat = 42 + 8
    /// The pill's padding (4 each side) and the chosen tab without and with its name (12 each side, a 17 icon, 6, name).
    public static let pillPadding: CGFloat = 8
    public static func chosenWidth(name: CGFloat?) -> CGFloat { 12 * 2 + 17 + (name.map { 6 + $0 } ?? 0) }

    public init(mode: Mode, tabWidth: CGFloat, menu: Bool) {
        self.mode = mode
        self.tabWidth = tabWidth
        self.menu = menu
    }

    /// `width`: the bar's own width; `nameWidth`: the chosen tab's name as drawn; `temporary`: the bubble is on this bar
    /// (the chat tab). Without it there is nothing to fold: the bar contracts straight away.
    public static func of(width: CGFloat, nameWidth: CGFloat, temporary: Bool, tabs: Int = PhoneTab.allCases.count) -> TopBarLayout {
        func tab(_ chosen: CGFloat, buttons: Int) -> CGFloat {
            ((width - pillPadding - CGFloat(buttons) * button - chosen) / CGFloat(tabs - 1)).rounded(.down)
        }
        let named = chosenWidth(name: nameWidth)
        let normal = tab(named, buttons: temporary ? 2 : 1)
        if normal >= minTab { return TopBarLayout(mode: .normal, tabWidth: min(maxTab, normal), menu: false) }
        if temporary {
            let folded = tab(named, buttons: 1)
            if folded >= minTab { return TopBarLayout(mode: .folded, tabWidth: min(maxTab, folded), menu: true) }
        }
        let bare = tab(chosenWidth(name: nil), buttons: 1)
        return TopBarLayout(mode: .contracted, tabWidth: min(maxTab, max(minTab, bare)), menu: temporary)
    }
}

/// Three lines, 18 wide, 2 thick, round caps, 5 apart, centred (the same on both phones).
public struct HamburgerShape: Shape {
    public init() {}
    public func path(in rect: CGRect) -> Path {
        var p = Path()
        for dy in [-7.0, 0, 7] {
            let y = rect.midY + dy
            p.move(to: CGPoint(x: rect.midX - 9, y: y))
            p.addLine(to: CGPoint(x: rect.midX + 9, y: y))
        }
        return p
    }
}

/// The folded bar's one round button: the three lines white on the dark circle, the connection's badge where the
/// profile button has it, and while the temporary chat is on the dashed bubble's small mark at its top left.
public struct TopBarMenuButton: View {
    let temporaryOn: Bool
    let mark: LinkMark?
    public init(temporaryOn: Bool, mark: LinkMark?) {
        self.temporaryOn = temporaryOn
        self.mark = mark
    }
    public var body: some View {
        HamburgerShape().stroke(Ink.white, style: StrokeStyle(lineWidth: 2, lineCap: .round))
            .frame(width: 42, height: 42)
            .background(Ink.darkFill, in: Circle())
            .overlay(alignment: .bottomTrailing) {
                if let mark {
                    LinkMarkView(mark, size: 7).frame(width: 14, height: 14).background(Ink.darkFill, in: Circle())
                        .offset(x: -2, y: -2)
                }
            }
            .overlay(alignment: .topLeading) {
                // a 16 white circle holding the 12 dashed bubble, dark (as Android's)
                if temporaryOn {
                    DashedBubble(on: false, size: 12, ink: Ink.black)
                        .frame(width: 16, height: 16)
                        .background(Ink.white, in: Circle())
                        .offset(x: 1, y: 1)
                        .accessibilityIdentifier("top-bar-menu-temporary")
                }
            }
    }
}

/// The folded bar's menu, on FERB's dark panel under its button (corner 20, padding 6, at least 240 wide, as Android's):
/// the temporary chat (its words and a check while on) and the profile with the connection's state.
public struct TopBarMenu: View {
    let temporary: TemporaryToggle
    let profile: String
    let connection: String?
    let onTemporary: () -> Void
    let onProfile: (() -> Void)?

    public init(temporary: TemporaryToggle, profile: String, connection: String?, onTemporary: @escaping () -> Void,
                onProfile: (() -> Void)?) {
        self.temporary = temporary
        self.profile = profile
        self.connection = connection
        self.onTemporary = onTemporary
        self.onProfile = onProfile
    }

    public var body: some View {
        VStack(spacing: 0) {
            row(icon: AnyView(DashedBubble(on: temporary.on, size: 18)), title: temporary.label, detail: nil, check: temporary.on,
                action: temporary.busy ? nil : onTemporary)
                .accessibilityIdentifier("top-bar-menu-temporary-row")
            Rectangle().fill(Color.white.opacity(0.1)).frame(height: 1).padding(.leading, 46)
            row(icon: AnyView(Image(systemName: "person.fill").font(.system(size: 16)).foregroundStyle(Ink.white)), title: profile,
                detail: connection, check: false, action: onProfile)
                .accessibilityIdentifier("top-bar-menu-profile-row")
        }
        .padding(6)
        .frame(minWidth: 240)
        .fixedSize()
        .background(Ink.black, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(Color.white.opacity(0.08), lineWidth: 1))
        .shadow(color: .black.opacity(0.25), radius: 18, y: 8)
    }

    private func row(icon: AnyView, title: String, detail: String?, check: Bool, action: (() -> Void)?) -> some View {
        HStack(spacing: 12) {
            icon.frame(width: 24, height: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).type(15.5, .medium, Ink.white).lineLimit(1)
                if let detail { Text(detail).type(13, .regular, Ink.onDark).lineLimit(1) }
            }
            Spacer(minLength: 0)
            if check { Image(systemName: "checkmark").font(.system(size: 14, weight: .semibold)).foregroundStyle(Ink.white) }
        }
        .padding(.horizontal, 10)
        .frame(minHeight: 50)
        .contentShape(Rectangle())
        .pressable(action)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(check ? [.isSelected, .isButton] : .isButton)
    }
}
