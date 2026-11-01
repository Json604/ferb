import AppKit
import SwiftUI
import NotchCore

/// FERB's intro lines in the notch: setting `introLine` swells the notch out of the camera housing to show the line
/// (the same native surface and spring as the plan preview, NotchPresentation), clearing it shrinks it back. The chat
/// stays closed throughout.
extension NotchModel {
    static let introFont = NSFont.systemFont(ofSize: 15, weight: .medium)
    static let introDetailFont = NSFont.systemFont(ofSize: 12)

    /// The surface's height for a line (nil: no line): the text measured at the surface's width, plus the camera's
    /// space when the notch is docked.
    func introSurfaceHeight(_ line: IntroLine?) -> CGFloat? {
        guard let line else { return nil }
        let width = max(1, bodyWidth - 50)
        func measured(_ text: String, _ font: NSFont) -> CGFloat {
            ceil((text as NSString).boundingRect(
                with: CGSize(width: width, height: .greatestFiniteMagnitude),
                options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font]).height)
        }
        var height = measured(line.text, Self.introFont)
        if let detail = line.detail, !detail.isEmpty { height += 4 + measured(detail, Self.introDetailFont) }
        let cameraSpace = isNotchDocked ? cutoutHeight : 0
        return min(availableSurfaceSize.height, height + 34 + cameraSpace)
    }

    func introLineChanged(from old: IntroLine?) {
        guard introLine != old else { return }
        if let introLine { introDisplayLine = introLine }
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

enum NotchCompactContent: Equatable {
    case intro(IntroLine)
    case plan(String)
    case pulse
}

struct IntroNarrationView: View {
    let line: IntroLine
    let cameraSpace: CGFloat

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(line.text)
                .font(Font(NotchModel.introFont))
            if let detail = line.detail, !detail.isEmpty {
                Text(detail)
                    .font(Font(NotchModel.introDetailFont))
                    .foregroundStyle(Palette.secondary)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 25)
        .padding(.vertical, 17)
        .padding(.top, cameraSpace)
        .accessibilityElement(children: .combine)
    }
}
