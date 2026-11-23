// Copied from ~/Desktop/ferb-glass-lab Sources/FerbGlassLab/Mobile.swift at 23a0fde (LabSource.commit), as a library.
// Changed: everything the app uses is public; the lab's scaffolding is gone (OS, Device, StatusBar, LabFonts and
// Roboto: on the iPhone the system draws the status bar and the face is the system's); `os.top` and `os.bottom` are
// PhoneInsets (the lab's iPhone numbers by default); `Tab` is PhoneTab (SwiftUI has a Tab of its own) and the kinds of
// Mark, Pill and LinkPicture are MarkKind, PillKind and LinkKind (Android's names; Mark.Kind and Pill.Kind stay as
// aliases); each piece takes optional callbacks (Android's names, kb PHN-509) whose absence draws exactly the lab's
// piece; the running mark reads the motion clock (Motion.swift) so a still picture can be made. Every drawn number
// and colour is the lab's.

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// The phone app: one design, drawn on an iPhone and on an Android phone. Only the hardware differs (the island or
// the camera hole, the status bar); everything inside is the same, so Android can copy the iOS version exactly.

/// The system's bars, the lab's `os.top` and `os.bottom`. The app passes the real ones.
public struct PhoneInsets: Equatable, Sendable {
    public var top: CGFloat
    public var bottom: CGFloat
    /// Not from the lab (kb PHN-820): the safe area's sides (a notch or island beside the page); the top bar keeps
    /// clear of them and gives way when they leave it too little room.
    public var left: CGFloat = 0
    public var right: CGFloat = 0
    public init(top: CGFloat, bottom: CGFloat, left: CGFloat = 0, right: CGFloat = 0) {
        self.top = top
        self.bottom = bottom
        self.left = left
        self.right = right
    }
    /// Mobile.swift OS.ios: a 58 status bar and a 30 home-indicator area, on the lab's 402 by 874 iPhone.
    public static let lab = PhoneInsets(top: 58, bottom: 30)
    /// Mobile.swift OS.ios.size: the iPhone the lab draws.
    public static let labSize = CGSize(width: 402, height: 874)
}

private struct SkyInkKey: EnvironmentKey { static let defaultValue = SkyInk(top: .white, low: .white) }
private struct PhoneInsetsKey: EnvironmentKey { static let defaultValue = PhoneInsets(top: 0, bottom: 0) }

public extension EnvironmentValues {
    /// The insets the page is drawn inside (PhoneScreen sets them): the top bar reads its sides (kb PHN-820).
    var phoneInsets: PhoneInsets {
        get { self[PhoneInsetsKey.self] }
        set { self[PhoneInsetsKey.self] = newValue }
    }
}

/// Black or white for what sits straight on the sky, chosen by how light the sky is behind it.
public struct SkyInk: Equatable {
    public var top: Color
    public var low: Color
    public init(top: Color, low: Color) { self.top = top; self.low = low }
    public init(_ look: SkyFrameLook) {
        top = look.lightTop > SkyInk.threshold ? .black : .white
        low = look.lightLow > SkyInk.threshold ? .black : .white
    }
    /// The lab's threshold: OKLab L 0.68.
    public static let threshold = 0.68
    /// True when the top of the sky takes black text, so the status bar should be dark.
    public var topIsDark: Bool { top == .black }
}

public extension EnvironmentValues {
    var skyInk: SkyInk {
        get { self[SkyInkKey.self] }
        set { self[SkyInkKey.self] = newValue }
    }
}

// MARK: Colour: black, white, and neutral greys only where something is secondary

public enum Ink {
    public static let black = Color.black
    public static let white = Color.white
    public static let detail = Color(hex: 0x666666)      // secondary text on white
    public static let faint = Color(hex: 0x8E8E8E)       // the quietest text on white
    public static let fill = Color(hex: 0xF2F2F2)        // a quiet surface on white
    public static let line = Color(hex: 0xE8E8E8)        // a line on white
    public static let darkFill = Color(hex: 0x2C2C2E)    // a quiet surface on black
    public static let onDark = Color(hex: 0x9A9A9A)      // secondary text on black
    public static let amber = Color(hex: 0xFFB020)       // FERB wants you
    public static let red = Color(hex: 0xE5281B)         // stop, forget, failed
}

/// The other literal colours the lab's phone draws, named by where it draws them.
public enum LabColors {
    /// Mark(.waiting): the dashed ring.
    public static let waitingRing = Color(hex: 0xBDBDBD)
    /// Sheet: the grabber.
    public static let grabber = Color(hex: 0xD1D1D1)
    /// Switch: the track when off.
    public static let switchOff = Color(hex: 0xE0E0E0)
    /// SecretSheet: the keypad's ground.
    public static let keypad = Color(hex: 0xD1D1D6)
}

