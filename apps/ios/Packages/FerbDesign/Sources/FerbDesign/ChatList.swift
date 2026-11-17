import SwiftUI

// The chat as a list (kb PHN-627, PHN-628), Android's ChatList.kt: a day's start between days, the "new message" pill
// when the owner scrolled up, and a waiting look while the first page comes. Not from the lab.

/// A day's start in the chat: "Today", "Yesterday" or the date, small and centred in the sky's ink.
public struct DayDivider: View {
    let text: String
    @Environment(\.skyInk) private var ink
    public init(_ text: String) { self.text = text }
    public var body: some View {
        Text(text).type(13, .semibold, ink.low)
            .frame(maxWidth: .infinity)
            .padding(.top, 6).padding(.bottom, 2)
            .accessibilityAddTraits(.isHeader)
    }
}

/// Over the chat's end when the owner scrolled up and something new arrived: a tap goes down to it.
public struct NewMessagePill: View {
    let text: String
    let onClick: () -> Void
    public init(_ text: String, onClick: @escaping () -> Void) { self.text = text; self.onClick = onClick }
    public var body: some View { JumpPill(text, label: text, onClick: onClick) }
}

/// The way back to the chat's latest messages whenever the owner is not at its end (kb PHN-662; Android's JumpPill),
/// on the dark fill so it stands apart from FERB's black messages under it: a down arrow, with `text` beside it when
/// there is something to say ("New message", or the latest messages' day, "Today", when he is in an earlier day), the
/// arrow alone when he is in the same day scrolled up. `label` says what a tap does (the arrow's name when alone).
public struct JumpPill: View {
    let text: String?
    let label: String
    let onClick: () -> Void
    public init(_ text: String?, label: String, onClick: @escaping () -> Void) { self.text = text; self.label = label; self.onClick = onClick }
    public var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.down").font(.system(size: text == nil ? 15 : 13, weight: .semibold))
            if let text { Text(text).type(14, .semibold, Ink.white).lineLimit(1) }
        }
        .foregroundStyle(Ink.white)
        .padding(.horizontal, text == nil ? 11 : 16).padding(.vertical, 9)
        .background(Ink.darkFill, in: Capsule())
        .pressable(onClick, label: text.map { "\($0), \(label)" } ?? label)
        .accessibilityAddTraits(.updatesFrequently)
    }
}

/// What the chat's jump control shows (kb PHN-662, PHN-698): see `of`.
public enum ChatJump: Equatable, Sendable {
    case none, newMessage, day, latest
    /// In the earlier days' chat: "Today" takes him back to today, from anywhere in it, its end too.
    case today

    /// In the earlier days' chat (`earlier`) always the way back to today, wherever he is in it, its end too (kb
    /// PHN-698). In today's: nothing at the end; "New message" when something arrived while he was away; the latest
    /// messages' day when the day he is looking at (`shownDay`, days since 1970) is before it (`endDay`); else the
    /// arrow alone.
    public static func of(atEnd: Bool, unseen: Bool, shownDay: Int?, endDay: Int?, earlier: Bool = false) -> ChatJump {
        if earlier { return .today }
        if atEnd { return .none }
        if unseen { return .newMessage }
        if let shownDay, let endDay, shownDay < endDay { return .day }
        return .latest
    }
}

/// The day he is looking at, held at the top of the chat while he scrolls through it (kb PHN-663): the day divider's
/// words on a small capsule of the dark fill, so it reads over the messages under it and apart from them.
public struct StickyDay: View {
    let text: String
    public init(_ text: String) { self.text = text }
    public var body: some View {
        Text(text).type(13, .semibold, Ink.white).lineLimit(1)
            .padding(.horizontal, 12).padding(.vertical, 4)
            .background(Ink.darkFill, in: Capsule())
            .frame(maxWidth: .infinity)
            .accessibilityAddTraits(.isHeader)
    }
}

/// How a status line reads (kb PHN-660, PHN-661; Android's StatusTone): `working`, something on its way that clears by
/// itself (a dashed ring, the lab's waiting mark, never a spinner); `info`, a standing fact (a quiet grey dot);
/// `attention`, something that needs him on the Mac's side (an amber lamp and amber words). None is something to tap:
/// what has an action is a card.
public enum StatusTone: Sendable { case working, info, attention }

/// The app's one status line under the tabs (kb PHN-660): small grey words on a compact black capsule (the lab's
/// surface for what sits on the sky: the tabs, the message box), led by the tone's mark, so it reads as the app talking
/// about itself, never as a message, an error or a button, at every time of day.
public struct StatusLine: View {
    let text: String
    let tone: StatusTone
    public init(_ text: String, tone: StatusTone) { self.text = text; self.tone = tone }
    public var body: some View {
        HStack(spacing: 8) {
            mark
            Text(text).type(13, .medium, tone == .attention ? Ink.amber : Ink.onDark).lineSpacing(1)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.leading, 11).padding(.trailing, 13).padding(.vertical, 6)
        .background(Ink.black, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 16)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.updatesFrequently)
    }

    @ViewBuilder private var mark: some View {
        switch tone {
        case .attention: LampDot(color: Ink.amber, size: 6)
        case .info: Circle().fill(Ink.onDark).frame(width: 6, height: 6)
        case .working:
            Circle().strokeBorder(Ink.onDark, style: StrokeStyle(lineWidth: 1.2, dash: [2, 2])).frame(width: 9, height: 9)
        }
    }
}

