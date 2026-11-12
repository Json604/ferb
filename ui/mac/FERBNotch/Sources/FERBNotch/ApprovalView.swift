import SwiftUI
import NotchCore

/// One open question, shown inline at the end of today's chat while the composer stays usable (spec §4.7), or inside
/// its card row (`inRow`, APP-300: the question and its buttons in the row of the ask it is about).
/// Hidden values live only in this field and the one answer RPC. Never in NotchSession or task history.
struct ApprovalView: View {
    @ObservedObject var model: NotchModel
    let approval: DaemonApproval
    var inRow = false
    @State private var answer = ""
    @FocusState private var inputFocused: Bool

    /// The card's box (APP-726, `DaemonApproval.answerBox`): FERB's own check-ins and runtime offers are buttons only
    /// (ORCH-536, ORCH-315); a task's question takes its buttons or the owner's words, in a row too; a safety check of a
    /// task takes Yes / No, or words for that task, sent as a correction and never as a Yes (GATE-510).
    private var box: DaemonApproval.AnswerBox { approval.answerBox }
    private var takesTypedAnswer: Bool { box != .none }

    var body: some View {
        VStack(alignment: .leading, spacing: inRow ? 6 : 9) {
            if !inRow {
                // GATE-566: the header the daemon sends by the question's structure
                Label(approval.title ?? (approval.isSecret ? "Private input needed" : "FERB needs your answer"),
                      systemImage: approval.isSecret ? "lock" : "questionmark.bubble")
                    .font(.system(size: 11, weight: .medium)).foregroundStyle(Palette.secondary)
                if let context = model.questionContext(approval) {
                    Text(context).font(.system(size: 11)).foregroundStyle(Palette.secondary).lineLimit(2)
                        .help("Task " + (approval.taskID ?? ""))
                }
            }
            if approval.fromTask {                   // D33, GATE-225: a task's question, tagged by structure
                Text("asked by the AI").font(.system(size: 10, weight: .medium)).foregroundStyle(Palette.secondary)
                    .padding(.horizontal, 6).padding(.vertical, 1.5)
                    .background(.white.opacity(0.07), in: Capsule())
                    .accessibilityLabel("Question asked by the AI")
            }
            if approval.plainText {                  // FERB's own question: verbatim, never Markdown (D31)
                // GATE-513 (refining GATE-225): one question in plain words, why FERB asks under it, and the exact
                // thing approved in its own code box. A long question or exact part (a command, rule 6) stays fully
                // shown in a capped-height block that scrolls, never behind a click.
                ApprovalText(text: approval.visibleReason, size: 12.5, bold: true)
                if approval.hasPicture || approval.pictureProblem != nil {   // GATE-579: the payment page, once
                    PaymentPictureView(model: model, pictures: model.paymentPictures, approval: approval)
                }
                if let detail = approval.detail {
                    ApprovalText(text: detail, size: 11.5, secondary: true)
                }
                if let exact = approval.exact { ExactBox(text: exact) }
            } else {
                MarkdownContent(approval.visibleReason).equatable().font(.system(size: 12, weight: inRow ? .medium : .regular))
                if let detail = approval.detail {
                    // ORCH-623: what a plan's question is about (the checked answers of the steps before it), whole,
                    // under it, as the answers themselves read
                    MarkdownContent(detail).equatable().font(.system(size: 11.5)).foregroundStyle(Palette.secondary)
                }
            }
            if let why = approval.why {                // the task's reason, its own words, smaller (2026-10-27)
                // chat text draws through FERB's own text view (APP-788); GATE-566: marked as the AI's words
                NativeMessageText((approval.capability == "ferb.retry" ? "AI's note: " : "AI's reason: ") + why,
                                  size: 11, secondary: true)
            }
            if !approval.evidence.isEmpty {
                DisclosureGroup("App-use evidence") {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(Array(approval.evidence.enumerated()), id: \.offset) { _, line in
                            NativeMessageText(line, size: 11, secondary: true)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }.font(.system(size: 11)).foregroundStyle(Palette.secondary)
            }
            if !approval.isSecret && inRow && !approval.options.isEmpty {
                // The row's choices side by side, the first one a little brighter (the mock's `.opts`).
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 6) { optionButtons }
                    VStack(alignment: .leading, spacing: 6) { optionButtons }
                }
                .padding(.top, 1)
            } else if !approval.isSecret {
                ForEach(Array(approval.options.indices), id: \.self) { index in
                    Button(approval.optionLabel(index)) { model.chooseApproval(approval, index: index) }
                        .buttonStyle(FerbButtonStyle()).disabled(model.sendingAnswer)
                }
            }
            if !approval.isSecret, let note = approval.note {   // GATE-566: what the 30-minute answer covers
                Text(note).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if approval.isSecret {
                SecureField(approval.fieldLabel ?? (approval.kind == "owner_login_code" ? "Code" : "Type it here"),
                            text: $answer)
                    .ferbField()
                    .focused($inputFocused)
                    .privacySensitive()
                    .onSubmit(send)
                    .accessibilityLabel(approval.kind == "owner_login_code" ? "Private verification code" : "Private password or secret")
            } else if takesTypedAnswer {
                TextField(box == .correction ? "Or tell FERB what to do instead…" : "Or type your answer…", text: $answer)
                    .ferbField()
                    .focused($inputFocused).onSubmit(send)
                    .accessibilityLabel(box == .correction ? "Tell the task something else instead" : "Your answer")
            }
            if takesTypedAnswer { answerButtons }
        }
        .modifier(QuestionChrome(inRow: inRow) { model.focusQuestion(approval.id) })
        // Its field takes keyboard focus only when the owner asks; a question arriving never moves focus. Once
        // asked, a panel focus request returns focus here rather than to the composer, until the field loses it.
        .onChange(of: model.questionFocus) { _, _ in takeRequestedFocus() }
        .onChange(of: model.focusRequest) { _, _ in takeRequestedFocus() }
        .onAppear(perform: takeRequestedFocus)
        .onChange(of: inputFocused) { _, focused in
            if !focused, model.questionFocus?.id == approval.id { model.questionFocus = nil }
        }
        .onChange(of: approval.id) { _, _ in answer = "" }
        .onChange(of: model.bodyVisible) { _, visible in if !visible { answer = ""; inputFocused = false } }
        .onDisappear {
            answer = ""
            if model.questionFocus?.id == approval.id { model.questionFocus = nil }
        }
    }