/// Mobile.swift `View.type`'s tracking: -0.4 from 20 points up, -0.1 below.
public enum PhoneType {
    public static func tracking(_ size: CGFloat) -> CGFloat { size >= 20 ? -0.4 : -0.1 }
}

public extension View {
    func type(_ size: CGFloat, _ weight: Font.Weight = .regular, _ color: Color = Ink.black) -> some View {
        font(.system(size: size, weight: weight)).tracking(PhoneType.tracking(size)).foregroundStyle(color)
    }
}

// MARK: Touch (not from the lab: it draws no touch; DESIGN.md "Motion": a press sinks to 0.97)

/// A press that sinks to 0.97 and 85 % (Theme.swift FerbButton's numbers) on PairIt's springs: quick at touch-down,
/// settling at release (FerbMotion.pressDown, .pressUp; kb PHN-624). Under Reduce Motion it only dims.
public struct PressStyle: ButtonStyle {
    public init() {}
    public func makeBody(configuration: Configuration) -> some View {
        PressLook(label: configuration.label, pressed: configuration.isPressed)
    }
}

private struct PressLook<Label: View>: View {
    let label: Label
    let pressed: Bool
    @Environment(\.accessibilityReduceMotion) private var reduce
    var body: some View {
        label
            .contentShape(Rectangle())
            .scaleEffect(pressed && !reduce ? FerbMotion.pressScale : 1)
            .opacity(pressed ? FerbMotion.pressAlpha : 1)
            .animation((pressed ? FerbMotion.pressDown : FerbMotion.pressUp).animation, value: pressed)
    }
}

public extension View {
    /// A button when there is something to do, else the view as drawn.
    @ViewBuilder func pressable(_ action: (() -> Void)?, label: String? = nil) -> some View {
        if let action {
            if let label {
                Button(action: action) { self }.buttonStyle(PressStyle()).accessibilityLabel(Text(label))
            } else {
                Button(action: action) { self }.buttonStyle(PressStyle())
            }
        } else {
            self
        }
    }
}

// MARK: Pieces

/// Mobile.swift `Tab`: the pages, with the owner's coding sessions after Tasks (not from the lab, kb HAND-651, PHN-798;
/// Android's PhoneTab.SESSIONS).
public enum PhoneTab: CaseIterable, Hashable, Sendable {
    case chat, tasks, sessions, files, mac
    public var title: String { ["Chat", "Tasks", "Sessions", "Files", "Mac"][PhoneTab.allCases.firstIndex(of: self)!] }
    public var symbol: String { ["bubble.left", "checklist", "terminal", "folder", "laptopcomputer"][PhoneTab.allCases.firstIndex(of: self)!] }
}

/// The top of every page: a short black pill of tabs (the chosen one shows its name, PHN-801) and the profile button
/// with the link's state as its badge.
public struct TopBar: View {
    let chosen: PhoneTab
    var waiting: PhoneTab? = .tasks
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    /// Not from the lab: the temporary chat's dashed bubble beside the profile button, on the chat tab only (nil: none).
    var temporary: TemporaryToggle?

    public init(chosen: PhoneTab, waiting: PhoneTab? = .tasks, onTab: ((PhoneTab) -> Void)? = nil, onProfile: (() -> Void)? = nil,
                temporary: TemporaryToggle? = nil, menuOpen: Bool = false) {
        _menuOpen = State(initialValue: menuOpen)
        self.chosen = chosen
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
        self.temporary = temporary
    }

    @Environment(\.tabBarHosted) private var hosted
    @Environment(\.accessibilityReduceMotion) private var reduce
    /// The safe area's sides the bar keeps clear of (kb PHN-820).
    @Environment(\.phoneInsets) private var insets
    /// The folded bar's menu is open (kb PHN-820).
    @State private var menuOpen = false
    @Namespace private var pill
    /// Not from the lab (kb PHN-751): the connection control beside the profile button, when the app gives one.
    @Environment(\.connection) private var connection

    public var body: some View {
        if hosted {
            // inside a TabBarHost: keep the room, draw nothing, take no touch (the host draws the one bar)
            bar(onTab: nil, onProfile: nil, temporary: nil).hidden().accessibilityHidden(true)
        } else {
            bar(onTab: onTab, onProfile: onProfile, temporary: temporary)
                .onChange(of: chosen) { menuOpen = false }
                .onChange(of: temporary == nil) { menuOpen = false }
        }
    }

