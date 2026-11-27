import Foundation
import NotchCore

// Reply to a message (owner 2026-11-27: "gotta [add] quote any ferb's reply message, either slide or an on hover icon
// maybe. many apps have that"; "yeah my messages too"). A message's reply arrow (on hover, or a two-finger sideways
// swipe on it) puts its quote above the composer; sending passes `reply_to` and the sent bubble shows the quote. The
// quote belongs to the composer: it goes with the draft when the message is sent (APP-577), on Esc or its ✕.

/// The quote waiting in the composer, and the chat it was taken in (the saved chat's or the temporary chat's).
struct ComposerQuote: Equatable {
    let quote: ReplyQuote
    let source: ChatSource
}

@MainActor
extension NotchModel {
    /// The quote the composer shows: only in the chat it was taken in.
    var shownComposerQuote: ReplyQuote? {
        guard let held = composerQuote, held.source == composerSource else { return nil }
        return held.quote
    }

    /// The height the composer's quote line adds to the chat.
    var replyQuoteHeight: CGFloat { shownComposerQuote == nil ? 0 : Self.replyQuoteLineHeight }
    static let replyQuoteLineHeight: CGFloat = 26

    /// The owner chose to reply to `quote`'s message: its quote goes above the composer, which takes focus as a hover
    /// or a click on the chat gives it (APP-553), never over a question's field he is answering (`ChatInput`).
    func reply(to quote: ReplyQuote) {
        composerQuote = ComposerQuote(quote: quote, source: composerSource)
        onInteraction?()
        focusRequest = UUID()
        onBackgroundChange?(true)
    }

    /// Esc or the quote's ✕: the quote goes, the draft stays. False when there was none (Esc then does what it did).
    @discardableResult
    func dropReplyQuote() -> Bool {
        guard shownComposerQuote != nil else { return false }
        composerQuote = nil
        onBackgroundChange?(true)
        return true
    }

    /// The quote taken for the message sent now, which clears it with the draft (APP-577).
    func takeComposerQuote() -> ReplyQuote? {
        let quote = shownComposerQuote
        composerQuote = nil
        return quote
    }

    /// The quote of a message the chat shows, when its entry id is known (else it offers no reply).
    func quote(entryID: String?, kind: String, owner: Bool, text: String) -> ReplyQuote? {
        guard let entryID, !entryID.isEmpty,
              !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return ReplyQuote.of(entryID: entryID, kind: kind, owner: owner, text: text)
    }

    /// The owner clicked a quote: the quoted message comes into view and lights up briefly. Nothing moves when the
    /// chat shown doesn't hold it.
    func showQuoted(_ quote: ReplyQuote) {
        guard let target = shownArchive.shownTarget(ofEntry: quote.entryID) else { return }
        requestScroll(target, .asked)
        quoteHighlight = quote.entryID
        let shown = quote.entryID
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1.4))
            if self?.quoteHighlight == shown { self?.quoteHighlight = nil }
        }
    }
}
