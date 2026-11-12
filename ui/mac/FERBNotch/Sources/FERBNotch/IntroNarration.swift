import AppKit
import SwiftUI
import NotchCore

/// FERB's intro lines in the notch: setting `introLine` swells the notch out of the camera housing to show the line
/// (the same native surface and spring as the plan preview, NotchPresentation), clearing it shrinks it back. The chat
/// stays closed throughout.
extension NotchModel {
    static let introFont = NSFont.systemFont(ofSize: 24, weight: .semibold)
    static let introDetailFont = NSFont.systemFont(ofSize: 13)

    /// The surface's height for a line (nil: no line): its content, plus the camera's space when the notch is docked.
    func introSurfaceHeight(_ line: IntroLine?) -> CGFloat? {
        guard let line else { return nil }
        let cameraSpace = isNotchDocked ? cutoutHeight : 0
        return min(availableSurfaceSize.height, introContentHeight(line) + cameraSpace)
    }

    /// Match the view's actual padding, text widths and rows; there is no reserved space below its footer.
    func introContentHeight(_ line: IntroLine, width: CGFloat? = nil,
                            inset: EdgeInsets? = nil) -> CGFloat {
        let inset = inset ?? IntroNarrationView.surfaceInset
        let width = max(1, (width ?? bodyWidth) - inset.leading - inset.trailing)
        func measured(_ text: String, _ font: NSFont, _ width: CGFloat) -> CGFloat {
            ceil((text as NSString).boundingRect(
                with: CGSize(width: max(1, width), height: .greatestFiniteMagnitude),
                options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font]).height)
        }
        let section = IntroNarrationView.sectionSpacing
        var height = inset.top + inset.bottom + IntroNarrationView.headerHeight + section
            + measured(line.text, Self.introFont, width)
        if let detail = line.detail, !detail.isEmpty { height += section + measured(detail, Self.introDetailFont, width) }
        if !line.checklist.isEmpty { height += section + CGFloat(line.checklist.count) * 32 + 78 }
        guard onIntroAction != nil else { return height }
        if line.entry != nil { height += IntroNarrationView.rowSpacing + IntroNarrationView.rowHeight }
        for choice in line.choices {
            height += IntroNarrationView.rowSpacing
                + max(IntroNarrationView.rowHeight,
                      measured(choice.title, Self.introChoiceFont, width - IntroNarrationView.choiceChromeWidth) + 12)
        }
        return height
    }

    static let introChoiceFont = NSFont.systemFont(ofSize: 12.5, weight: .medium)

    /// The owner did something on the intro line (the intro controller acts on it).
    func introAction(_ action: IntroAction) { onIntroAction?(action) }

    /// What the line's buttons call while the tour runs; nil: the line is only read.
    var introActions: ((IntroAction) -> Void)? {
        guard onIntroAction != nil else { return nil }
        return { [weak self] action in
            self?.onInteraction?()
            self?.introAction(action)
        }
    }

    func introLineChanged(from old: IntroLine?) {
        guard introLine != old else { return }
        if introLine == nil { endYield(.introCompanion); endYield(.activeAppUnderTourLine) }
        if let introLine { introDisplayLine = introLine }
        // In the open chat the line is a strip above the composer: the body's height follows it.
        if expanded { onBackgroundChange?(true); return }
        // A new line of the same size doesn't restart the spring; its text updates in place.
        guard !expanded, introSurfaceBefore.visible != surfaceVisible || introSurfaceBefore.size != surfaceSize
        else { return }
        onBackgroundChange?(true)
    }

    /// What the closed notch's surface shows. The live intro line wins while it is set; then a live plan preview;
    /// then the orb's homecoming swell; then whichever surface is still closing.
    var compactContent: NotchCompactContent? {
        guard !bodyVisible else { return nil }
        if let introLine { return .intro(introLine) }
        if compactPlanText != nil, let text = compactPlanDisplayText {
            return compactReminderCard.map { .reminder(card: $0, text: text) } ?? .plan(text)
        }
        if introPulsing { return .pulse }
        if let introDisplayLine { return .intro(introDisplayLine) }
        return compactPlanDisplayText.map { text in
            compactReminderCard.map { .reminder(card: $0, text: text) } ?? .plan(text)
        }
    }

    var introPulseSize: CGSize { CGSize(width: handleWidth + 40, height: cutoutHeight + 10) }

    /// The notch swells once and shrinks back, as it takes the orb back (Task 8 calls it on the orb's home arrival).
    /// Nothing happens while the chat is open or a line is showing.
    func pulseIntro(seconds: Double = 0.3) {
        guard !expanded, introLine == nil else { return }
        introDisplayLine = nil
        introPulseTask?.cancel()
        setIntroPulse(true)
        introPulseTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(seconds))
            guard !Task.isCancelled else { return }
            self?.setIntroPulse(false)
        }
    }

    private func setIntroPulse(_ on: Bool) {
        let wasVisible = surfaceVisible, before = surfaceSize
        introPulsing = on
        if !expanded && (wasVisible != surfaceVisible || before != surfaceSize) { onBackgroundChange?(true) }
    }
}

