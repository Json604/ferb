import SwiftUI
import NotchCore

/// One open question, shown inline at the end of today's chat while the composer stays usable (spec §4.7).
/// Hidden values live only in this field and the one answer RPC. Never in NotchSession or task history.
struct ApprovalView: View {
    @ObservedObject var model: NotchModel
    let approval: DaemonApproval
    @State private var answer = ""
    @FocusState private var inputFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            Label(approval.isSecret ? "Private input needed" : approval.title ?? "FERB needs your answer",
                  systemImage: approval.isSecret ? "lock" : "questionmark.bubble")
                .font(.system(size: 11, weight: .medium)).foregroundStyle(Palette.secondary)
            if let context = model.questionContext(approval) {
                Text(context).font(.system(size: 11)).foregroundStyle(Palette.secondary).lineLimit(2)
                    .help("Task " + (approval.taskID ?? ""))
            }
            if approval.plainText {                  // FERB's own question: verbatim, never Markdown (D31)
                Text(verbatim: approval.visibleReason).font(.system(size: 12)).textSelection(.enabled)
            } else {
                MarkdownContent(approval.visibleReason).equatable().font(.system(size: 12))
            }
            if let details = approval.details {
                DisclosureGroup("Details") {
                    if approval.plainText {
                        Text(verbatim: details).font(.system(size: 11)).textSelection(.enabled)
                    } else {
                        MarkdownContent(details).equatable().font(.system(size: 11))
                    }
                }.font(.system(size: 11))
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
            if !approval.isSecret {
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
            } else {
                TextField("Your answer…", text: $answer)
                    .textFieldStyle(.plain).padding(8)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
                    .focused($inputFocused).onSubmit(send)
            }
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
        .padding(.horizontal, 11).padding(.vertical, 9)
        .background {
            // Clicking the card itself (not a control in it) is the owner choosing to answer it.
            RoundedRectangle(cornerRadius: 15).fill(.white.opacity(0.10))
                .onTapGesture { model.focusQuestion(approval.id) }
        }
        .padding(.trailing, 28)
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