/// While the first page of the chat comes from the Mac and nothing is stored yet: the shapes of a few messages and one
/// plain line, never a blank screen. Still shapes (the lab's only unasked motion is the spinner).
public struct ChatWaiting: View {
    let text: String
    @Environment(\.skyInk) private var ink
    public init(_ text: String) { self.text = text }
    public var body: some View {
        let shade = Color.white.opacity(0.16)
        VStack(alignment: .leading, spacing: 12) {
            HStack { Spacer(); RoundedRectangle(cornerRadius: 16).fill(shade).frame(width: 190, height: 40) }
            RoundedRectangle(cornerRadius: 16).fill(shade).frame(width: 250, height: 64)
            HStack { Spacer(); RoundedRectangle(cornerRadius: 16).fill(shade).frame(width: 140, height: 40) }
            HStack(spacing: 10) {
                Spinner(size: 16)
                Text(text).type(14.5, .medium, ink.low)
            }
            .padding(.top, 6)
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.updatesFrequently)
        }
        .padding(16)
    }
}

/// A file waiting in the message box, or riding with a message he sent (kb PHN-690; Android's AttachmentChip): a small
/// picture of a photo where one is cheap to make (`thumbnail`), else the file's kind; its name and `detail` (its size,
/// or where it is). With `onRemove` a cross takes it out of the box (`removeLabel` for VoiceOver); nothing was sent.
public struct AttachmentChip: View {
    let name: String
    let detail: String
    let thumbnail: Image?
    let removeLabel: String?
    let onRemove: (() -> Void)?

    public init(_ name: String, detail: String, thumbnail: Image? = nil, removeLabel: String? = nil, onRemove: (() -> Void)? = nil) {
        self.name = name
        self.detail = detail
        self.thumbnail = thumbnail
        self.removeLabel = removeLabel
        self.onRemove = onRemove
    }

    public var body: some View {
        HStack(spacing: 8) {
            Group {
                if let thumbnail {
                    thumbnail.resizable().scaledToFill().frame(width: 36, height: 36).clipShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
                } else {
                    FileIcon(name: name, width: 24).frame(width: 36, height: 36)
                }
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(name).type(13.5, .medium, Ink.white).lineLimit(1)
                Text(detail).type(12, .medium, Ink.onDark).lineLimit(1)
            }
            .frame(maxWidth: 132, alignment: .leading)
            .accessibilityElement(children: .combine)
            if let onRemove {
                Image(systemName: "xmark").font(.system(size: 11, weight: .semibold)).foregroundStyle(Ink.onDark)
                    .frame(width: 40, height: 40).contentShape(Rectangle())
                    .pressable(onRemove, label: removeLabel)
            }
        }
        .padding(.leading, 6).padding(.trailing, onRemove == nil ? 12 : 0)
        .frame(height: 48)
        .background(Ink.darkFill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .fixedSize(horizontal: true, vertical: false)
    }
}

/// The files in the message box, side by side, scrolling sideways when there are more than fit.
public struct AttachmentRow<Content: View>: View {
    let content: Content
    public init(@ViewBuilder content: () -> Content) { self.content = content() }
    public var body: some View {
        ScrollView(.horizontal, showsIndicators: false) { HStack(spacing: 8) { content } }
    }
}

/// A message he sent with files, as the chat shows it while it is held on the phone (kb PHN-691; Android's
/// WithFilesRow): its files and words as his message, how it stands (`status`, in red when it failed: `bad`) and what
/// he can do (`actions`: a label and its tap).
public struct FilesMessage: View {
    let files: [(name: String, detail: String, thumbnail: Image?)]
    let text: String
    let status: String
    let bad: Bool
    let actions: [(String, () -> Void)]
    @Environment(\.skyInk) private var ink

    public init(files: [(name: String, detail: String, thumbnail: Image?)], text: String, status: String, bad: Bool, actions: [(String, () -> Void)]) {
        self.files = files
        self.text = text
        self.status = status
        self.bad = bad
        self.actions = actions
    }

    public var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            VStack(alignment: .trailing, spacing: 6) {
                ForEach(Array(files.prefix(4).enumerated()), id: \.offset) { _, f in
                    AttachmentChip(f.name, detail: f.detail, thumbnail: f.thumbnail)
                }
                if files.count > 4 { Text("+\(files.count - 4)").type(13.5, .medium, ink.low).padding(.horizontal, 6) }
                if !text.trimmingCharacters(in: .whitespaces).isEmpty { OwnerBubble(text) }
            }
            .opacity(bad ? 1 : 0.85)
            HStack(spacing: 12) {
                Text(status).type(13.5, .medium, bad ? Ink.red : ink.low)
                    .accessibilityAddTraits(.updatesFrequently)
                ForEach(Array(actions.enumerated()), id: \.offset) { _, a in
                    Text(a.0).type(13.5, .semibold, ink.top).frame(minHeight: 44).pressable(a.1, label: a.0)
                }
            }
            .padding(.horizontal, 6)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// The way to earlier days (kb PHN-626, PHN-698; Android's EarlierLink): an up arrow and `text` ("Earlier messages"),
/// small in the sky's ink, centred. On the home page under the tabs, and at the top of today's chat.
public struct EarlierLink: View {
    let text: String
    let onClick: (() -> Void)?
    @Environment(\.skyInk) private var ink
    public init(_ text: String, onClick: (() -> Void)?) { self.text = text; self.onClick = onClick }
    public var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.up").font(.system(size: 13, weight: .semibold))
            Text(text).type(14, .medium, ink.low)
        }
        .foregroundStyle(ink.low)
        .padding(.horizontal, 14).padding(.vertical, 8)
        .pressable(onClick, label: text)
        .frame(maxWidth: .infinity)
    }
}
