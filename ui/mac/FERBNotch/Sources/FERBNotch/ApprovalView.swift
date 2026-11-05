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

    /// A typed answer: a secret, a question without choices, or (outside a row) any question but a runtime offer,
    /// which only its buttons answer (ORCH-315). In a row the choices are the answer, like the mock's.
    private var takesTypedAnswer: Bool {
        if approval.isSecret { return true }
        if approval.runtimeSwitch != nil { return false }
        return !inRow || approval.options.isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: inRow ? 6 : 9) {
            if !inRow {
                Label(approval.isSecret ? "Private input needed" : approval.title ?? "FERB needs your answer",
                      systemImage: approval.isSecret ? "lock" : "questionmark.bubble")
                    .font(.system(size: 11, weight: .medium)).foregroundStyle(Palette.secondary)
                if let context = model.questionContext(approval) {
                    Text(context).font(.system(size: 11)).foregroundStyle(Palette.secondary).lineLimit(2)
                        .help("Task " + (approval.taskID ?? ""))
                }
            }
            if approval.fromTask {                   // D33, GATE-225: a task's question, tagged by structure
                Text("from the task").font(.system(size: 10, weight: .medium)).foregroundStyle(Palette.secondary)
                    .padding(.horizontal, 6).padding(.vertical, 1.5)
                    .background(.white.opacity(0.07), in: Capsule())
                    .accessibilityLabel("Question from the task")
            }
            if approval.plainText {                  // FERB's own question: verbatim, monospaced, no Markdown (D31)
                // One question, no Details (GATE-225): a long one (an exact command, rule 6) stays fully shown in a
                // capped-height block that scrolls, never behind a click.
                if approval.visibleReason.count > ApprovalLayout.longQuestionChars {
                    ScrollView(.vertical) {
                        Text(verbatim: approval.visibleReason).font(.system(size: 12, design: .monospaced))
                            .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: ApprovalLayout.longQuestionHeight)
                } else {
                    Text(verbatim: approval.visibleReason).font(.system(size: 12, design: .monospaced))
                        .textSelection(.enabled)
                }
            } else {
                MarkdownContent(approval.visibleReason).equatable().font(.system(size: 12, weight: inRow ? .medium : .regular))
            }
            if let why = approval.why {                // the task's reason, its own words, smaller (2026-10-27)
                Text(verbatim: why).font(.system(size: 11)).foregroundStyle(Palette.secondary)
                    .textSelection(.enabled)
            }
            if !approval.evidence.isEmpty {
                DisclosureGroup("App-use evidence") {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(Array(approval.evidence.enumerated()), id: \.offset) { _, line in
                            Text(line).font(.system(size: 11)).textSelection(.enabled)
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
                ForEach(Array(approval.options.enumerated()), id: \.offset) { index, option in
                    Button(option) { model.chooseApproval(approval, index: index) }
                        .buttonStyle(FerbButtonStyle()).disabled(model.sendingAnswer)
                }
            }
            if approval.isSecret {
                SecureField(approval.kind == "owner_login_code" ? "Verification code" : "Password or secret", text: $answer)
                    .textFieldStyle(.plain).padding(8)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
                    .focused($inputFocused)
                    .privacySensitive()
                    .onSubmit(send)
                    .accessibilityLabel(approval.kind == "owner_login_code" ? "Private verification code" : "Private password or secret")
                Text("Sent privately to FERB’s sealed input path.")
                    .font(.system(size: 10)).foregroundStyle(Palette.secondary)
            } else if takesTypedAnswer {
                TextField("Your answer…", text: $answer)
                    .textFieldStyle(.plain).padding(8)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
                    .focused($inputFocused).onSubmit(send)
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
        ForEach(Array(approval.options.enumerated()), id: \.offset) { index, option in
            Button(option) { model.chooseApproval(approval, index: index) }
                .buttonStyle(FerbButtonStyle(primary: index == 0)).disabled(model.sendingAnswer)
        }
    }

    private var answerButtons: some View {
        HStack {
            Button(approval.isSecret ? "Cancel" : "Decline") {
                answer = ""
                model.declineApproval(approval)
            }.buttonStyle(FerbButtonStyle(quiet: true))
            Spacer()
            Button(model.sendingAnswer ? "Sending…" : "Send", action: send)
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
        model.answerApproval(approval, text: value)
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

/// Display only: when FERB's own question scrolls inside its card instead of growing it.
enum ApprovalLayout {
    static let longQuestionChars = 360
    static let longQuestionHeight: CGFloat = 150
}
