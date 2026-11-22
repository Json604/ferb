import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI

// One of FERB's questions as a box in the chat, as FERB.app draws it (kb PHN-770; ApprovalView.swift, placed by
// ConversationTranscript.swift:205-208 at the end of the chat and TurnCardView.swift:298-301 in its row). Android:
// ui/QuestionBox.kt.

/// The open questions as the model merged them (their needs from the Mac's status), for the boxes in the chat.
struct QuestionsKey: EnvironmentKey { static let defaultValue: [Question] = [] }
/// What is on its way (`answer:<id>` while a box's answer is sent).
struct BusyKey: EnvironmentKey { static let defaultValue: Set<String> = [] }
extension EnvironmentValues {
    var questions: [Question] { get { self[QuestionsKey.self] } set { self[QuestionsKey.self] = newValue } }
    var busy: Set<String> { get { self[BusyKey.self] } set { self[BusyKey.self] = newValue } }
}

/// The question's card: its header, the question, why, the exact thing, the task's reason, the evidence, the buttons
/// with the Mac's words, the note, and the typed answer's box. It stays until answered anywhere or withdrawn (the
/// transcript closes it); nothing dismisses it. The buttons act in place: a Yes that needs Face ID asks for it when
/// pressed, No needs nothing. A hidden input is entered here: a code on the lab's own keypad (the system keyboard never
/// sees it), anything else in a SecureField; the app stays out of captures while its field shows, and the value is
/// cleared once sent (PHN-110, PHN-502). `title`/`reason` are the transcript's words, shown alone while the live
/// question isn't known (FERB.app then draws the words only, TurnCardView.swift:302-305).
struct QuestionBox: View {
    @Bindable var model: AppModel
    let approvalId: String
    let title: String?
    let reason: String?
    @Environment(\.questions) private var questions
    @Environment(\.busy) private var busySet
    @State private var words = ""
    @State private var secret = ""
    @State private var digits: [UInt16] = []
    @State private var evidenceOpen = false
    private static let maxCode = 10
    private let ink = CardInk(true)