    /// The bar: the white pill slides between tabs on PairIt's tab spring (overshoot cut by the bar's round ends, as
    /// PairIt's iPhone bar); the tabs' widths, names and colours follow on the enter curve (kb PHN-622).
    private func bar(onTab: ((PhoneTab) -> Void)?, onProfile: (() -> Void)?, temporary: TemporaryToggle?) -> some View {
        GeometryReader { geo in
            let width = geo.size.width - 32 - insets.left - insets.right
            let layout = TopBarLayout.of(width: width, nameWidth: Self.nameWidth(chosen), temporary: temporary != nil)
            barRow(layout: layout, onTab: onTab, onProfile: onProfile, temporary: temporary)
                .padding(.leading, 16 + insets.left).padding(.trailing, 16 + insets.right)
                // the menu closes when the bar stops folding
                .onChange(of: layout.menu) { _, folded in if !folded { menuOpen = false } }
                // the folded bar's menu: under its button, right-aligned; a tap anywhere else closes it
                .overlay {
                    if menuOpen && layout.menu {
                        Color.black.opacity(0.001).frame(width: 6000, height: 6000)
                            .onTapGesture { menuOpen = false }
                            .accessibilityHidden(true)
                    }
                }
                .overlay(alignment: .topTrailing) {
                    if menuOpen && layout.menu, let temporary {
                        TopBarMenu(temporary: temporary, profile: profileWords, connection: connection?.label,
                                   onTemporary: { menuOpen = false; temporary.action() },
                                   onProfile: onProfile.map { f in { menuOpen = false; f() } })
                            .padding(.trailing, 16 + insets.right)
                            .offset(y: 42 + 8)
                            .transition(.opacity)
                    }
                }
        }
        .frame(height: 42)
        .padding(.top, 6)
    }

    /// The profile button's words (and the menu's row for it).
    private var profileWords: String { "This phone" }

    /// The chosen tab's name, as wide as it draws (15 semibold: a fixed size, so it is the same at every text size).
    static func nameWidth(_ t: PhoneTab) -> CGFloat {
        #if canImport(UIKit)
        let font = UIFont.systemFont(ofSize: 15, weight: .semibold)
        return ceil((t.title as NSString).size(withAttributes: [.font: font]).width)
        #else
        return CGFloat(t.title.count) * 8.5
        #endif
    }

    // decision: PHN-801 — the chosen tab's name in the pill; the tabs share what is left (the lab's 46 at most, never
    // under 36); the link's state is a badge on the profile button, whose page holds the connection. As Android's.
    // The temporary chat's bubble (chat tab only) takes its 42 and a gap of 8 from that room. Where that leaves a tab
    // under 36, the bar gives way (TopBarLayout, kb PHN-820).
    private func barRow(layout: TopBarLayout, onTab: ((PhoneTab) -> Void)?, onProfile: (() -> Void)?, temporary: TemporaryToggle?) -> some View {
        let tabW = layout.tabWidth
        let label = connection.map { "\(profileWords). \($0.label)" } ?? profileWords
        return HStack(spacing: 0) {
            HStack(spacing: 0) {
                ForEach(PhoneTab.allCases, id: \.self) { tab in
                    let on = tab == chosen
                    HStack(spacing: 6) {
                        Image(systemName: tab.symbol + (on && tab != .tasks && tab != .mac && tab != .sessions ? ".fill" : ""))
                            .font(.system(size: 17, weight: .medium))
                        if on && layout.named { Text(tab.title).font(.system(size: 15, weight: .semibold)).lineLimit(1).fixedSize() }
                    }
                    .foregroundStyle(on ? Ink.black : Ink.white)
                    .padding(.horizontal, on ? 12 : 0)
                    .frame(width: on ? nil : tabW)
                    .frame(height: 42)
                    .animation(reduce ? FerbMotion.reduced.animation : FerbMotion.enter.animation, value: on)
                    .background { if on { Capsule().fill(Ink.white).matchedGeometryEffect(id: "pill", in: pill) } }
                    .overlay(alignment: .topTrailing) {
                        if waiting == tab, !on {
                            Circle().fill(Ink.amber).frame(width: 8, height: 8)
                                .overlay(Circle().stroke(.black, lineWidth: 2)).offset(x: -8, y: 8)
                        }
                    }
                    .pressable(onTab.map { f in { f(tab) } })
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(Text(tab.title))
                    .accessibilityAddTraits(on ? [.isSelected, .isButton] : .isButton)
                }
            }
            .padding(4)
            .background(Ink.black, in: Capsule())
            // an overshooting pill is cut at the bar's round ends; a point outside them, so the bar's own edge is untouched
            .clipShape(Capsule().inset(by: -1))
            .animation(reduce ? nil : FerbMotion.tab.animation, value: chosen)
            Spacer(minLength: 0)
            if layout.menu, let temporary {
                // folded (kb PHN-820): the bubble and the profile button in one menu button, the badge on it
                TopBarMenuButton(temporaryOn: temporary.on, mark: connection?.mark)
                    .pressable { menuOpen.toggle() }
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(Text(temporary.menuLabel))
                    .accessibilityValue(Text(temporary.on ? temporary.label : ""))
                    .accessibilityIdentifier("top-bar-menu")
            } else {
                roundButtons(onProfile: onProfile, temporary: temporary, label: label)
            }
        }
    }

