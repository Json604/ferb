import FerbCore
import FerbMobile
import Foundation

/// One of FERB's questions as the phone shows it, read from the question entry's body (the gateway's own fields:
/// GATE-566's title and summary, the exact thing, the options and their button words). Nothing here judges what a
/// question is about: which answers need the fingerprint is the Mac's to say (PHN-470), and the phone follows its
/// refusal (PHN-13: refused as locked, it unlocks and asks once more). (Android: model/Questions.kt.)
public struct Question: Equatable, Sendable {
    public var approvalId: String
    public var taskId: String?
    /// GATE-566's summary or title, else nil (the app's own words).
    public var title: String?
    public var reason: String
    public var exact: String?
    public var kind: String
    public var capability: String?
    public var options: [String]
    public var labels: [String]
    public var hidden: Bool
    /// What answering needs, as the Mac's status says (nil: no status yet).
    public var needs: QuestionNeeds? = nil
    /// The words were held back (a locked phone, PHN-126): only that a question waits.
    public var held: Bool = false
    /// What FERB.app's card shows for it (kb PHN-739, ferb-mobile `question`): header, question, why, exact, buttons.
    public var shown: QuestionShown? = nil
    /// A temporary chat's task asks it (the Mac's status says so): shown only in the temporary chat, never kept.
    public var temporary: Bool = false

    public init(approvalId: String, taskId: String?, title: String?, reason: String, exact: String?, kind: String, capability: String?,
                options: [String], labels: [String], hidden: Bool, needs: QuestionNeeds? = nil, held: Bool = false, shown: QuestionShown? = nil) {
        self.shown = shown
        self.approvalId = approvalId
        self.taskId = taskId
        self.title = title
        self.reason = reason
        self.exact = exact
        self.kind = kind
        self.capability = capability
        self.options = options
        self.labels = labels
        self.hidden = hidden
        self.needs = needs
        self.held = held
    }

    /// PHN-417, PHN-470: answered from the notification's buttons with no prompt. Unknown needs are not routine; nor is
    /// the planner's question, which is answered by a message in the chat (kb PHN-762).
    public var routine: Bool { (needs.map { $0.canAnswer && !$0.needsUnlock && !$0.hidden } ?? false) && !hidden && !clarify }

    /// The planner's question that waits on the owner (the Mac's status lists it as kind "clarify", kb PHN-761):
    /// FERB's own bubble in the chat, answered by his next message; never a gateway question with buttons or a sheet.
    public var clarify: Bool { kind == Question.clarifyKind }

    /// Only on the Mac (a hidden input on a phone that can't sign it, …): its words say why.
    public var onlyOnMac: Bool { needs?.canAnswer == false }

    /// The Yes needs the phone's signed proof: the fingerprint-only key when `needs.fingerprint`.
    public var needsProof: Bool { needs.map { $0.needsUnlock && $0.canAnswer } ?? false }

    /// A hidden input's kind is a code (the keypad) rather than a password or a secret (the system keyboard).
    public var isCode: Bool { kind == "owner_login_code" }

    /// One of FERB's own checks (the gateway's OWN_CHECKS by capability): the class whose Yes may need the fingerprint.
    public var ownCheck: Bool { capability.map(Question.ownChecks.contains) ?? false }

    public struct Choice: Equatable, Sendable {
        public let label: String
        public let input: AnswerInput
        public let yes: Bool
    }

    /// The Mac offers "until I'm back at my Mac" on this question (PHN-610, PHN-611: the screen-use question, its
    /// `QuestionNeeds.untilBack`): a further Yes, answered with `AnswerInput.untilBack`, under the two buttons.
    public var untilBack: Bool { needs?.untilBack == true && yesNo }

    /// The gateway's Yes and No come first: the lab's two-button sheet, with any further option under them (PHN-607).
    public var yesNo: Bool { !hidden && options.count >= 2 && options[0] == "Yes" && options[1] == "No" }

