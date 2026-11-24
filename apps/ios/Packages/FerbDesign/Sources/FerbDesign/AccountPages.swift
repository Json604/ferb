import SwiftUI

// Not from the lab: FERB's account on the phone (owner 2026-11-24, kb PHN-827, PHN-829), built from the pages that are
// (MessagePage's mark and title over the sky, the pairing page's white card at the bottom, its pills and LinkPicture).
// Stateless: the app passes its words and what to do.

/// The sign-in, first after the lock: FERB's mark and the welcome over the sky; in the card the line, Continue with
/// Google, Continue with Apple, and the footnote. `line` is what happened (cancelled, can't reach…) or the browser
/// still open; `working` greys the two buttons while a sign-in is under way.
public struct SignInPage: View {
    @Environment(\.skyInk) private var ink
    let title: String
    let text: String
    let google: String
    let apple: String
    let footnote: String
    let line: String?
    let working: Bool
    var insets: PhoneInsets
    var onGoogle: (() -> Void)?
    var onApple: (() -> Void)?

    public init(title: String, text: String, google: String, apple: String, footnote: String, line: String? = nil,
                working: Bool = false, insets: PhoneInsets = .lab, onGoogle: (() -> Void)? = nil, onApple: (() -> Void)? = nil) {
        self.title = title
        self.text = text
        self.google = google
        self.apple = apple
        self.footnote = footnote
        self.line = line
        self.working = working
        self.insets = insets
        self.onGoogle = onGoogle
        self.onApple = onApple
    }

    public var body: some View {
        VStack(spacing: 14) {
            Spacer(minLength: 12)
            FerbMark(size: 84)
            Text(title).type(28, .semibold, ink.top).multilineTextAlignment(.center).padding(.top, 6).padding(.horizontal, 24)
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 12)
            Card(padding: 20) {
                VStack(alignment: .leading, spacing: 14) {
                    Text(text).type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                    if let line {
                        HStack(alignment: .top, spacing: 10) {
                            if working { Mark(kind: .running, size: 18).padding(.top, 1) }
                            Text(line).type(15, .medium, Ink.black).fixedSize(horizontal: false, vertical: true)
                        }
                        .accessibilityElement(children: .combine)
                        .accessibilityAddTraits(.updatesFrequently)
                    }
                    Pill(title: google, kind: .black, onClick: working ? nil : onGoogle).padding(.top, 2)
                    Pill(title: apple, kind: .black, symbol: "apple.logo", onClick: working ? nil : onApple)
                    Text(footnote).type(13, .regular, Ink.detail).lineSpacing(1).fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 2)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}

/// Phone first (the owner: "how are we gonna do those cases where its phone download first???"): signed in, no Mac
/// paired yet. The phone and the Mac as the pairing page draws them; in the card why, where to get FERB for the Mac,
/// Send me the link with its answer, the way on to pairing, and another account.
public struct MacFirstPage: View {
    let title: String
    let text: String
    let address: String
    let sendTitle: String
    let pairTitle: String
    let otherTitle: String
    let line: String?
    let sending: Bool
    var insets: PhoneInsets
    var onSend: (() -> Void)?
    var onPair: (() -> Void)?
    var onOther: (() -> Void)?

    public init(title: String, text: String, address: String, sendTitle: String, pairTitle: String, otherTitle: String,
                line: String? = nil, sending: Bool = false, insets: PhoneInsets = .lab, onSend: (() -> Void)? = nil,
                onPair: (() -> Void)? = nil, onOther: (() -> Void)? = nil) {
        self.title = title
        self.text = text
        self.address = address
        self.sendTitle = sendTitle
        self.pairTitle = pairTitle
        self.otherTitle = otherTitle
        self.line = line
        self.sending = sending
        self.insets = insets
        self.onSend = onSend
        self.onPair = onPair
        self.onOther = onOther
    }

    public var body: some View {
        VStack(spacing: 0) {
            Spacer(minLength: 8)
            LinkPicture(kind: .search)
            Spacer(minLength: 12)
            Card(padding: 20) {
                VStack(alignment: .leading, spacing: 12) {
                    Text(title).type(24, .semibold).accessibilityAddTraits(.isHeader)
                    Text(text).type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 10) {
                        Image(systemName: "laptopcomputer").font(.system(size: 16, weight: .medium)).foregroundStyle(Ink.black)
                        Text(address).type(17, .semibold).textSelection(.enabled)
                    }
                    .padding(.horizontal, 16).frame(maxWidth: .infinity, minHeight: 48, alignment: .leading)
                    .background(Ink.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .accessibilityElement(children: .combine)
                    Pill(title: sendTitle, kind: .grey, symbol: "envelope", height: 48, onClick: sending ? nil : onSend)
                    if let line {
                        Text(line).type(14.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                            .accessibilityAddTraits(.updatesFrequently)
                    }
                    Pill(title: pairTitle, kind: .black, onClick: onPair).padding(.top, 2)
                    Text(otherTitle).type(15, .medium, Ink.detail)
                        .padding(.horizontal, 16).padding(.vertical, 4)
                        .pressable(onOther, label: otherTitle)
                        .frame(maxWidth: .infinity)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}