    @ViewBuilder
    private func roundButtons(onProfile: (() -> Void)?, temporary: TemporaryToggle?, label: String) -> some View {
        if let temporary {
            TemporaryButton(on: temporary.on)
                .opacity(temporary.busy ? 0.6 : 1)
                .pressable(temporary.busy ? nil : temporary.action)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(Text(temporary.label))
                .accessibilityAddTraits(temporary.on ? [.isSelected, .isButton] : .isButton)
                .accessibilityIdentifier("temporary-chat")
                .padding(.trailing, 8)
        }
        ZStack(alignment: .bottomTrailing) {
            Image(systemName: "person.fill")
                .font(.system(size: 17))
                .foregroundStyle(Ink.white)
                .frame(width: 42, height: 42)
            // the link's state as a badge on the profile button (PHN-801; its sheet is a row on the profile page)
            if let connection {
                LinkMarkView(connection.mark, size: 7).frame(width: 14, height: 14).background(Ink.darkFill, in: Circle())
                    .offset(x: -2, y: -2)
            }
        }
        .background(Ink.darkFill, in: Circle())
        .pressable(onProfile)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(label))
    }
}

/// A white card holding rows, PairIt's settings card; on FERB's surface (`onFerb`, kb PHN-734) FERB's dark card.
public struct Card<Content: View>: View {
    var padding: CGFloat = 6
    var content: Content
    @Environment(\.onFerb) private var onFerb
    public init(padding: CGFloat = 6, @ViewBuilder content: () -> Content) {
        self.padding = padding
        self.content = content()
    }
    public var body: some View {
        VStack(spacing: 0) { content }
            .padding(padding)
            .background(CardInk(onFerb).card, in: RoundedRectangle(cornerRadius: 28, style: .continuous))
    }
}

public struct Divide: View {
    @Environment(\.onFerb) private var onFerb
    public init() {}
    public var body: some View { Rectangle().fill(CardInk(onFerb).line).frame(height: 1).padding(.horizontal, 14) }
}

/// FERB's surface (kb PHN-734, the owner: "why not maintain its dark color??/ it consisten - like in mac app"):
/// everything FERB shows in the chat sits on its bubble's black with the on-dark inks; the owner's own messages and
/// the sheets stay white. Pieces read it from the environment (`onFerb`), so no card picks a colour of its own.
private struct OnFerbKey: EnvironmentKey { static let defaultValue = false }
public extension EnvironmentValues {
    var onFerb: Bool { get { self[OnFerbKey.self] } set { self[OnFerbKey.self] = newValue } }
}
public extension View {
    /// Draws what is inside on FERB's dark surface.
    func ferbSurface() -> some View { environment(\.onFerb, true) }
}

/// The inks of a card: the lab's white card, or FERB's dark surface. Text contrast on black: white 21:1, secondary
/// #9A9A9A 7.4:1, faint #8E8E8E 6.4:1, amber #FFB020 11:1, red #FF6B5E 6.9:1, link #9CC7FF 12:1.
public struct CardInk: Sendable {
    public let onFerb: Bool
    public init(_ onFerb: Bool) { self.onFerb = onFerb }
    public var card: Color { onFerb ? Ink.black : Ink.white }
    public var primary: Color { onFerb ? .white : Ink.black }
    public var secondary: Color { onFerb ? Ink.onDark : Ink.detail }
    public var faint: Color { onFerb ? Color(hex: 0x8E8E8E) : Ink.faint }
    public var fill: Color { onFerb ? Ink.darkFill : Ink.fill }
    public var line: Color { onFerb ? Color.white.opacity(0.12) : Ink.line }
    public var red: Color { onFerb ? Color(hex: 0xFF6B5E) : Ink.red }
    public var link: Color { onFerb ? Color(hex: 0x9CC7FF) : Color(hex: 0x1F6FEB) }
    public var amberText: Color { onFerb ? Ink.amber : Color(hex: 0xB86E00) }
}

