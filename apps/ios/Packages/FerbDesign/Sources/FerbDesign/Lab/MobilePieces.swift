// The pieces the lab draws inside its phone screens, taken out of ~/Desktop/ferb-glass-lab
// Sources/FerbGlassLab/MobileScreens.swift (and Conversation.swift for TypingDots and the cart question's item) at
// 23a0fde (LabSource.commit), so the app can use them on their own: the bubbles, the task card, the progress card,
// the link picture, the pairing code and steps, the exact thing a question is about, the private code's boxes and
// keypad, a sheet's title. Each says which lab view it was; its drawing is that view's, numbers and all. Their data
// and callbacks are parameters (Android's names, kb PHN-509) whose defaults draw the lab's sample.

import SwiftUI

// MARK: The conversation (MobileScreens.swift ChatPage, Conversation.swift)

/// ChatPage.mine: the owner's words on white, the tail corner at the bottom right.
public struct OwnerBubble: View {
    let text: String
    /// Not from the lab (kb APP-1146): the message this one quotes, above the bubble. The bubble also swipes to reply
    /// and lights up as a quoted message when the row says so (`bubbleReply`).
    var quote: QuoteShown? = nil
    public init(_ text: String, quote: QuoteShown? = nil) { self.text = text; self.quote = quote }
    public var body: some View {
        let shape = UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 22, bottomTrailingRadius: 6, topTrailingRadius: 22, style: .continuous)
        VStack(alignment: .trailing, spacing: 4) {
            if let quote { SentQuote(quote) }
            Text(text).type(16, .regular, .black).lineSpacing(2)
                .padding(.horizontal, 16).padding(.vertical, 11)
                .background(Ink.white, in: shape)
                .modifier(Quotable(shape: shape))
                .frame(maxWidth: 310, alignment: .trailing)
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
    }
}

