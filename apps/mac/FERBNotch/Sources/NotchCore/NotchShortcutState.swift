import Foundation

public struct ShortcutModifiers: OptionSet {
    public let rawValue: UInt
    public init(rawValue: UInt) { self.rawValue = rawValue }
    public static let rightOption = Self(rawValue: 1 << 0)
    public static let leftOption = Self(rawValue: 1 << 1)
    public static let rightCommand = Self(rawValue: 1 << 2)
    public static let leftCommand = Self(rawValue: 1 << 3)
    public static let control = Self(rawValue: 1 << 4)
    public static let shift = Self(rawValue: 1 << 5)
    public static let function = Self(rawValue: 1 << 6)
}

public enum NotchShortcutAction: Equatable {
    case togglePanel, startVoice, finishVoice, cancelVoice
}

/// Modifier-only recognition; duplicate local/global deliveries are harmless.
public struct NotchShortcutState {
    private var rightOptionDown = false
    private var voiceHeld = false
    private var voiceBlocked = false
    private var voiceKeyCount: UInt32 = 0
    private var tapStartedAt: TimeInterval?
    private var tapKeyCount: UInt32 = 0
    private var firstTapReleasedAt: TimeInterval?
    private var firstTapKeyCount: UInt32 = 0
    /// Double-tap right Option opens and closes FERB unless the owner switched it off (D24); hold-to-talk is unaffected.
    public var doubleTapEnabled = true

    public init() {}

    public mutating func update(_ modifiers: ShortcutModifiers, at time: TimeInterval,
                                keyCount: UInt32) -> [NotchShortcutAction] {
        var actions: [NotchShortcutAction] = []
        let optionDown = !modifiers.intersection([.rightOption, .leftOption]).isEmpty
        let forbidden = !modifiers.intersection([.leftCommand, .control, .shift, .function]).isEmpty
        let voiceChord = modifiers.contains(.rightCommand) && optionDown && !forbidden
        if voiceHeld && (!voiceChord || keyCount != voiceKeyCount) {
            voiceHeld = false
            let cancelled = forbidden || keyCount != voiceKeyCount
            voiceBlocked = cancelled
            actions.append(cancelled ? .cancelVoice : .finishVoice)
        }
        if !optionDown && !modifiers.contains(.rightCommand) { voiceBlocked = false }
        if voiceChord && !voiceHeld && !voiceBlocked {
            voiceHeld = true
            voiceKeyCount = keyCount
            actions.append(.startVoice)
        }

        let rightDown = modifiers.contains(.rightOption)
        if !modifiers.subtracting(.rightOption).isEmpty {
            tapStartedAt = nil
            firstTapReleasedAt = nil
        }
        if rightDown != rightOptionDown {
            rightOptionDown = rightDown
            if rightDown && modifiers == .rightOption {
                tapStartedAt = time
                tapKeyCount = keyCount
            } else if !rightDown, let began = tapStartedAt {
                tapStartedAt = nil
                if time - began < 0.22 && tapKeyCount == keyCount {
                    if let first = firstTapReleasedAt, time - first <= 0.36, firstTapKeyCount == keyCount {
                        if doubleTapEnabled { actions.append(.togglePanel) }
                        firstTapReleasedAt = nil
                    } else {
                        firstTapReleasedAt = time
                        firstTapKeyCount = keyCount
                    }
                } else { firstTapReleasedAt = nil }
            }
        }
        return actions
    }

    public mutating func reset() -> [NotchShortcutAction] {
        let actions: [NotchShortcutAction] = voiceHeld ? [.cancelVoice] : []
        let doubleTap = doubleTapEnabled
        self = Self()
        doubleTapEnabled = doubleTap
        return actions
    }
}