/// One row of a card: an icon or a mark, a title and its detail, and what is on the right.
public struct CardRow<Leading: View, Trailing: View>: View {
    @Environment(\.onFerb) private var onFerb
    let title: String
    var detail: String? = nil
    var danger = false
    var onClick: (() -> Void)?
    var leading: Leading
    var trailing: Trailing

    public init(title: String, detail: String? = nil, danger: Bool = false, onClick: (() -> Void)? = nil,
                @ViewBuilder leading: () -> Leading, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.detail = detail
        self.danger = danger
        self.onClick = onClick
        self.leading = leading()
        self.trailing = trailing()
    }

    public var body: some View {
        HStack(spacing: 14) {
            leading.frame(width: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).type(16.5, .semibold, danger ? CardInk(onFerb).red : CardInk(onFerb).primary).lineLimit(2)
                if let detail { Text(detail).type(14, .regular, CardInk(onFerb).secondary).lineLimit(1) }
            }
            Spacer(minLength: 8)
            trailing
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 13)
        .pressable(onClick)
        .accessibilityElement(children: .combine)
    }
}

public extension CardRow where Trailing == EmptyView {
    init(title: String, detail: String? = nil, danger: Bool = false, onClick: (() -> Void)? = nil, @ViewBuilder leading: () -> Leading) {
        self.init(title: title, detail: detail, danger: danger, onClick: onClick, leading: leading, trailing: { EmptyView() })
    }
}

public struct RowIcon: View {
    let symbol: String
    var danger = false
    @Environment(\.onFerb) private var onFerb
    public init(symbol: String, danger: Bool = false) { self.symbol = symbol; self.danger = danger }
    public var body: some View { Image(systemName: symbol).font(.system(size: 20)).foregroundStyle(danger ? CardInk(onFerb).red : CardInk(onFerb).primary) }
}

public struct Opens: View {
    var value: String? = nil
    @Environment(\.onFerb) private var onFerb
    public init(value: String? = nil) { self.value = value }
    public var body: some View {
        HStack(spacing: 5) {
            if let value { Text(value).type(15, .medium, CardInk(onFerb).faint) }
            Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(CardInk(onFerb).faint)
        }
    }
}

/// The state of a task or a part (Mark.Kind in the lab).
public enum MarkKind: Sendable {
    case done, asking, failed, running, waiting
    /// What a screen reader says (not the lab's: it has no spoken words; Android's MarkKind.spoken).
    public var spoken: String {
        switch self {
        case .done: return "Done"
        case .asking: return "Waiting for you"
        case .failed: return "Couldn't finish"
        case .running: return "Running"
        case .waiting: return "Not started"
        }
    }
}

/// The state of a task or a part: solid shapes, so colour is never the only sign.
public struct Mark: View {
    public typealias Kind = MarkKind
    let kind: Kind
    var size: CGFloat = 26
    @Environment(\.onFerb) private var onFerb

    public init(kind: Kind, size: CGFloat = 26) { self.kind = kind; self.size = size }

    public var body: some View {
        ZStack {
            switch kind {
            case .done:
                Circle().fill(onFerb ? Color.white : Ink.black)
                Image(systemName: "checkmark").font(.system(size: size * 0.45, weight: .bold)).foregroundStyle(onFerb ? Ink.black : .white)
            case .asking:
                Circle().fill(Ink.amber)
                Text("?").font(.system(size: size * 0.55, weight: .bold)).foregroundStyle(.black)
            case .failed:
                Circle().fill(Ink.red)
                Text("!").font(.system(size: size * 0.55, weight: .bold)).foregroundStyle(.white)
            case .running:
                MotionClock { t in
                    let turn = t.truncatingRemainder(dividingBy: 0.8) / 0.8
                    Circle().trim(from: 0, to: 0.3)
                        .stroke(onFerb ? Color.white : Ink.black, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                        .rotationEffect(.degrees(turn * 360))
                        .padding(2)
                }
            case .waiting:
                Circle().strokeBorder(LabColors.waitingRing, style: StrokeStyle(lineWidth: 2, dash: [3, 3]))
            }
        }
        .frame(width: size, height: size)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(kind.spoken))
    }
}

/// Mobile.swift Pill.Kind.
public enum PillKind: Sendable { case black, grey, plain }

/// A big button. Black is the one thing to do; grey is the other choice; plain is a way out.
public struct Pill: View {
    public typealias Kind = PillKind
    let title: String
    var kind: Kind = .black
    var symbol: String? = nil
    var height: CGFloat = 52
    /// Not from the lab: false lets the pill take only its words' width (28 each side, Android's Pill(fill = false)).
    var fill = true
    var onClick: (() -> Void)?