/// ChatPage.ferb: FERB's words on black, the tail corner at the bottom left. The home screen's line is 300 wide at
/// most (HomePage), the chat's 320.
public struct FerbBubble: View {
    let text: String
    var maxWidth: CGFloat = 320
    public init(_ text: String, maxWidth: CGFloat = 320) { self.text = text; self.maxWidth = maxWidth }
    public var body: some View {
        Text(text).type(16, .regular, .white).lineSpacing(2)
            .padding(.horizontal, 16).padding(.vertical, 11)
            .background(Ink.black, in: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 6, bottomTrailingRadius: 22, topTrailingRadius: 22, style: .continuous))
            .modifier(Quotable(shape: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 6, bottomTrailingRadius: 22, topTrailingRadius: 22, style: .continuous)))
            .frame(maxWidth: maxWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Conversation.swift TypingDots: FERB writing, three dots in a black bubble, rising one after another.
public struct TypingDots: View {
    public init() {}
    public var body: some View {
        MotionClock { t in
            HStack(spacing: 5) {
                ForEach(0..<3, id: \.self) { i in
                    Circle().fill(.white.opacity(0.85)).frame(width: 7, height: 7)
                        .offset(y: -3 * max(0, sin(t * 6 - Double(i) * 0.7)))
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 15)
            .background(Ink.black, in: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 6, bottomTrailingRadius: 22, topTrailingRadius: 22, style: .continuous))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// One part of a task: its mark, its title and what it is doing.
public struct PlanPart: Equatable, Sendable {
    public var kind: MarkKind
    public var title: String
    public var detail: String?
    /// Not from the lab (kb PHN-733; FERB.app's CardRowView): the part's own answer in rich text, folded past 8 lines
    /// to 3 with "Show more"; the colour its plain words take; its lines under them ("Couldn't confirm", "Updated", "You
    /// chose"); whether it was updated; its key (where its fold is kept).
    public var rich: [RichBlock]? = nil
    public var tone: PartTone = .dim
    public var notes: [PartNote] = []
    public var updated = false
    public var key = ""
    /// The steps a waiting part waits on that no part answers (APP-895), each its own line under it.
    public var sub: [PlanPart] = []
    public init(kind: MarkKind, title: String, detail: String? = nil, rich: [RichBlock]? = nil, tone: PartTone = .dim, notes: [PartNote] = [],
                updated: Bool = false, key: String = "", sub: [PlanPart] = []) {
        self.kind = kind; self.title = title; self.detail = detail
        self.rich = rich; self.tone = tone; self.notes = notes; self.updated = updated; self.key = key; self.sub = sub
    }
}

/// The colour a part's words take (FERB.app's row content: red for a failure, amber when it needs you, the quieter
/// grey otherwise).
public enum PartTone: Sendable { case dim, red, amber, blue }

/// A line under a part's words: a label in its tone ("Couldn't confirm: ") and the words after it.
public struct PartNote: Equatable, Sendable {
    public var label: String?, text: String, tone: PartTone
    public init(label: String? = nil, text: String, tone: PartTone = .dim) { self.label = label; self.text = text; self.tone = tone }
}

extension PartTone {
    func color(_ onFerb: Bool) -> Color {
        let ink = CardInk(onFerb)
        switch self {
        case .dim: return ink.secondary
        case .red: return ink.red
        case .amber: return ink.amberText
        case .blue: return ink.link
        }
    }
}

/// ChatPage.task: the plan line and its count, the parts (a running one on a grey fill), and Watch live and Stop
/// while anything runs or asks (Conversation.swift's rule; the lab's still chat always runs).
public struct TaskCard: View {
    let headline: String
    let progress: String
    let parts: [PlanPart]
    var watchTitle = "Watch live"
    var stopTitle = "Stop"
    var onWatch: (() -> Void)?
    var onStop: (() -> Void)?
    /// Not from the lab (kb PHN-726): FERB.app's running status line, and once ended its footer and code's lines.
    var status: String? = nil
    var footer: String? = nil
    var footerLines: [String] = []
    /// Not from the lab (kb PHN-726): which parts show their own live picture (FERB.app's RowPreview), its picture once
    /// one came, and what a tap on it does.
    var watching: [Bool] = []
    var pictures: [UIImage?] = []
    var onPicture: ((Int) -> Void)? = nil
    @Environment(\.onFerb) private var onFerb

    public init(headline: String, progress: String, parts: [PlanPart], watchTitle: String = "Watch live", stopTitle: String = "Stop",
                onWatch: (() -> Void)? = nil, onStop: (() -> Void)? = nil, status: String? = nil, footer: String? = nil, footerLines: [String] = [],
                watching: [Bool] = [], pictures: [UIImage?] = [], onPicture: ((Int) -> Void)? = nil) {
        self.watching = watching
        self.pictures = pictures
        self.onPicture = onPicture
        self.status = status
        self.footer = footer
        self.footerLines = footerLines
        self.headline = headline
        self.progress = progress
        self.parts = parts
        self.watchTitle = watchTitle
        self.stopTitle = stopTitle
        self.onWatch = onWatch
        self.onStop = onStop
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(headline).type(14, .medium, CardInk(onFerb).secondary)
                Spacer()
                Text(progress).type(14, .medium, CardInk(onFerb).secondary).monospacedDigit()
            }
            .padding(.horizontal, 10).padding(.top, 8).padding(.bottom, 4)
            ForEach(Array(parts.enumerated()), id: \.offset) { i, p in
                part(p, watching: i < watching.count && watching[i], picture: i < pictures.count ? pictures[i] : nil, index: i)
                    .background(p.kind == .running ? CardInk(onFerb).fill : .clear, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            }
            if let status {
                Text(status).type(13.5, .medium, CardInk(onFerb).secondary).lineLimit(1).padding(.horizontal, 10).padding(.top, 6)
            }
            if footer != nil || !footerLines.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    if let footer { Text(footer).type(14.5, .semibold, CardInk(onFerb).primary) }
                    ForEach(footerLines, id: \.self) { Text($0).type(13.5, .regular, CardInk(onFerb).secondary) }
                }
                .padding(.horizontal, 10).padding(.top, 8).padding(.bottom, 4)
            }
            if parts.contains(where: { $0.kind == .running || $0.kind == .asking }) {
                HStack(spacing: 8) {
                    Pill(title: watchTitle, kind: .grey, symbol: "eye", height: 46, onClick: onWatch)
                    Pill(title: stopTitle, kind: .black, symbol: "stop.fill", height: 46, onClick: onStop)
                }
                .padding(.top, 6).padding(.horizontal, 2)
            }
        }
        .padding(8)
        .background(CardInk(onFerb).card, in: RoundedRectangle(cornerRadius: 26, style: .continuous))
    }

    private func part(_ p: PlanPart, watching: Bool = false, picture: UIImage? = nil, index: Int = 0) -> some View {
        let (kind, title) = (p.kind, p.title)
        return HStack(alignment: .top, spacing: 12) {
            Mark(kind: kind)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text(title).type(15.5, kind == .waiting ? .regular : .semibold, kind == .waiting ? CardInk(onFerb).faint : CardInk(onFerb).primary)
                    if p.updated {
                        Text("UPDATED").font(.system(size: 10, weight: .bold)).tracking(0.5).foregroundStyle(.white)
                            .padding(.horizontal, 5).background(Color(hex: 0x1F6FEB), in: RoundedRectangle(cornerRadius: 4))
                    }
                }
                if let detail = p.detail { Text(detail).type(14, .regular, p.tone.color(onFerb)) }
                if let rich = p.rich {
                    // the part's own answer as FERB.app's row shows it: rich text, past 8 lines folded to 3
                    let look = RichLook(size: 14.5, ink: CardInk(onFerb).primary, dim: CardInk(onFerb).secondary, link: CardInk(onFerb).link, codeFill: CardInk(onFerb).fill)
                    RichWords(richText(rich, look), look: look,
                              key: p.key, fold: .row, more: "Show more", width: 250)
                }
                ForEach(Array(p.notes.enumerated()), id: \.offset) { _, n in
                    (Text(n.label ?? "").fontWeight(.semibold).foregroundColor(n.tone.color(onFerb)) + Text(n.text).foregroundColor(n.label == nil ? n.tone.color(onFerb) : CardInk(onFerb).secondary))
                        .font(.system(size: 13.5))
                }
            }
            Spacer(minLength: 0)
            if watching {
                // the row's own live picture (FERB.app's RowPreview: 22 points, the amber ring, no dot)
                Button { onPicture?(index) } label: {
                    Group { if let picture { Image(uiImage: picture).resizable().scaledToFill() } else { Ink.darkFill } }
                        .frame(width: 26, height: 26).clipShape(Circle())
                        .overlay(Circle().stroke(Ink.amber.opacity(0.65), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Live preview of \(title)")
            }
        }
        .padding(10)
        .accessibilityElement(children: .combine)
    }
}

// MARK: Files (MobileScreens.swift PhoneFilesPage)

/// PhoneFilesPage's progress card: what is being sent, how far, and a bar. `value` is the words on the right (the
/// percentage by default); `note` and `actions` are not from the lab (the app's line under it and its buttons).
public struct TransferCard<Actions: View>: View {
    let title: String
    let fraction: Double
    var value: String?
    var note: String?
    var actions: Actions

    public init(title: String, fraction: Double, value: String? = nil, note: String? = nil, @ViewBuilder actions: () -> Actions) {
        self.title = title
        self.fraction = fraction
        self.value = value
        self.note = note
        self.actions = actions()
    }

    public var body: some View {
        Card(padding: 18) {
            HStack {
                Text(title).type(16, .semibold)
                Spacer()
                Text(value ?? "\(Int(fraction * 100))%").type(15, .medium, Ink.faint).monospacedDigit()
            }
            .accessibilityElement(children: .combine)
            GeometryReader { g in
                ZStack(alignment: .leading) {
                    Capsule().fill(Ink.line)
                    Capsule().fill(Ink.black).frame(width: g.size.width * max(0, min(1, fraction)))
                }
            }
            .frame(height: 6)
            .padding(.top, 10)
            if let note { Text(note).type(14, .regular, Ink.detail).frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8) }
            actions
        }
    }
}

public extension TransferCard where Actions == EmptyView {
    init(title: String, fraction: Double, value: String? = nil, note: String? = nil) {
        self.init(title: title, fraction: fraction, value: value, note: note, actions: { EmptyView() })
    }
}

// MARK: The link (MobileScreens.swift LinkPicture)

/// LinkPicture.Kind.
public enum LinkKind: Sendable { case direct, relay, search }

/// Your phone and the Mac side by side, joined by a still line of dots (through the relay when there is one), with
/// the Mac's name and how it is reached underneath. Nothing moves.
public struct LinkPicture: View {
    @Environment(\.skyInk) private var ink
    public typealias Kind = LinkKind
    let kind: Kind
    var macName = "MacBook Pro"
    /// Not from the lab: the words under the name when the app has its own (its own relay, a Mac it can't reach).
    var status: String?
    /// Not from the lab: the name to show instead of the lab's ("Your Mac" while searching).
    var name: String?
    var onClick: (() -> Void)?

    public init(kind: Kind, macName: String = "MacBook Pro", status: String? = nil, name: String? = nil, onClick: (() -> Void)? = nil) {
        self.kind = kind
        self.macName = macName
        self.status = status
        self.name = name
        self.onClick = onClick
    }

    public var body: some View {
        VStack(spacing: 18) {
            HStack(spacing: 14) {
                disc("iphone", size: 58, white: false)
                dots
                if kind == .relay {
                    Image(systemName: "cloud").font(.system(size: 15, weight: .medium)).foregroundStyle(.white)
                        .frame(width: 38, height: 38).background(Ink.black, in: Circle())
                    dots
                }
                if kind == .search {
                    Image(systemName: "laptopcomputer").font(.system(size: 26)).foregroundStyle(ink.low)
                        .frame(width: 76, height: 76)
                        .overlay(Circle().strokeBorder(ink.low, style: StrokeStyle(lineWidth: 1.5, dash: [4, 5])))
                } else {
                    disc("laptopcomputer", size: 76, white: true)
                }
            }
            .accessibilityHidden(true)
            VStack(spacing: 4) {
                Text(name ?? (kind == .search ? "Your Mac" : macName)).type(24, .semibold, ink.low)
                Text(status ?? (kind == .search ? "Not paired yet" : kind == .relay ? "Through the shared relay" : "Connected directly"))
                    .type(15.5, .medium, ink.low)
            }
            .accessibilityElement(children: .combine)
        }
        .frame(maxWidth: .infinity)
        .pressable(onClick)
    }

    private var dots: some View {
        HStack(spacing: 6) {
            ForEach(0..<(kind == .relay ? 3 : 6), id: \.self) { _ in Circle().fill(ink.low).frame(width: 4, height: 4) }
        }
    }

    private func disc(_ symbol: String, size: CGFloat, white: Bool) -> some View {
        Image(systemName: symbol).font(.system(size: size * 0.4)).foregroundStyle(white ? Ink.black : Ink.white)
            .frame(width: size, height: size).background(white ? Ink.white : Ink.black, in: Circle())
    }
}

// MARK: Pairing (MobileScreens.swift PairPage, CodesPage)

/// CodesPage's code: three pairs of digits in grey squares, fixed-width so the pairs line up. The digits are drawn
/// black: the lab leaves their colour to the colour scheme (its dark stage would draw them white on light grey).
public struct PairingCode: View {
    let code: String
    public init(_ code: String) { self.code = code }

    public var body: some View {
        let digits = Array(code.filter(\.isNumber))
        let pairs = stride(from: 0, to: digits.count, by: 2).map { String(digits[$0..<min($0 + 2, digits.count)]) }
        HStack(spacing: 10) {
            ForEach(Array(pairs.enumerated()), id: \.offset) { _, pair in
                Text(pair).font(.system(size: 42, weight: .semibold)).monospacedDigit().foregroundStyle(Ink.black)
                    .frame(width: 86, height: 86).background(Ink.fill, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
            }
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(digits.map(String.init).joined(separator: " ")))
    }
}

/// PairPage.step: a numbered step.
public struct PairStep: View {
    let number: Int
    let text: String
    public init(_ number: Int, _ text: String) { self.number = number; self.text = text }
    public var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text("\(number)").type(13, .bold).frame(width: 26, height: 26).background(Ink.fill, in: Circle())
            Text(text).type(15.5, .regular).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
    }
}

/// PairPage.action: a small labelled way out under the main button.
public struct PairAction: View {
    let symbol: String
    let title: String
    var onClick: (() -> Void)?
    public init(symbol: String, title: String, onClick: (() -> Void)? = nil) { self.symbol = symbol; self.title = title; self.onClick = onClick }
    public var body: some View {
        VStack(spacing: 6) {
            Image(systemName: symbol).font(.system(size: 20))
            Text(title).type(13.5, .medium, Ink.detail)
        }
        .foregroundStyle(.black)
        .frame(maxWidth: .infinity)
        .pressable(onClick)
    }
}

// MARK: Questions (MobileScreens.swift QuestionSheet, SecretSheet, RelaySheet; Conversation.swift CartSheet)

/// QuestionSheet's command: the exact thing, on black, in a fixed-width face.
public struct ExactCommand: View {
    let command: String
    public init(_ command: String) { self.command = command }
    public var body: some View {
        Text(command)
            .font(.system(size: 15, design: .monospaced)).foregroundStyle(.white)
            .padding(15).frame(maxWidth: .infinity, alignment: .leading)
            .background(Ink.black, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

/// Conversation.swift CartSheet's item: the exact thing, on black, with its price.
public struct ExactItem: View {
    let title: String
    let detail: String
    let value: String
    public init(title: String, detail: String, value: String) { self.title = title; self.detail = detail; self.value = value }
    public var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).type(16, .semibold, .white)
                Text(detail).type(14, .regular, Ink.onDark)
            }
            Spacer()
            Text(value).type(20, .semibold, .white)
        }
        .padding(15)
        .background(Ink.black, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityElement(children: .combine)
    }
}

/// QuestionSheet's line under the exact thing: what Yes will ask for (Android's FingerprintNote).
public struct BiometricNote: View {
    var text = "Yes asks for Face ID."
    var symbol = "faceid"
    public init(_ text: String = "Yes asks for Face ID.", symbol: String = "faceid") { self.text = text; self.symbol = symbol }
    public var body: some View {
        HStack(spacing: 6) {
            Image(systemName: symbol)
            Text(text)
        }
        .type(14, .regular, Ink.detail)
        .accessibilityElement(children: .combine)
    }
}

/// SecretSheet's boxes: `filled` of them hold a dot and the next one is ringed with the caret. The digits themselves
/// are never drawn.
public struct SecretCodeBoxes: View {
    let filled: Int
    var length = 6
    public init(filled: Int, length: Int = 6) { self.filled = filled; self.length = length }
    public var body: some View {
        HStack(spacing: 8) {
            ForEach(0..<length, id: \.self) { i in
                ZStack {
                    RoundedRectangle(cornerRadius: 14, style: .continuous).fill(i == filled ? Ink.white : Ink.fill)
                        .overlay { if i == filled { RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(.black, lineWidth: 2.5) } }
                    if i < filled { Circle().fill(.black).frame(width: 11, height: 11) }
                    if i == filled { Rectangle().fill(.black).frame(width: 2, height: 22) }
                }
                .frame(height: 54)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text("\(filled) of \(length)"))
    }
}

/// SecretSheet's keypad: digits with their letters on white keys over a grey ground. `onKey` gets "0" to "9" and
/// "⌫" for delete.
public struct SecretKeypad: View {
    var bottomInset: CGFloat = PhoneInsets.lab.bottom
    var onKey: ((String) -> Void)?
    private let keys: [(String, String)] = [("1", ""), ("2", "ABC"), ("3", "DEF"), ("4", "GHI"), ("5", "JKL"), ("6", "MNO"),
                                            ("7", "PQRS"), ("8", "TUV"), ("9", "WXYZ"), ("", ""), ("0", ""), ("⌫", "")]

    public init(bottomInset: CGFloat = PhoneInsets.lab.bottom, onKey: ((String) -> Void)? = nil) {
        self.bottomInset = bottomInset
        self.onKey = onKey
    }

    public var body: some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 7), count: 3), spacing: 7) {
            ForEach(keys.indices, id: \.self) { i in
                let key = keys[i]
                VStack(spacing: -2) {
                    if key.0 == "⌫" { Image(systemName: "delete.left").font(.system(size: 20)) }
                    else {
                        Text(key.0).font(.system(size: 25))
                        if !key.1.isEmpty { Text(key.1).font(.system(size: 9, weight: .bold)).tracking(1.6).foregroundStyle(Ink.detail) }
                    }
                }
                .foregroundStyle(.black)
                .frame(maxWidth: .infinity).frame(height: 50)
                .background { if !key.0.isEmpty, key.0 != "⌫" { RoundedRectangle(cornerRadius: 12, style: .continuous).fill(.white) } }
                .pressable(key.0.isEmpty ? nil : onKey.map { f in { f(key.0) } }, label: key.0 == "⌫" ? "Delete" : nil)
            }
        }
        .padding(.horizontal, 6).padding(.top, 8).padding(.bottom, bottomInset + 10)
        .background(LabColors.keypad)
    }
}

/// RelaySheet's and ProfileSheet's head: a title, and the close button.
public struct SheetTitle: View {
    let title: String
    var onClose: (() -> Void)?
    var showsClose = true
    public init(_ title: String, showsClose: Bool = true, onClose: (() -> Void)? = nil) {
        self.title = title
        self.showsClose = showsClose
        self.onClose = onClose
    }
    public var body: some View {
        HStack(alignment: .top) {
            Text(title).type(22, .semibold).accessibilityAddTraits(.isHeader)
            Spacer()
            if showsClose { CloseButton(onClick: onClose) }
        }
    }
}
