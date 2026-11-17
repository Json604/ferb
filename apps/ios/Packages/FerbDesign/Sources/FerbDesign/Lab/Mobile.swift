// Copied from ~/Desktop/ferb-glass-lab Sources/FerbGlassLab/Mobile.swift at 23a0fde (LabSource.commit), as a library.
// Changed: everything the app uses is public; the lab's scaffolding is gone (OS, Device, StatusBar, LabFonts and
// Roboto: on the iPhone the system draws the status bar and the face is the system's); `os.top` and `os.bottom` are
// PhoneInsets (the lab's iPhone numbers by default); `Tab` is PhoneTab (SwiftUI has a Tab of its own) and the kinds of
// Mark, Pill and LinkPicture are MarkKind, PillKind and LinkKind (Android's names; Mark.Kind and Pill.Kind stay as
// aliases); each piece takes optional callbacks (Android's names, kb PHN-509) whose absence draws exactly the lab's
// piece; the running mark reads the motion clock (Motion.swift) so a still picture can be made. Every drawn number
// and colour is the lab's.

import SwiftUI

// The phone app: one design, drawn on an iPhone and on an Android phone. Only the hardware differs (the island or
// the camera hole, the status bar); everything inside is the same, so Android can copy the iOS version exactly.

/// The system's bars, the lab's `os.top` and `os.bottom`. The app passes the real ones.
public struct PhoneInsets: Equatable, Sendable {
    public var top: CGFloat
    public var bottom: CGFloat
    public init(top: CGFloat, bottom: CGFloat) { self.top = top; self.bottom = bottom }
    /// Mobile.swift OS.ios: a 58 status bar and a 30 home-indicator area, on the lab's 402 by 874 iPhone.
    public static let lab = PhoneInsets(top: 58, bottom: 30)
    /// Mobile.swift OS.ios.size: the iPhone the lab draws.
    public static let labSize = CGSize(width: 402, height: 874)
}

private struct SkyInkKey: EnvironmentKey { static let defaultValue = SkyInk(top: .white, low: .white) }

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

/// A press that sinks to 0.97 and 85 % on the press spring (Theme.swift FerbButton's numbers).
public struct PressStyle: ButtonStyle {
    public init() {}
    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .contentShape(Rectangle())
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(.spring(duration: 0.2, bounce: 0.15), value: configuration.isPressed)
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

/// Mobile.swift `Tab`: the four pages.
public enum PhoneTab: CaseIterable, Hashable, Sendable {
    case chat, tasks, files, mac
    public var title: String { ["Chat", "Tasks", "Files", "Mac"][PhoneTab.allCases.firstIndex(of: self)!] }
    public var symbol: String { ["bubble.left", "checklist", "folder", "laptopcomputer"][PhoneTab.allCases.firstIndex(of: self)!] }
}

/// The top of every page: a short black pill of tabs (only the chosen one shows its name) and the profile button.
public struct TopBar: View {
    let chosen: PhoneTab
    var waiting: PhoneTab? = .tasks
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?

    public init(chosen: PhoneTab, waiting: PhoneTab? = .tasks, onTab: ((PhoneTab) -> Void)? = nil, onProfile: (() -> Void)? = nil) {
        self.chosen = chosen
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
    }