    public init(title: String, kind: Kind = .black, symbol: String? = nil, height: CGFloat = 52, fill: Bool = true,
                onClick: (() -> Void)? = nil) {
        self.title = title
        self.kind = kind
        self.symbol = symbol
        self.height = height
        self.fill = fill
        self.onClick = onClick
    }

    @Environment(\.onFerb) private var onFerb

    public var body: some View {
        // on FERB's surface the one thing to do is white on black's inverse, the other choice the dark fill
        let primary = onFerb ? (kind == .black ? Ink.black : Ink.white) : (kind == .black ? Ink.white : Ink.black)
        return HStack(spacing: 9) {
            if let symbol { Image(systemName: symbol).font(.system(size: 17, weight: .medium)) }
            Text(title).font(.system(size: 16, weight: .semibold))
        }
        .foregroundStyle(primary)
        .fixedSize(horizontal: !fill, vertical: false)
        .padding(.horizontal, fill ? 0 : 28)
        .frame(maxWidth: fill ? .infinity : nil)
        .frame(height: height)
        .background {
            switch kind {
            case .black: RoundedRectangle(cornerRadius: 16, style: .continuous).fill(onFerb ? Ink.white : Ink.black)
            case .grey: RoundedRectangle(cornerRadius: 16, style: .continuous).fill(onFerb ? Ink.darkFill : Ink.fill)
            case .plain: EmptyView()
            }
        }
        .pressable(onClick)
    }
}

/// The microphone's state as the composer draws it (kb PHN-643; Android's MicState).
public enum MicState: Sendable { case idle, readying, listening, finishing }

public struct PhoneComposer: View {
    var placeholder = "Ask FERB"
    /// What the owner has typed so far, shown with its caret; nil shows the placeholder.
    var typed: String? = nil
    /// Not from the lab: with a binding the composer is a working text field (Android's onValueChange).
    var text: Binding<String>?
    var onSend: (() -> Void)?
    var onAttach: (() -> Void)?
    var onMic: (() -> Void)?
    /// Not from the lab, hold to speak (kb PHN-603; Android's onMicDown/onMicUp): with `onMicDown` the microphone works
    /// while it is held, `onMicUp` when the finger lifts or the touch is taken away; `onMic` is then unused. While
    /// `listening` the microphone sits in a white circle (the send button's) and an empty box reads `listeningText`.
    /// The defaults draw as before.
    var onMicDown: (() -> Void)?
    var onMicUp: (() -> Void)?
    var listening = false
    var listeningText: String?
    var micLabel = "Speak"
    /// Not from the lab (kb PHN-643): getting ready, listening (with a ring that grows with `micLevel`, 0...1), finishing.
    var micState: MicState = .idle
    var micLevel: Float = 0
    /// Not from the lab (kb PHN-690): files waiting in the box, drawn inside it above the words; the capsule becomes a card.
    var attachments: AnyView? = nil
    /// Not from the lab: lets the app put the cursor in the box (a tap on FERB's question's notification, kb PHN-739).
    var focus: FocusState<Bool>.Binding? = nil
    @GestureState private var held = false

    public init(placeholder: String = "Ask FERB", typed: String? = nil, text: Binding<String>? = nil,
                onSend: (() -> Void)? = nil, onAttach: (() -> Void)? = nil, onMic: (() -> Void)? = nil,
                onMicDown: (() -> Void)? = nil, onMicUp: (() -> Void)? = nil, listening: Bool = false,
                listeningText: String? = nil, micLabel: String = "Speak", micState: MicState? = nil, micLevel: Float = 0,
                attachments: AnyView? = nil, focus: FocusState<Bool>.Binding? = nil) {
        self.focus = focus
        self.placeholder = placeholder
        self.typed = typed
        self.text = text
        self.onSend = onSend
        self.onAttach = onAttach
        self.onMic = onMic
        self.onMicDown = onMicDown
        self.onMicUp = onMicUp
        self.listening = listening
        self.listeningText = listeningText
        self.micLabel = micLabel
        self.micState = micState ?? (listening ? .listening : .idle)
        self.micLevel = micLevel
        self.attachments = attachments
    }

