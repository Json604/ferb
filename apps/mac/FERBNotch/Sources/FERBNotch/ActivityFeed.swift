import SwiftUI
import NotchCore

struct ActivityFeed: View {
    let session: NotchSession
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                SectionCaption(text: "Task activity")
                Spacer()
                if session.phase == .working, let startedAt = session.startedAt {
                    Text(startedAt, style: .timer)
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(Palette.secondary)
                        .fixedSize()
                        .accessibilityLabel("Elapsed time")
                } else {
                    Text(session.phase == .attention ? "Needs attention" : (session.phase == .stopped ? "Stopped" : "Complete"))
                        .font(.system(size: 9)).foregroundStyle(Palette.secondary)
                }
            }
            Text(session.result?.request ?? session.draft)
                .font(.system(size: 12, weight: .medium))
                .lineLimit(1).truncationMode(.tail)
            ScrollViewReader { scroll in
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(session.activity) { event in
                            let isLatest = event.id == session.activity.last?.id
                            HStack(spacing: 8) {
                                Image(systemName: event.symbol)
                                    .font(.system(size: 10))
                                    .foregroundStyle(isLatest ? Palette.taskColor(session.phase) : Palette.secondary)
                                    .frame(width: 14)
                                Text(event.title)
                                    .font(.system(size: 11))
                                    .foregroundStyle(isLatest ? Palette.ink : Palette.secondary)
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                            }
                            .frame(height: 23)
                            .id(event.id)
                            .transition(reduceMotion ? .opacity : .opacity.combined(with: .offset(y: 3)))
                        }
                    }
                    .animation(.easeOut(duration: 0.18), value: session.activity.count)
                }
                .scrollIndicators(.hidden)
                .onAppear { scroll.scrollTo(session.activity.last?.id, anchor: .bottom) }
                .onChange(of: session.activity.count) { _, _ in
                    scroll.scrollTo(session.activity.last?.id, anchor: .bottom)
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Task activity")
    }
}
