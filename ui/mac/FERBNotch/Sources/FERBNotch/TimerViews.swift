import SwiftUI
import NotchCore

// Timers that flow like Apple's (owner, 2026-11-06, APP-762): set (in chat) → running (the closed notch's live
// activity) → done (the arrival's alert). Apple's Clock app and Live Activities are the model: the timer's name, a
// large countdown in monospaced digits with a numeric transition, a ring that drains linearly from full to empty over
// the timer's duration, "Ends 10:41" with a bell, and at the end the name large, "Done" in the timer's orange with the
// ring complete and one gentle pulse, and Stop and Repeat. FERB's rules win where they differ: one notification sound,
// never a looping alarm (APP-521, APP-532); no Pause (FERB's timers can't pause). Reduce Motion: no pulse, no
// draining animation, cross-fades only.

extension Palette {
    /// The timer's one accent: Apple's timer orange.
    static let timer = Color(.sRGB, red: 1, green: 0.58, blue: 0, opacity: 1)
}

/// The ring: a faint track and the timer's orange from the top, clockwise, as long as `fraction` (1 full, 0 empty).
struct TimerRing: View {
    let fraction: Double
    var lineWidth: CGFloat = 5
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            Circle().stroke(.white.opacity(0.12), lineWidth: lineWidth)
            Circle()
                .trim(from: 0, to: max(0, min(1, fraction)))
                .stroke(Palette.timer, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
                .rotationEffect(.degrees(-90))
                // one tick a second, drawn as a steady linear drain (Apple's ring)
                .animation(reduceMotion ? nil : .linear(duration: 1), value: fraction)
        }
    }
}

/// A countdown in monospaced digits that changes with a numeric transition.
struct TimerDigits: View {
    let text: String
    var size: CGFloat
    var tint: Color = Palette.ink
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Text(verbatim: text)
            .font(.system(size: size, weight: .semibold, design: .rounded)).monospacedDigit()
            .foregroundStyle(tint)
            .contentTransition(reduceMotion ? .opacity : .numericText(countsDown: true))
            .animation(reduceMotion ? nil : .snappy(duration: 0.3), value: text)
    }
}

/// The timer's own small label, tinted.
private struct TimerMeta: View {
    let text: String
    var body: some View {
        Label(text, systemImage: "timer").font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.timer)
    }
}

/// "Timer set" in chat: the name, a large live countdown inside its draining ring, "Ends 10:41" with a bell, Cancel.
/// What it says after comes from FERB's row (DMN-548, `NotchModel.timerRecords`), whoever changed it: Done only for a
/// timer that fired, Cancelled for one cancelled anywhere (its card, chat, Settings).
struct TimerSetCard: View {
    let card: ResultCard
    @ObservedObject var model: NotchModel

    var body: some View {
        CardSurface {
            if let ends = card.timerEnds, ends > Date(), record?.state ?? .running == .running {
                TimelineView(.periodic(from: Date(), by: 1)) { context in content(now: context.date, ends: ends) }
            } else {
                content(now: Date(), ends: card.timerEnds)
            }
        }
        .onAppear { model.needTimerState(card.timerID) }
    }

    private var record: TimerRecord? { card.timerID.flatMap { model.timerRecords[$0] } }