/// What the owner did on an intro line.
enum IntroAction: Equatable {
    case choose(String)
    case enter(String)
    case skipTour
}

enum NotchCompactContent: Equatable {
    case intro(IntroLine)
    case plan(String)
    case reminder(card: ResultCard, text: String)
    case pulse
}

struct IntroNarrationView: View {
    static let rowHeight: CGFloat = 42
    static let rowSpacing: CGFloat = 10
    static let headerHeight: CGFloat = 24
    static let sectionSpacing: CGFloat = 12
    static let choiceChromeWidth: CGFloat = 48 // 14pt insets, an 8pt gap and a 12pt arrow
    static let surfaceInset = EdgeInsets(top: 28, leading: 32, bottom: 12, trailing: 32)
    static let chatInset = EdgeInsets(top: 12, leading: 12, bottom: 12, trailing: 12)

    let line: IntroLine
    let cameraSpace: CGFloat
    /// Nil: the line is only read (no choices are offered).
    var act: ((IntroAction) -> Void)? = nil
    /// The notch's surface pads the line like the plan preview; the strip in the open chat is tighter.
    var inset = Self.surfaceInset
    /// The field's text (NotchModel.introTyped): emptied when the line changes, when the field goes and on submit.
    var typed: Binding<String> = .constant("")
    /// From the permissions beat to the end, the header offers Skip tour (APP-685).
    var canSkip = false
    @State private var hoveredPermission: String?