    var body: some View {
        let q = questions.first { $0.approvalId == approvalId }
        let busy = busySet.contains("answer:\(approvalId)")
        VStack(alignment: .leading, spacing: 8) {
            if let q, !q.held {
                let m = q.shown
                let detail: String? = m.map { [$0.pictureProblem, $0.detail].compactMap { $0 }.joined(separator: "\n\n") }.flatMap { $0.isEmpty ? nil : $0 }
                    ?? (q.title != nil && !q.reason.isEmpty ? q.reason : nil)
                // FERB's review of an import (kb MEM-460): its list in place of the detail; the typed answer's box stays
                let review = m?.review == true
                head(header: m.flatMap { $0.header.isEmpty ? nil : $0.header } ?? L.s(q.hidden ? "question_private_title" : "question_needs_answer"),
                     question: m?.question ?? q.title ?? q.reason, detail: review ? nil : detail, fromTask: m?.fromTask == true)
                if review { ReviewList(model: model, ink: ink) }
                if let e = m?.exact ?? q.exact {
                    Text(e).font(.system(size: 14.5, design: .monospaced)).foregroundStyle(ink.primary)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(12)
                        .background(ink.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .accessibilityLabel("Exactly: " + e)
                }
                if let why = m?.why { Text(why).type(13.5, .regular, ink.secondary).fixedSize(horizontal: false, vertical: true) }
                if let ev = m?.evidence, !ev.isEmpty {
                    Button { evidenceOpen.toggle() } label: { Text((evidenceOpen ? "▾ " : "▸ ") + "App-use evidence").type(13.5, .medium, ink.secondary) }
                        .buttonStyle(.plain)
                    if evidenceOpen { ForEach(ev, id: \.self) { Text($0).type(13.5, .regular, ink.secondary).padding(.leading, 12) } }
                }
                if q.onlyOnMac {
                    Text([q.needs?.problem, L.s("question_on_mac")].compactMap { $0 }.joined(separator: "\n\n")).type(14.5, .regular, ink.secondary)
                } else if q.hidden {
                    hidden(q, busy)
                } else {
                    answers(q, busy)
                }
                if busy { Text("Sending your answer…").type(13, .regular, ink.secondary) }
            } else {
                head(header: reason, question: title ?? L.s("question_needs_answer"), detail: nil, fromTask: false)
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ink.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .padding(.trailing, 28)
        .environment(\.onFerb, true)
        .accessibilityIdentifier("question:\(approvalId)")
        .onDisappear { words = ""; secret = ""; for i in digits.indices { digits[i] = 0 }; digits = [] }
    }

    private func head(header: String?, question: String, detail: String?, fromTask: Bool) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Mark(kind: .asking)
            VStack(alignment: .leading, spacing: 3) {
                if let header { Text(header).type(13, .medium, ink.secondary) }
                // D33, GATE-225: a task's question, tagged by structure
                if fromTask {
                    Text("from the task").type(11.5, .medium, ink.secondary).padding(.horizontal, 7).padding(.vertical, 1)
                        .background(ink.fill, in: Capsule())
                }
                Text(question).type(16.5, .semibold, ink.primary).fixedSize(horizontal: false, vertical: true)
                if let detail { Text(detail).type(14.5, .regular, ink.secondary).fixedSize(horizontal: false, vertical: true) }
            }
        }
    }

    @ViewBuilder private func answers(_ q: Question, _ busy: Bool) -> some View {
        let m = q.shown
        let choices = q.choices
        let act: (AnswerInput) -> (() -> Void)? = { input in busy ? nil : { _ = model.answer(q.approvalId, input) } }
        if q.yesNo {
            let no = choices[0], yes = choices[1]
            // the phone's own line, said only where the Mac's status says the Yes takes the strong key
            if q.needsProof && q.needs?.fingerprint == true {
                HStack(spacing: 6) {
                    Image(systemName: "faceid")
                    Text(L.bio("question_yes_fingerprint"))
                }.type(13.5, .regular, ink.secondary)
            }
            HStack(spacing: 10) {
                Pill(title: no.label, kind: .grey, height: 44, fill: false, onClick: act(no.input))
                Pill(title: yes.label, kind: .black, height: 44, onClick: act(yes.input)).frame(maxWidth: .infinity).layoutPriority(1)
            }
            ForEach(Array(choices.dropFirst(2).enumerated()), id: \.offset) { _, c in Pill(title: c.label, kind: .grey, height: 44, onClick: act(c.input)) }
            if q.untilBack {
                Pill(title: L.s("until_back_yes"), kind: .grey, height: 44, onClick: act(.untilBack))
                Text(L.s("until_back_note")).type(13.5, .regular, ink.secondary)
            }
        } else {
            ForEach(Array(choices.enumerated()), id: \.offset) { i, c in Pill(title: c.label, kind: i == 0 ? .black : .grey, height: 44, onClick: act(c.input)) }
        }
        if let n = m?.note { Text(n).type(13, .regular, ink.secondary).fixedSize(horizontal: false, vertical: true) }
        // FERB.app's box: his own words (a question without choices, or a task's question beside its choices)
        if choices.isEmpty || m?.answerBox == "answer" {
            ZStack(alignment: .leading) {
                if words.isEmpty { Text(m?.field ?? L.s("question_answer_hint")).type(16, .regular, ink.faint) }
                TextField("", text: $words, axis: .vertical).lineLimit(1...4).type(16, .regular, ink.primary).tint(ink.primary)
                    .accessibilityLabel(Text(m?.field ?? L.s("question_answer_hint")))
            }
            .padding(.horizontal, 14).padding(.vertical, 12).frame(minHeight: 48)
            .background(ink.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            HStack(spacing: 10) {
                if choices.isEmpty {
                    Pill(title: m?.decline ?? L.s("question_skip"), kind: .grey, height: 44, fill: false, onClick: act(.words(text: "")))
                }
                Pill(title: m?.send ?? L.s("question_send"), kind: .black, height: 44,
                     onClick: busy || words.trimmingCharacters(in: .whitespaces).isEmpty ? nil : {
                         _ = model.answer(q.approvalId, .words(text: words.trimmingCharacters(in: .whitespacesAndNewlines)))
                         words = ""
                     })
            }
        }
    }

    @ViewBuilder private func hidden(_ q: Question, _ busy: Bool) -> some View {
        let m = q.shown
        let decline = m?.decline ?? L.s("private_decline")
        let send = m?.send ?? L.s("private_send")
        Group {
            if q.isCode {
                SecretCodeBoxes(filled: digits.count, length: max(6, min(Self.maxCode, digits.count + 1)))
                SecretKeypad(bottomInset: 0) { k in
                    if k == "⌫" { if !digits.isEmpty { digits[digits.count - 1] = 0; digits.removeLast() } }
                    else if digits.count < Self.maxCode, let u = k.utf16.first { digits.append(u) }
                }
                .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            } else {
                SecureField("", text: $secret, prompt: Text(m?.field ?? L.s("private_field")).foregroundColor(ink.faint))
                    .font(.system(size: 17)).foregroundStyle(ink.primary).textContentType(.password)
                    .autocorrectionDisabled().textInputAutocapitalization(.never)
                    .padding(.horizontal, 14).frame(minHeight: 48)
                    .background(ink.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            }
            HStack(alignment: .top, spacing: 6) {
                Image(systemName: "lock.fill")
                Text("Private: it goes straight to your Mac and is not kept on this phone.")
            }
            .type(13.5, .regular, ink.secondary)
            HStack(spacing: 10) {
                Pill(title: decline, kind: .grey, height: 44, fill: false, onClick: busy ? nil : {
                    secret = ""
                    _ = model.answer(q.approvalId, .words(text: ""))
                })
                Pill(title: send, kind: .black, height: 44, onClick: busy || (q.isCode ? digits.isEmpty : secret.isEmpty) ? nil : {
                    if q.isCode {
                        _ = model.answerHidden(q.approvalId, units: &digits)
                    } else {
                        var units = Array(secret.utf16)
                        secret = ""
                        _ = model.answerHidden(q.approvalId, units: &units)
                    }
                })
            }
        }
        .onAppear { model.hiddenShown(q.approvalId, true) }
        .onDisappear { model.hiddenShown(q.approvalId, false) }
    }
}