    private func content(now: Date, ends: Date?) -> some View {
        let cancelled = record?.state == .cancelled, fired = record?.state == .fired
        let running = !cancelled && !fired && (ends.map { $0 > now } ?? false)
        let left = ends.map { $0.timeIntervalSince(now) } ?? 0
        let fraction: Double? = cancelled ? 0 : fired ? 1
            : running ? ends.flatMap { TimerClock.remaining(started: card.startedDate, ends: $0, now: now) } : 0
        // ended, but FERB's row doesn't say fired yet (its firing in flight, or an older daemon): neither Done nor a count
        let digits = cancelled ? "–" : running ? TimerClock.countdown(left) : fired ? "Done" : "0:00"
        return HStack(alignment: .center, spacing: 14) {
            VStack(alignment: .leading, spacing: 6) {
                TimerMeta(text: cancelled ? "Timer cancelled" : fired ? "Timer done" : "Timer")
                Text(verbatim: card.what ?? card.title ?? "Timer")
                    .font(.system(size: 13.5, weight: .semibold)).foregroundStyle(Palette.ink)
                    .fixedSize(horizontal: false, vertical: true)
                if let ends, !cancelled {
                    Label(running ? "Ends " + CardTime.time(ends) : "Ended " + CardTime.time(ends),
                          systemImage: "bell.fill")
                        .font(.system(size: 10.5, weight: .medium)).foregroundStyle(Palette.secondary)
                }
                if running, let id = card.timerID {
                    Button("Cancel") { model.cancelTimer(id) }
                        .buttonStyle(FerbButtonStyle(quiet: true))
                        .accessibilityLabel("Cancel the timer")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            ZStack {
                if let fraction { TimerRing(fraction: fraction) }
                TimerDigits(text: digits, size: running ? 19 : 15, tint: fired ? Palette.timer : Palette.ink)
            }
            .frame(width: 78, height: 78)
        }
        .accessibilityElement(children: .contain)
    }
}

/// The timer's end, as the arrival's alert (the closed notch's compact arrival grows into it, APP-622) and as its card
/// in chat: the name large, "Done" in orange inside the completed ring with one gentle pulse, when it was due, and
/// Stop and Repeat. It stays until handled or the owner hovers, clicks or opens the chat (APP-624). Stop and Repeat show
/// while FERB's row says the timer fired and wasn't repeated and the owner hasn't stopped it (DMN-548,
/// `NotchModel.timerAlertOpen`): the arrival and the card agree, and a relaunch reads the row again.
struct TimerDoneView: View {
    let card: ResultCard
    var model: NotchModel?
    var framed = false
    @State private var pulse = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if framed { CardSurface { content } } else { content }
    }

    private var content: some View {
        HStack(alignment: .center, spacing: framed ? 14 : 22) {
            VStack(alignment: .leading, spacing: framed ? 6 : 10) {
                TimerMeta(text: "Timer")
                Text(verbatim: card.title ?? "Timer")
                    .font(.system(size: framed ? 15 : 24, weight: .semibold)).tracking(framed ? 0 : -0.5)
                    .foregroundStyle(Palette.ink).fixedSize(horizontal: false, vertical: true)
                if let due = card.dueDate {
                    Label("Due " + CardTime.time(due), systemImage: "bell.fill")
                        .font(.system(size: framed ? 10.5 : 12, weight: .medium)).foregroundStyle(Palette.secondary)
                }
                if let model, let id = card.timerID {
                    TimerAlertButtons(id: id, repeatable: card.started != nil, framed: framed, model: model)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            ZStack {
                TimerRing(fraction: 1, lineWidth: framed ? 5 : 7)
                VStack(spacing: 2) {
                    TimerDigits(text: "0:00", size: framed ? 17 : 26, tint: Palette.timer)
                    Text("Done").font(.system(size: framed ? 10 : 11.5, weight: .semibold)).foregroundStyle(Palette.timer)
                }
            }
            .frame(width: framed ? 78 : 118, height: framed ? 78 : 118)
            .scaleEffect(pulse ? 1.06 : 1)
        }
        .frame(maxWidth: .infinity, minHeight: framed ? 0 : 118, alignment: .topLeading)
        .onAppear {
            model?.needTimerState(card.timerID)
            guard !reduceMotion else { return }                   // Reduce Motion: no pulse
            withAnimation(.spring(response: 0.32, dampingFraction: 0.6)) { pulse = true }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.32) {
                withAnimation(.spring(response: 0.45, dampingFraction: 0.8)) { pulse = false }
            }
        }
    }
}

/// A fired timer's Stop and Repeat, shown from the model's answer (`timerAlertOpen`), never per-view state.
private struct TimerAlertButtons: View {
    let id: String
    /// The card has the timer's start: a duration to repeat.
    let repeatable: Bool
    let framed: Bool
    @ObservedObject var model: NotchModel

    var body: some View {
        if model.timerAlertOpen(id) {
            HStack(spacing: 8) {
                Button("Stop") { model.stopTimerAlert(id) }
                    .buttonStyle(FerbButtonStyle(quiet: true))
                    .accessibilityLabel("Stop the timer alert")
                if repeatable {
                    Button("Repeat") { model.repeatTimer(id) }
                        .buttonStyle(FerbButtonStyle(primary: true))
                        .accessibilityLabel("Repeat the timer")
                }
            }
            .padding(.top, framed ? 2 : 4)
        }
    }
}

/// The closed notch's live activity (APP-762): while a timer runs and FERB is closed, the timer glyph on one side of the
/// camera and the countdown on the other, on black that continues the camera housing; on a notchless Mac they sit on
/// either side of the pill, which they lengthen. Drawn by a click-through window of its own and only beside the housing
/// or the pill (the middle stays clear), so press, drag and hover reach the handle as before.
struct TimerActivityView: View {
    @ObservedObject var model: NotchModel
    /// The housing's or the pill's width in the middle, left clear.
    let gap: CGFloat
    let height: CGFloat
    /// Beside the pill: the outer ends are rounded like the pill's.
    var pill = false

    var body: some View {
        TimelineView(.periodic(from: Date(), by: 1)) { context in
            if let timer = model.soonestTimer(now: context.date) {
                let left = TimerClock.countdown(timer.ends.timeIntervalSince(context.date))
                HStack(spacing: 0) {
                    Image(systemName: "timer")
                        .font(.system(size: min(15, height * 0.5), weight: .semibold)).foregroundStyle(Palette.timer)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(ear(leading: true))
                    Color.clear.frame(width: gap)
                    TimerDigits(text: left, size: min(14, height * 0.48), tint: Palette.timer)
                        .lineLimit(1).minimumScaleFactor(0.7)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(ear(leading: false))
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Timer \(timer.name), \(left) left")
            }
        }
        .preferredColorScheme(.dark)
        .ignoresSafeArea()
    }

    /// One side's black: square where it meets the housing or the pill, rounded at its outer end (the housing's lower
    /// corner, or the pill's end).
    private func ear(leading: Bool) -> some View {
        let r = pill ? height / 2 : height * 0.42
        return UnevenRoundedRectangle(topLeadingRadius: pill && leading ? r : 0,
                                      bottomLeadingRadius: leading ? r : 0,
                                      bottomTrailingRadius: leading ? 0 : r,
                                      topTrailingRadius: pill && !leading ? r : 0, style: .continuous)
            .fill(.black)
    }
}