    @ViewBuilder private var optionButtons: some View {
        ForEach(Array(approval.options.indices), id: \.self) { index in
            Button(approval.optionLabel(index)) { model.chooseApproval(approval, index: index) }
                .buttonStyle(FerbButtonStyle(primary: index == 0)).disabled(model.sendingAnswer)
        }
    }

    private var answerButtons: some View {
        HStack {
            if approval.isSecret || approval.options.isEmpty {     // with choices, the choices already answer it
                Button(approval.isSecret ? approval.declineLabel ?? "Don't fill in" : "Skip") {
                    answer = ""
                    model.declineApproval(approval)
                }.buttonStyle(FerbButtonStyle(quiet: true))
            }
            Spacer()
            Button(model.sendingAnswer ? "Sending…" : approval.isSecret ? approval.sendLabel ?? "Fill in" : "Send",
                   action: send)
                .buttonStyle(FerbButtonStyle(primary: true)).disabled(answer.isEmpty || model.sendingAnswer)
        }.disabled(model.sendingAnswer)
    }

    private func takeRequestedFocus() {
        guard model.questionFocus?.id == approval.id, model.bodyVisible else { return }
        DispatchQueue.main.async {
            if model.questionFocus?.id == approval.id, model.bodyVisible { inputFocused = true }
        }
    }

    private func send() {
        guard !answer.isEmpty, !model.sendingAnswer else { return }
        let value = answer
        answer = ""
        if box == .correction { model.correctApproval(approval, text: value) }
        else { model.answerApproval(approval, text: value) }
    }
}

/// The question's own card at the end of the chat; inside a card row the row is its card.
private struct QuestionChrome: ViewModifier {
    let inRow: Bool
    let tapped: () -> Void

    func body(content: Content) -> some View {
        if inRow {
            content
        } else {
            content
                .padding(.horizontal, 11).padding(.vertical, 9)
                .background {
                    // Clicking the card itself (not a control in it) is the owner choosing to answer it.
                    RoundedRectangle(cornerRadius: 15).fill(.white.opacity(0.10))
                        .onTapGesture(perform: tapped)
                }
                .padding(.trailing, 28)
        }
    }
}

/// FERB's own words on a question card, word for word and selectable in FERB's own text view (APP-788); a long one
/// scrolls in a capped block.
private struct ApprovalText: View {
    let text: String
    let size: CGFloat
    var bold = false
    var secondary = false
    var body: some View {
        let line = NativeMessageText(text, size: size, secondary: secondary, bold: bold)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
        if text.count > ApprovalLayout.longQuestionChars {
            ScrollView(.vertical) { line }.frame(maxHeight: ApprovalLayout.longQuestionHeight)
        } else {
            line
        }
    }
}

/// GATE-513: the exact thing the owner approves, in a code box: the same font as code in answers, word for word,
/// selectable, scrolling when long rather than hiding any of it.
struct ExactBox: View {
    let text: String
    var body: some View {
        let shown = NativeMessageText(text, size: 11.5, monospaced: true).fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 9).padding(.vertical, 7)
        Group {
            if text.count > ApprovalLayout.longQuestionChars || text.split(separator: "\n").count > 6 {
                ScrollView(.vertical) { shown }.frame(maxHeight: ApprovalLayout.longQuestionHeight)
            } else {
                shown
            }
        }
        .background(.black.opacity(0.28), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .accessibilityLabel("Exactly: " + text)
    }
}

/// Display only: when FERB's own question scrolls inside its card instead of growing it.
enum ApprovalLayout {
    static let longQuestionChars = 360
    static let longQuestionHeight: CGFloat = 150
}
