import AppKit
import SwiftUI
import NotchCore

/// FERB's intro lines in the notch: setting `introLine` swells the notch out of the camera housing to show the line
/// (the same native surface and spring as the plan preview, NotchPresentation), clearing it shrinks it back. The chat
/// stays closed throughout.
extension NotchModel {
    static let introFont = NSFont.systemFont(ofSize: 15, weight: .medium)
    static let introDetailFont = NSFont.systemFont(ofSize: 12)

    /// The surface's height for a line (nil: no line): its content, plus the camera's space when the notch is docked.
    func introSurfaceHeight(_ line: IntroLine?) -> CGFloat? {
        guard let line else { return nil }
        let cameraSpace = isNotchDocked ? cutoutHeight : 0
        return min(availableSurfaceSize.height, introContentHeight(line) + cameraSpace)
    }

    /// The line's text measured at the surface's width, and the rows of its field, choices and Skip.
    func introContentHeight(_ line: IntroLine) -> CGFloat {
        let width = max(1, bodyWidth - 50)
        func measured(_ text: String, _ font: NSFont, _ width: CGFloat) -> CGFloat {
            ceil((text as NSString).boundingRect(
                with: CGSize(width: width, height: .greatestFiniteMagnitude),
                options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font]).height)
        }
        var height = measured(line.text, Self.introFont, width)
        if let detail = line.detail, !detail.isEmpty { height += 4 + measured(detail, Self.introDetailFont, width) }
        guard onIntroAction != nil else { return height + 34 }     // read only: no field, choices or Skip
        if line.entry != nil { height += IntroNarrationView.rowSpacing + IntroNarrationView.rowHeight }
        if IntroNarrationView.stacksChoices(line.choices) {
            for choice in line.choices {
                height += IntroNarrationView.rowSpacing
                    + max(IntroNarrationView.rowHeight, measured(choice.title, Self.introChoiceFont, width - 24) + 12)
            }
        }
        // Short choices share one row with Skip; stacked ones leave Skip a row of its own.
        return height + IntroNarrationView.rowSpacing + IntroNarrationView.rowHeight + 34
    }

    static let introChoiceFont = NSFont.systemFont(ofSize: 12.5, weight: .medium)

    /// The owner did something on the intro line (the intro controller acts on it).
    func introAction(_ action: IntroAction) { onIntroAction?(action) }

    /// What the line's buttons call while the tour runs; nil: the line is only read.
    var introActions: ((IntroAction) -> Void)? {
        guard onIntroAction != nil else { return nil }
        return { [weak self] action in self?.introAction(action) }
    }

    func introLineChanged(from old: IntroLine?) {
        guard introLine != old else { return }
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
        guard !expanded else { return nil }
        if let introLine { return .intro(introLine) }
        if compactPlanText != nil, let text = compactPlanDisplayText { return .plan(text) }
        if introPulsing { return .pulse }
        if let introDisplayLine { return .intro(introDisplayLine) }
        return compactPlanDisplayText.map { .plan($0) }
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
    case skip
    case stopPreview
}

enum NotchCompactContent: Equatable {
    case intro(IntroLine)
    case plan(String)
    case pulse
}

struct IntroNarrationView: View {
    static let rowHeight: CGFloat = 28
    static let rowSpacing: CGFloat = 8

    let line: IntroLine
    let cameraSpace: CGFloat
    /// Nil: the line is only read (no choices or Skip are offered).
    var act: ((IntroAction) -> Void)? = nil
    /// The notch's surface pads the line like the plan preview; the strip in the open chat is tighter.
    var inset = EdgeInsets(top: 17, leading: 25, bottom: 17, trailing: 25)
    @State private var typed = ""

    /// Long choices (a suggested task) stack, one per row; short ones share a row.
    static func stacksChoices(_ choices: [IntroChoice]) -> Bool { choices.contains { $0.title.count > 26 } }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(line.text)
                .font(Font(NotchModel.introFont))
            if let detail = line.detail, !detail.isEmpty {
                Text(detail)
                    .font(Font(NotchModel.introDetailFont))
                    .foregroundStyle(Palette.secondary)
            }
            if let act { actions(act).padding(.top, Self.rowSpacing - 4) }
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(inset)
        .padding(.top, cameraSpace)
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder private func actions(_ act: @escaping (IntroAction) -> Void) -> some View {
        VStack(alignment: .leading, spacing: Self.rowSpacing) {
            if let entry = line.entry {
                HStack(spacing: 8) {
                    Group {
                        if entry.secure { SecureField(entry.placeholder, text: $typed) }
                        else { TextField(entry.placeholder, text: $typed) }
                    }
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { submit(act) }
                    Button("OK") { submit(act) }.buttonStyle(FerbButtonStyle())
                        .disabled(typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                .frame(height: Self.rowHeight)
            }
            if Self.stacksChoices(line.choices) {
                ForEach(line.choices, id: \.id) { choice in
                    Button { act(.choose(choice.id)) } label: {
                        Text(choice.title).font(Font(NotchModel.introChoiceFont))
                            .frame(maxWidth: .infinity, minHeight: Self.rowHeight - 12, alignment: .leading)
                    }
                    .buttonStyle(FerbButtonStyle(quiet: true))
                }
                skip(act)
            } else {
                HStack(spacing: 8) {
                    ForEach(Array(line.choices.enumerated()), id: \.element.id) { index, choice in
                        Button(choice.title) { act(.choose(choice.id)) }
                            .buttonStyle(FerbButtonStyle(quiet: index > 0))
                    }
                    Spacer(minLength: 0)
                    skip(act)
                }
                .frame(height: Self.rowHeight)
            }
        }
    }

    private func skip(_ act: @escaping (IntroAction) -> Void) -> some View {
        Button("Skip intro") { act(.skip) }
            .buttonStyle(.plain)
            .font(.system(size: 11))
            .foregroundStyle(Palette.secondary)
            .frame(maxWidth: .infinity, minHeight: Self.rowHeight, alignment: .trailing)
    }

    /// A typed entry goes to the controller and leaves the field at once (an API key is never kept here).
    private func submit(_ act: (IntroAction) -> Void) {
        let text = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        typed = ""
        guard !text.isEmpty else { return }
        act(.enter(text))
    }
}
