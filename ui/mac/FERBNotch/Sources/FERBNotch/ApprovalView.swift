import SwiftUI
import NotchCore

/// Hidden values live only in this field and the one answer RPC. Never in NotchSession or task history.
struct ApprovalView: View {
    @ObservedObject var model: NotchModel
    let approval: DaemonApproval
    @State private var answer = ""
    @FocusState private var inputFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(approval.isSecret ? "Private input needed" : approval.title ?? "FERB needs your answer")
                    .font(.system(size: 13, weight: .medium))
                Spacer()
                Button("Tasks", action: model.toggleTasks).buttonStyle(.plain).font(.system(size: 11))
            }
            if let taskID = approval.taskID {
                Text(model.runningTasks[taskID] ?? "Task " + taskID)
                    .font(.system(size: 11)).foregroundStyle(Palette.secondary).lineLimit(2)
                Text("Task " + taskID).font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Palette.secondary).textSelection(.enabled)
            } else if approval.title == nil {
                Text("Owner request · no task identity provided")
                    .font(.system(size: 10)).foregroundStyle(Palette.secondary)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 9) {
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
                }.frame(maxWidth: .infinity, alignment: .leading)
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
            if let error = model.promptError {
                Text(error).font(.system(size: 10)).foregroundStyle(Palette.secondary)
            }
            HStack {
                Button(approval.isSecret ? "Cancel" : "Decline") {
                    answer = ""
                    model.declineApproval(approval)
                }.buttonStyle(FerbButtonStyle(quiet: true))
                Spacer()
                if model.pendingApprovals.count > 1 {
                    Text("\((model.pendingApprovals.firstIndex(where: { $0.id == approval.id }) ?? 0) + 1) of \(model.pendingApprovals.count)").font(.system(size: 10)).foregroundStyle(Palette.secondary)
                }
                Button(model.sendingAnswer ? "Sending…" : "Send", action: send)
                    .buttonStyle(FerbButtonStyle(primary: true)).disabled(answer.isEmpty || model.sendingAnswer)
            }.disabled(model.sendingAnswer)
        }
        .onChange(of: model.focusRequest) { _, _ in inputFocused = model.bodyVisible }
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