    @ViewBuilder private var mic: some View {
        if let onMicDown {
            // a lighter circle the moment the finger goes down (before FERB answers), then the state FERB says
            let shown: MicState = micState == .idle && held ? .readying : micState
            let level = shown == .listening ? CGFloat(min(max(micLevel, 0), 1)) : 0
            let grow: CGFloat = shown == .idle ? 1 : 1.12
            Image(systemName: "mic").font(.system(size: 19)).foregroundStyle(shown == .idle ? .white : .black)
                // the circle and its ring sit behind the icon, so the composer's layout is the lab's in every state
                .background {
                    ZStack {
                        switch shown {
                        case .idle: EmptyView()
                        case .readying: Circle().fill(Ink.white.opacity(0.55)).frame(width: 40 * grow, height: 40 * grow)
                        case .listening:
                            Circle().fill(Ink.white.opacity(0.22 + 0.2 * level)).frame(width: 40 * (grow + 0.08 + 0.25 * level),
                                                                                       height: 40 * (grow + 0.08 + 0.25 * level))
                            Circle().fill(Ink.white).frame(width: 40 * grow, height: 40 * grow)
                        case .finishing: Circle().fill(Ink.white.opacity(0.8)).frame(width: 40, height: 40)
                        }
                    }
                    .animation(.easeOut(duration: 0.09), value: level)
                    .animation(.easeOut(duration: 0.12), value: shown)
                }
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0)
                    .updating($held) { _, state, _ in state = true })
                .onChange(of: held) { _, now in if now { onMicDown() } else { onMicUp?() } }
                .accessibilityElement()
                .accessibilityLabel(Text(micLabel))
                .accessibilityAddTraits(.isButton)
        } else {
            Image(systemName: "mic").font(.system(size: 19)).foregroundStyle(.white)
                .pressable(onMic, label: micLabel)
        }
    }

    public var body: some View {
        if let attachments {
            VStack(spacing: 0) {
                attachments.padding(.horizontal, 8).padding(.top, 8)
                row.padding(.horizontal, 8).padding(.vertical, 8).frame(minHeight: 56)
            }
            .background(Ink.black, in: RoundedRectangle(cornerRadius: 28, style: .continuous))
        } else {
            row
                .padding(.horizontal, 8)
                .padding(.vertical, text == nil ? 0 : 8)
                .frame(minHeight: 56)
                .frame(height: text == nil ? 56 : nil)
                .background(Ink.black, in: Capsule())
        }
    }

    private var row: some View {
        HStack(spacing: 10) {
            Image(systemName: "plus").font(.system(size: 18, weight: .medium)).foregroundStyle(.white)
                .frame(width: 40, height: 40).background(Ink.darkFill, in: Circle())
                .pressable(onAttach, label: "Attach")
            if let text {
                ZStack(alignment: .leading) {
                    if text.wrappedValue.isEmpty {
                        Text(listening ? listeningText ?? placeholder : placeholder).type(16.5, .regular, Ink.onDark).lineLimit(1)
                    }
                    Group {
                        if let focus {
                            TextField("", text: text, axis: .vertical).focused(focus)
                        } else {
                            TextField("", text: text, axis: .vertical)
                        }
                    }
                    .lineLimit(1...4).type(16.5, .regular, .white).tint(Ink.onDark)
                    .accessibilityLabel(Text(placeholder))
                }
            } else if let typed, !typed.isEmpty {
                (Text(typed) + Text("|").foregroundColor(Ink.onDark)).type(16.5, .regular, .white).lineLimit(1).truncationMode(.head)
            } else {
                Text(placeholder).type(16.5, .regular, Ink.onDark).lineLimit(1)
            }
            Spacer()
            mic
            Image(systemName: "arrow.up").font(.system(size: 17, weight: .semibold)).foregroundStyle(.black)
                .frame(width: 40, height: 40).background(Ink.white, in: Circle())
                .pressable(onSend, label: "Send")
        }
    }
}

/// A label that sits straight on the sky.
public struct SkyLabel: View {
    @Environment(\.skyInk) private var ink
    let text: String
    public init(text: String) { self.text = text }
    public var body: some View {
        Text(text).type(15, .semibold, ink.low).padding(.horizontal, 8).padding(.top, 22).padding(.bottom, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityAddTraits(.isHeader)
    }
}

/// What slides up: a white sheet, inset from the screen's edges, over a dimmed page.
public struct Sheet<Content: View>: View {
    var lift: CGFloat = 0
    /// The lab's os.bottom: the home indicator's area.
    var bottomInset: CGFloat = PhoneInsets.lab.bottom
    /// Not from the lab: a tap on the dimmed page closes a sheet that can be closed.
    var onDismiss: (() -> Void)?
    var content: Content

    public init(lift: CGFloat = 0, bottomInset: CGFloat = PhoneInsets.lab.bottom, onDismiss: (() -> Void)? = nil,
                @ViewBuilder content: () -> Content) {
        self.lift = lift
        self.bottomInset = bottomInset
        self.onDismiss = onDismiss
        self.content = content()
    }

