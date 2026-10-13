import SwiftUI
import NotchCore

private struct TranscriptHeight: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

struct ConversationTranscript: View {
    let turns: [ConversationTurn]
    @ObservedObject var model: NotchModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(turns) { turn in
                    VStack(alignment: .leading, spacing: 7) {
                        HStack {
                            Spacer(minLength: 28)
                            VStack(alignment: .leading, spacing: 5) {
                                Text(turn.request).textSelection(.enabled)
                                ForEach(turn.sources, id: \.self) { url in
                                    Label(url.lastPathComponent, systemImage: "paperclip").font(.system(size: 10))
                                }
                            }
                            .padding(.horizontal, 11).padding(.vertical, 8)
                            .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 15))
                            .help(turn.createdAt.formatted(date: .omitted, time: .shortened))
                        }
                        if !turn.answer.isEmpty {
                            HStack {
                                VStack(alignment: .leading, spacing: 6) {
                                    MarkdownContent(turn.visibleAnswer).equatable()
                                    if turn.displayText != nil && turn.visibleAnswer != turn.answer {
                                        DisclosureGroup("Details") {
                                            MarkdownContent(turn.answer).equatable()
                                                .frame(maxWidth: .infinity, alignment: .leading)
                                        }.font(.system(size: 11))
                                    }
                                }
                                    .padding(.horizontal, 11).padding(.vertical, 8)
                                    .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 15))
                                Spacer(minLength: 28)
                            }
                        }
                        if turn.isProvisional {
                            Text("Draft · still responding")
                                .font(.system(size: 10)).foregroundStyle(Palette.secondary)
                                .accessibilityLabel("Provisional response. FERB is still responding.")
                        }
                        if let notice = turn.responseNotice {
                            Text(notice).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                        }
                        if turn.responseNotice == nil && turn.status != "done" && turn.status != "working" {
                            Text(turn.status.replacingOccurrences(of: "_", with: " ").capitalized)
                                .font(.system(size: 10)).foregroundStyle(Palette.secondary)
                        }
                        ForEach(turn.artifacts, id: \.self) { url in
                            Button { model.onOpenDocument?(url) } label: {
                                Label(url.lastPathComponent, systemImage: "doc").lineLimit(1)
                            }
                            .buttonStyle(FerbButtonStyle(quiet: true))
                            .onDrag { NSItemProvider(object: url as NSURL) }
                            .contextMenu {
                                Button("Open") { model.onOpenDocument?(url) }
                                Button("Quick Look") { model.onQuickLook?(url) }
                                Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
                            }
                        }
                    }.id(turn.id)
                }
            }
            .font(.system(size: 12))
            .padding(.top, 36)
            .padding(.bottom, 2)
            .frame(maxWidth: .infinity)
            .background(GeometryReader { proxy in
                Color.clear.preference(key: TranscriptHeight.self, value: proxy.size.height)
            })
        }
        .defaultScrollAnchor(.bottom)
        .frame(height: model.chatTranscriptHeight)
        .mask {
            // Fade the content itself into the existing native glass. A material
            // overlay here creates a visible gray rectangle over the messages.
            VStack(spacing: 0) {
                LinearGradient(stops: [
                    .init(color: .clear, location: 0),
                    .init(color: .black.opacity(0.08), location: 0.15),
                    .init(color: .black.opacity(0.35), location: 0.38),
                    .init(color: .black.opacity(0.72), location: 0.65),
                    .init(color: .black.opacity(0.94), location: 0.85),
                    .init(color: .black, location: 1)
                ], startPoint: .top, endPoint: .bottom)
                .frame(height: 36)
                Rectangle().fill(.black)
            }
        }
        .onPreferenceChange(TranscriptHeight.self) { height in
            guard abs(model.measuredTranscriptHeight - height) > 1 else { return }
            model.measuredTranscriptHeight = height
            model.onBackgroundChange?(true)
        }
    }
}