    public var body: some View {
        HStack {
            HStack(spacing: 2) {
                ForEach(PhoneTab.allCases, id: \.self) { tab in
                    let on = tab == chosen
                    HStack(spacing: 7) {
                        Image(systemName: tab.symbol + (on && tab != .tasks && tab != .mac ? ".fill" : ""))
                            .font(.system(size: 17, weight: .medium))
                        if on { Text(tab.title).font(.system(size: 15, weight: .semibold)) }
                    }
                    .foregroundStyle(on ? Ink.black : Ink.white)
                    .padding(.horizontal, on ? 15 : 12)
                    .frame(minWidth: 46)
                    .frame(height: 42)
                    .background { if on { Capsule().fill(Ink.white) } }
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
            Spacer()
            Image(systemName: "person.fill")
                .font(.system(size: 17))
                .foregroundStyle(Ink.white)
                .frame(width: 42, height: 42)
                .background(Ink.darkFill, in: Circle())
                .pressable(onProfile)
                .accessibilityLabel(Text("This phone"))
        }
        .padding(.horizontal, 16)
        .padding(.top, 6)
    }
}

/// A white card holding rows, PairIt's settings card.
public struct Card<Content: View>: View {
    var padding: CGFloat = 6
    var content: Content
    public init(padding: CGFloat = 6, @ViewBuilder content: () -> Content) {
        self.padding = padding
        self.content = content()
    }
    public var body: some View {
        VStack(spacing: 0) { content }
            .padding(padding)
            .background(Ink.white, in: RoundedRectangle(cornerRadius: 28, style: .continuous))
    }
}

public struct Divide: View {
    public init() {}
    public var body: some View { Rectangle().fill(Ink.line).frame(height: 1).padding(.horizontal, 14) }
}

/// One row of a card: an icon or a mark, a title and its detail, and what is on the right.
public struct CardRow<Leading: View, Trailing: View>: View {
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
                Text(title).type(16.5, .semibold, danger ? Ink.red : Ink.black).lineLimit(2)
                if let detail { Text(detail).type(14, .regular, Ink.detail).lineLimit(1) }
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
    public init(symbol: String, danger: Bool = false) { self.symbol = symbol; self.danger = danger }
    public var body: some View { Image(systemName: symbol).font(.system(size: 20)).foregroundStyle(danger ? Ink.red : Ink.black) }
}

public struct Opens: View {
    var value: String? = nil
    public init(value: String? = nil) { self.value = value }
    public var body: some View {
        HStack(spacing: 5) {
            if let value { Text(value).type(15, .medium, Ink.faint) }
            Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(Ink.faint)
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

    public init(kind: Kind, size: CGFloat = 26) { self.kind = kind; self.size = size }

    public var body: some View {
        ZStack {
            switch kind {
            case .done:
                Circle().fill(Ink.black)
                Image(systemName: "checkmark").font(.system(size: size * 0.45, weight: .bold)).foregroundStyle(.white)
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
                        .stroke(Ink.black, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
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

    public var body: some View {
        HStack(spacing: 9) {
            if let symbol { Image(systemName: symbol).font(.system(size: 17, weight: .medium)) }
            Text(title).font(.system(size: 16, weight: .semibold))
        }
        .foregroundStyle(kind == .black ? Ink.white : Ink.black)
        .fixedSize(horizontal: !fill, vertical: false)
        .padding(.horizontal, fill ? 0 : 28)
        .frame(maxWidth: fill ? .infinity : nil)
        .frame(height: height)
        .background {
            switch kind {
            case .black: RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Ink.black)
            case .grey: RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Ink.fill)
            case .plain: EmptyView()
            }
        }
        .pressable(onClick)
    }
}

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
    @GestureState private var held = false

    public init(placeholder: String = "Ask FERB", typed: String? = nil, text: Binding<String>? = nil,
                onSend: (() -> Void)? = nil, onAttach: (() -> Void)? = nil, onMic: (() -> Void)? = nil,
                onMicDown: (() -> Void)? = nil, onMicUp: (() -> Void)? = nil, listening: Bool = false,
                listeningText: String? = nil, micLabel: String = "Speak") {
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
    }

    @ViewBuilder private var mic: some View {
        if let onMicDown {
            Image(systemName: "mic").font(.system(size: 19)).foregroundStyle(listening ? .black : .white)
                // the white circle sits behind the icon, so the composer's layout is the lab's whether or not it listens
                .background { if listening { Circle().fill(Ink.white).frame(width: 40, height: 40) } }
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
        HStack(spacing: 10) {
            Image(systemName: "plus").font(.system(size: 18, weight: .medium)).foregroundStyle(.white)
                .frame(width: 40, height: 40).background(Ink.darkFill, in: Circle())
                .pressable(onAttach, label: "Attach")
            if let text {
                ZStack(alignment: .leading) {
                    if text.wrappedValue.isEmpty {
                        Text(listening ? listeningText ?? placeholder : placeholder).type(16.5, .regular, Ink.onDark).lineLimit(1)
                    }
                    TextField("", text: text, axis: .vertical).lineLimit(1...4).type(16.5, .regular, .white).tint(Ink.onDark)
                        .accessibilityLabel(Text(placeholder))
                }
            } else if let typed, !typed.isEmpty {
                (Text(typed) + Text("|").foregroundColor(Ink.onDark)).type(16.5, .regular, .white).lineLimit(1).truncationMode(.head)
            } else {
                Text(placeholder).type(16.5, .regular, Ink.onDark)
            }
            Spacer()
            mic
            Image(systemName: "arrow.up").font(.system(size: 17, weight: .semibold)).foregroundStyle(.black)
                .frame(width: 40, height: 40).background(Ink.white, in: Circle())
                .pressable(onSend, label: "Send")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, text == nil ? 0 : 8)
        .frame(minHeight: 56)
        .frame(height: text == nil ? 56 : nil)
        .background(Ink.black, in: Capsule())
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

    public var body: some View {
        ZStack(alignment: .bottom) {
            Color.black.opacity(0.5)
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
            .accessibilityAddTraits(.isModal)
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