    /// The answers as buttons, in the order the Mac draws them; empty: the owner types the answer.
    public var choices: [Choice] {
        if hidden || options.isEmpty { return [] }
        if yesNo {
            // the gateway's yes/no: No is a plain No (it needs nothing, PHN-470); Yes is option 1, as FERB.app sends it.
            // Any option the Mac adds after them (PHN-607) is its own number, as FERB.app's numbering has it.
            return [Choice(label: label(1), input: .approve(granted: false, choice: nil), yes: false),
                    Choice(label: label(0), input: .approve(granted: true, choice: 1), yes: true)]
                + (2..<options.count).map { Choice(label: label($0), input: .approve(granted: true, choice: Int64($0 + 1)), yes: false) }
        }
        return options.indices.map { Choice(label: label($0), input: .approve(granted: true, choice: Int64($0 + 1)), yes: $0 == 0) }
    }

    private func label(_ i: Int) -> String {
        let l = i < labels.count ? labels[i].trimmingCharacters(in: .whitespacesAndNewlines) : ""
        return l.isEmpty ? options[i] : l
    }

    /// ferb-link role.rs OWN_CHECKS (the gateway's): FERB's own consequential checks.
    public static let ownChecks: Set<String> = [
        "ferb.confirm", "ferb.retry", "conversation.approve", "memory.forget", "downloads.delete", "browsing.choose",
    ]

    /// The kind the Mac's status gives the planner's question (ferb-daemon phone/asking.rs).
    public static let clarifyKind = "clarify"

    /// ferb-link role.rs SECRET_KINDS: answered only through the sealed path (PHN-109, PHN-110).
    public static let secretKinds: Set<String> = ["owner_login_code", "owner_admin_password", "owner_secret"]
}

extension Question {
    func withTemporary(_ on: Bool) -> Question {
        var q = self
        q.temporary = on
        return q
    }
}

public enum Questions {
    public static func parse(_ q: QuestionItem) -> Question {
        let body = Json.object(q.bodyJson)
        let kind = body.str("kind") ?? "owner_question"
        // kb PHN-739: the Mac's words, from the library (a record an app made itself has none: read from its body)
        let shown = q.words ?? questionShown(bodyJson: q.bodyJson)
        return Question(approvalId: q.approvalId ?? "", taskId: q.taskId, title: shown.question,
                        reason: body.str("reason") ?? "", exact: body.str("exact"), kind: kind, capability: body.str("capability"),
                        options: body.strings("options"), labels: body.strings("labels"),
                        hidden: Question.secretKinds.contains(kind) || body.flag("secret"), shown: shown)
    }

    /// The open questions of a transcript, oldest first.
    public static func open(_ items: [QuestionItem]) -> [Question] {
        items.filter { $0.open && $0.approvalId != nil }.map(parse)
    }

    /// A question as the Mac's status gives it (the app is locked, or the chat hasn't brought it yet).
    public static func fromStatus(_ s: StatusQuestion) -> Question {
        let words = s.held ? nil : s.words
        return Question(approvalId: s.approvalId, taskId: s.taskId, title: words?.question, reason: s.held ? "" : s.reason, exact: words?.exact,
                 kind: s.kind == Question.clarifyKind ? Question.clarifyKind : s.needs.hidden ? "owner_secret" : "owner_question",
                 capability: s.capability,
                 options: s.held ? [] : s.options, labels: words?.buttons ?? [], hidden: s.needs.hidden, needs: s.needs, held: s.held, shown: words)
            .withTemporary(s.temporary)
    }

    /// The transcript's open questions, each with its needs from `status`, then the status's questions the transcript
    /// doesn't hold yet (each question once): the questions the app's sheets and the Tasks tab answer. The planner's
    /// question is not one of them (it is FERB's bubble in the chat, as on the Mac): see `waiting`.
    public static func merged(_ transcript: [Question], _ status: PhoneStatus?) -> [Question] {
        waiting(transcript, status).filter { !$0.clarify }
    }

