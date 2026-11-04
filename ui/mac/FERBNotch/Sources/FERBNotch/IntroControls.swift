import Foundation

/// The controls the tour teaches by doing (kb area app): double-tap right ⌥ (open or close), hold
/// right ⌘+⌥ (talk), Stop on a real task the tour starts (APP-667), and Pause FERB from the menu.
enum IntroControl: CaseIterable {
    case toggle, talk, stop, pause
}

/// Where the tour hears the owner use a control. PanelCoordinator reports the real ones (NotchShortcutMonitor's
/// double-tap and hold, the menu's Pause FERB) to its `IntroControlSignals`; Stop comes from the tour's own line, on
/// the task it started (IntroController.practiseStop).
@MainActor
protocol IntroControlFeeding: AnyObject {
    var onControl: ((IntroControl) -> Void)? { get set }
}

@MainActor
final class IntroControlSignals: IntroControlFeeding {
    var onControl: ((IntroControl) -> Void)?
    func saw(_ control: IntroControl) { onControl?(control) }
}

/// The tour's waits (a line's time on screen, the first-launch clip, a poll); tests make them instant or held.
@MainActor
protocol IntroSleeping {
    func sleep(seconds: Double) async
}

struct IntroTaskSleeper: IntroSleeping {
    func sleep(seconds: Double) async { try? await Task.sleep(for: .seconds(seconds)) }
}
