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
            if let taskID = approval.taskID, let goal = model.runningTasks[taskID] {
                Text(goal).font(.system(size: 11)).foregroundStyle(Palette.secondary).lineLimit(2)
                    .help("Task " + taskID)
            }
            MarkdownContent(approval.visibleReason).equatable().font(.system(size: 12))
            if approval.visibleReason != approval.reason {
                DisclosureGroup("Details") {
                    MarkdownContent(approval.reason).equatable().font(.system(size: 11))
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
        .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 15))
        .padding(.trailing, 28)
        // Only a secret question takes keyboard focus; an ordinary one leaves it with the composer.
        .onChange(of: model.focusRequest) { _, _ in inputFocused = model.bodyVisible && model.focusedQuestionID == approval.id }
        .onChange(of: approval.id) { _, _ in answer = "" }
        .onChange(of: model.bodyVisible) { _, visible in if !visible { answer = ""; inputFocused = false } }
        .onDisappear { answer = "" }
    }

    private func send() {
        guard !answer.isEmpty, !model.sendingAnswer else { return }
        let value = answer
        answer = ""
        model.answerApproval(approval, text: value)
    }
}