    var body: some View {
        VStack(alignment: .leading, spacing: Self.sectionSpacing) {
            HStack(spacing: 7) {
                Circle().fill(Color.orange).frame(width: 5, height: 5)
                Text("MAKE YOURSELF AT HOME")
                    .font(.system(size: 9, weight: .semibold)).tracking(1.8)
                    .foregroundStyle(Palette.secondary)
                if let since = line.since { IntroWaitIndicator(since: since) }      // APP-665: a wait stays alive
                if canSkip, let act {
                    Spacer(minLength: 8)
                    Button("Skip tour") { act(.skipTour) }
                        .buttonStyle(.plain)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(Palette.secondary)
                }
            }
            .frame(height: Self.headerHeight, alignment: .topLeading)
            Text(line.text)
                .font(Font(NotchModel.introFont))
            if let detail = line.detail, !detail.isEmpty {
                Text(detail)
                    .font(Font(NotchModel.introDetailFont))
                    .foregroundStyle(Palette.secondary)
            }
            if !line.checklist.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(line.checklist) { item in
                        Button {
                            if let id = item.actionID { act?(.choose(id)) }
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: item.complete ? "checkmark.circle.fill" : item.active ? "circle.inset.filled" : "circle")
                                    .foregroundStyle(item.complete ? Color.green : item.active ? Color.orange : Color.white.opacity(0.35))
                                Text(item.title).font(.system(size: 12, weight: .medium))
                                    .lineLimit(1).truncationMode(.tail).layoutPriority(1)
                                // HAND-424: the account beside the name, shortened in the middle; it shares the
                                // room with a long name (both shorten) so profiles with one name stay apart, and is
                                // left out when the name already is it
                                if !item.secondary.isEmpty, !item.title.contains(item.secondary) {
                                    Text(item.secondary).font(.system(size: 11))
                                        .foregroundStyle(Palette.secondary)
                                        .lineLimit(1).truncationMode(.middle)
                                        .layoutPriority(1)
                                        .help(item.secondary)
                                }
                                Spacer(minLength: 8)
                                // always whole on one line: a long name or account shortens instead (a friend's
                                // 13 profiles, owner 2026-11-09: the status wrapped and one was pushed out)
                                Text(item.status ?? (item.complete ? "Allowed" : "Required"))
                                    .font(.system(size: 10)).foregroundStyle(Palette.secondary)
                                    .lineLimit(1).fixedSize().layoutPriority(2)
                            }
                            .padding(.horizontal, 8)
                            .frame(height: 32)
                            .contentShape(Rectangle())
                            .background(Color.white.opacity(hoveredPermission == item.id ? 0.08 : item.active ? 0.04 : 0),
                                        in: RoundedRectangle(cornerRadius: 7))
                        }
                        .buttonStyle(.plain)
                        .disabled(item.actionID == nil || act == nil)
                        .onHover { hoveredPermission = $0 ? item.id : hoveredPermission == item.id ? nil : hoveredPermission }
                        .accessibilityHint(item.detail)
                    }
                    Text(checklistDescription)
                        .font(.system(size: 12))
                        .foregroundStyle(Palette.secondary)
                        .lineLimit(5)
                        .frame(maxWidth: .infinity, minHeight: 70, maxHeight: 70, alignment: .topLeading)
                        .padding(.top, 8)
                        .accessibilityIdentifier("permission-description")
                }
            }
            if let act, line.entry != nil || !line.choices.isEmpty {
                actions(act).padding(.top, Self.rowSpacing - Self.sectionSpacing)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(inset)
        .padding(.top, cameraSpace)
        .accessibilityElement(children: .contain)
    }

    private var checklistDescription: String {
        line.checklist.first(where: { $0.id == hoveredPermission })?.detail
            ?? line.checklist.first(where: \.active)?.detail
            ?? line.checklistHint
            ?? "Hover over a permission to see why it's needed. Permissions that don't need a restart can be set up together."
    }

    @ViewBuilder private func actions(_ act: @escaping (IntroAction) -> Void) -> some View {
        VStack(alignment: .leading, spacing: Self.rowSpacing) {
            if let entry = line.entry {
                HStack(spacing: 8) {
                    Group {
                        if entry.secure { SecureField(entry.placeholder, text: typed) }
                        else { TextField(entry.placeholder, text: typed) }
                    }
                    .ferbField()
                    .onSubmit { submit(act) }
                    .onDisappear { typed.wrappedValue = "" }
                    Button("OK") { submit(act) }.buttonStyle(FerbButtonStyle())
                        .disabled(typed.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                .frame(height: Self.rowHeight)
            }
            ForEach(Array(line.choices.enumerated()), id: \.element.id) { index, choice in
                Button { act(.choose(choice.id)) } label: {
                    HStack(spacing: 8) {
                        Text(choice.title).font(Font(NotchModel.introChoiceFont))
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Image(systemName: "arrow.up.right").font(.system(size: 11, weight: .medium))
                            .frame(width: 12)
                            .opacity(0.65)
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
                    .frame(maxWidth: .infinity, minHeight: Self.rowHeight, alignment: .leading)
                }
                .buttonStyle(IntroChoiceStyle(primary: index == 0))
            }
        }
    }

    /// A typed entry goes to the controller and leaves the field at once (an API key is never kept here).
    private func submit(_ act: (IntroAction) -> Void) {
        let text = typed.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines)
        typed.wrappedValue = ""
        guard !text.isEmpty else { return }
        act(.enter(text))
    }
}

/// A line that waits on something (APP-665): a small spinner, and past a few seconds the time it has waited, ticking
/// once a second, so a long wait never looks stuck. Nothing here moves the line or changes its height.
struct IntroWaitIndicator: View {
    let since: Date

    var body: some View {
        HStack(spacing: 6) {
            ProgressView().controlSize(.mini).scaleEffect(0.8)
            TimelineView(.periodic(from: since, by: 1)) { context in
                if let elapsed = IntroLine.elapsed(since: since, now: context.date) {
                    Text(elapsed).font(.system(size: 10, weight: .medium).monospacedDigit())
                        .foregroundStyle(Palette.secondary)
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Working")
    }
}

private struct IntroChoiceStyle: ButtonStyle {
    let primary: Bool
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(primary ? Color.black : Color.white.opacity(0.9))
            .background(primary ? Color.white.opacity(configuration.isPressed ? 0.72 : 0.94)
                        : Color.white.opacity(configuration.isPressed ? 0.16 : 0.08),
                        in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

/// The transcript's reminder details, directly on the notch's black glass.
/// The notch's reminder arrival (APP-622, APP-762): its buttons (Done; a timer's Stop and Repeat) take their own clicks
/// and anywhere else opens today's chat (APP-622, APP-624). For VoiceOver it is one button that opens the chat, with
/// its buttons as named actions (APP-765), so Stop, Repeat and Done are reachable without the pointer.
struct ReminderArrivalSurface: View {
    let card: ResultCard
    let text: String
    @ObservedObject var model: NotchModel

    var body: some View {
        ScrollView { ReminderArrivalView(card: card, text: text, model: model) }
            .contentShape(Rectangle())
            .pointingHand("reminder")
            .onTapGesture { model.openReminderChat() }
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.isButton)
            .accessibilityHint("Open reminder in chat")
            .accessibilityAction { model.openReminderChat() }
            .accessibilityActions {
                ForEach(model.arrivalActions(card)) { action in Button(action.name) { action.run() } }
            }
    }
}

struct ReminderArrivalView: View {
    let card: ResultCard
    let text: String
    var model: NotchModel? = nil

    var body: some View {
        ReminderCard(card: card, framed: false, model: model)
            .padding(.horizontal, 26).padding(.top, 16).padding(.bottom, 24)
    }
}