    // Shown by a sheet switch (Motion.swift), the dim fades and the panel comes up from below by its own height and goes
    // back down; a dismissible one follows a drag down and closes past a quarter of its height or on a fling, else
    // springs back (kb PHN-623). Under Reduce Motion it only fades. At rest (or with no switch) it is drawn as before.
    @Environment(\.motionPresence) private var presence
    @Environment(\.accessibilityReduceMotion) private var reduce
    @State private var drag: CGFloat = 0
    @State private var panelHeight: CGFloat = 1

    public var body: some View {
        let p = presence?.value ?? 1
        let still = presence == nil || reduce
        let pulled = drag
        ZStack(alignment: .bottom) {
            Color.black.opacity(FerbMotion.scrim)
                .opacity(min(1, max(0, p)))
                .contentShape(Rectangle())
                .onTapGesture { onDismiss?() }
                .accessibilityHidden(onDismiss == nil)
                .accessibilityAddTraits(.isButton)
            VStack(alignment: .leading, spacing: 0) {
                Capsule().fill(LabColors.grabber).frame(width: 38, height: 5).frame(maxWidth: .infinity).padding(.top, 10).padding(.bottom, 16)
                    .accessibilityHidden(true)
                content
            }
            .padding(.horizontal, 22)
            .padding(.bottom, lift > 0 ? 22 : bottomInset + 4)
            .background(Ink.white, in: RoundedRectangle(cornerRadius: lift > 0 ? 32 : 44, style: .continuous))
            .padding(.horizontal, 8)
            .padding(.bottom, 8 + lift)
            .opacity(still ? min(1, max(0, p)) : 1)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { panelHeight = max(1, $0) }
            .visualEffect { view, proxy in
                view.offset(y: still ? 0 : (1 - p) * proxy.size.height + pulled)
            }
            .gesture(dismissDrag, including: onDismiss == nil || presence == nil ? .subviews : .all)
            .accessibilityAddTraits(.isModal)
        }
    }

    private var dismissDrag: some Gesture {
        DragGesture(minimumDistance: 8)
            .onChanged { v in drag = max(0, v.translation.height) }
            .onEnded { v in
                if drag > panelHeight * FerbMotion.dismissFraction || v.velocity.height > FerbMotion.dismissVelocity {
                    onDismiss?()
                } else {
                    withAnimation(FerbMotion.sheet.animation) { drag = 0 }
                }
            }
    }
}

public struct SheetIcon: View {
    let symbol: String
    var asking = true
    public init(symbol: String, asking: Bool = true) { self.symbol = symbol; self.asking = asking }
    public var body: some View {
        Image(systemName: symbol).font(.system(size: 20, weight: .medium)).foregroundStyle(.black)
            .frame(width: 46, height: 46)
            .background(Ink.fill, in: RoundedRectangle(cornerRadius: 15, style: .continuous))
            .overlay(alignment: .topTrailing) {
                if asking { Circle().fill(Ink.amber).frame(width: 12, height: 12).overlay(Circle().stroke(.white, lineWidth: 3)).offset(x: 3, y: -3) }
            }
    }
}

public struct CloseButton: View {
    var onClick: (() -> Void)?
    public init(onClick: (() -> Void)? = nil) { self.onClick = onClick }
    public var body: some View {
        Image(systemName: "xmark").font(.system(size: 14, weight: .bold)).foregroundStyle(.black)
            .frame(width: 36, height: 36).background(Ink.fill, in: Circle())
            .pressable(onClick, label: "Close")
    }
}

/// Mobile.swift Switch (PhoneSwitch: Android's name).
public struct PhoneSwitch: View {
    var on = true
    /// Not from the lab: with it the switch turns on a tap, on Theme.swift GlassSwitch's spring.
    var onChange: ((Bool) -> Void)?
    var label: String?
    public init(on: Bool = true, onChange: ((Bool) -> Void)? = nil, label: String? = nil) {
        self.on = on
        self.onChange = onChange
        self.label = label
    }
    public var body: some View {
        ZStack(alignment: on ? .trailing : .leading) {
            Capsule().fill(on ? Ink.black : LabColors.switchOff)
            Circle().fill(.white).frame(width: 24, height: 24).padding(4)
        }
        .frame(width: 52, height: 32)
        .animation(.spring(duration: 0.25, bounce: 0.2), value: on)
        .contentShape(Capsule())
        .onTapGesture { onChange?(!on) }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(label ?? ""))
        .accessibilityValue(Text(on ? "On" : "Off"))
        .accessibilityAddTraits(onChange == nil ? [] : .isButton)
    }
}

/// The lab's name for the switch.
public typealias Switch = PhoneSwitch