    /// Every question that waits on the owner, the planner's too (kb PHN-760, PHN-761): what the notifications follow.
    public static func waiting(_ transcript: [Question], _ status: PhoneStatus?) -> [Question] {
        var byId: [String: StatusQuestion] = [:]
        for q in status?.questions ?? [] { byId[q.approvalId] = q }
        let known = transcript.map { q -> Question in
            guard let s = byId[q.approvalId] else { return q }
            var q = q
            q.needs = s.needs
            q.temporary = s.temporary
            return q
        }
        let ids = Set(known.map(\.approvalId))
        return known + (status?.questions ?? []).filter { !ids.contains($0.approvalId) }.map(fromStatus)
    }
}

/// How an answer went.
public enum Answered: Equatable {
    case sent
    /// The owner cancelled the prompt: nothing was sent.
    case notSent
    /// The Mac or the link said no: its words and code.
    case failed(FerbError)
    /// The Mac wants the Face ID / Touch ID key, which iOS made unusable when the enrolled faces or fingers changed:
    /// pair again.
    case needsPairingAgain
}

/// Sends one answer the way PHN-13 and PHN-470 have it (kb PHN-503): first as it is (a routine Yes, an ordinary
/// answer and a plain No need no proof from a session that proved the phone); if the Mac says this answer needs the
/// phone's own proof (`unlock_required`), it is signed once, by the Face ID / Touch ID key where the phone has one
/// (PHN-137), else the lock key; if the Mac's unlock window had closed (`locked`), the phone unlocks toward the Mac
/// and asks once more. A hidden value always goes signed (the sealed path, [answerHidden]). Never queued (PHN-448).
@MainActor
public final class AnswerFlow {
    private let api: PhoneApi
    private let keys: () -> KeyHealth?
    private let signer: (KeyKind) -> Signer?

    /// `signer` nil: no prompt can be shown (nothing is sent).
    public init(api: PhoneApi, keys: @escaping () -> KeyHealth?, signer: @escaping (KeyKind) -> Signer?) {
        self.api = api
        self.keys = keys
        self.signer = signer
    }

    public func answer(_ approvalId: String, _ input: AnswerInput, upfront: KeyKind? = nil) async -> Answered {
        var unlockedOnce = false
        var signedOnce = false
        // the Mac's status said this answer needs the phone's proof: asked for at once, not after a refusal
        var proof: Signer?
        if let upfront {
            if upfront == .strong && keys()?.strong != .ok { return .needsPairingAgain }
            guard let s = signer(upfront) else { return .notSent }
            proof = s
            signedOnce = true
        }
        while true {
            let r = await api.answer(approvalId, input, signer: proof)
            guard case .failed(let err) = r else { return .sent }
            guard case .NeedsUnlock = err else { return done(r) }
            switch err.linkCode {
            case "unlock_required", "unlock_invalid", "fresh_unlock_required":
                if signedOnce { return .failed(err) }
                signedOnce = true
                let k = keys()
                let kind: KeyKind
                if k?.strong == .ok { kind = .strong }
                else if k?.strong == .gone { return .needsPairingAgain }
                else if k?.lock == .ok { kind = .lock }
                else { return .needsPairingAgain }
                guard let s = signer(kind) else { return .notSent }
                proof = s
            default:
                if unlockedOnce { return .failed(err) }
                unlockedOnce = true
                guard let s = signer(.lock) else { return .notSent }
                if case .failed(let e) = await api.unlock(s) {
                    if case .NotSigned = e { return .notSent }
                    return .failed(e)
                }
            }
        }
    }

    /// A hidden value always goes signed, on the sealed path (PHN-110), from the library's [SecretInput] (never a
    /// String): the library empties it whatever happens.
    public func answerHidden(_ approvalId: String, secret: SecretInput) async -> Answered {
        guard let s = signer(.lock) else { return .notSent }
        return done(await api.answerHidden(approvalId, secret: secret, signer: s))
    }

    private func done(_ r: Outcome<String>) -> Answered {
        switch r {
        case .ok: return .sent
        case .failed(let e):
            if case .NotSigned = e { return .notSent }
            return .failed(e)
        }
    }
}
