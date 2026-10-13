import AppKit
import Carbon
import NotchCore

/// Watches modifier changes without capturing ordinary typed characters.
@MainActor
final class NotchShortcutMonitor: NSObject {
    var onDoubleTap: (() -> Void)?
    var onHoldBegan: (() -> Void)?
    var onHoldEnded: (() -> Void)?
    var onHoldCancelled: (() -> Void)?

    private var globalHandler: EventHandlerRef?
    private var localMonitor: Any?
    private var recognition = NotchShortcutState()

    func start() {
        var eventType = EventTypeSpec(eventClass: OSType(kEventClassKeyboard),
                                      eventKind: UInt32(kEventRawKeyModifiersChanged))
        let context = Unmanaged.passUnretained(self).toOpaque()
        let status = InstallEventHandler(GetEventMonitorTarget(), { _, event, context in
            guard let event, let context, let native = NSEvent(eventRef: UnsafeRawPointer(event)) else {
                return OSStatus(eventNotHandledErr)
            }
            let flags = native.modifierFlags
            let timestamp = native.timestamp
            let observer = Unmanaged<NotchShortcutMonitor>.fromOpaque(context).takeUnretainedValue()
            observer.enqueueModifiers(flags, timestamp: timestamp,
                keyCount: CGEventSource.counterForEventType(.combinedSessionState, eventType: .keyDown))
            return OSStatus(eventNotHandledErr)
        }, 1, &eventType, context, &globalHandler)
        if status != noErr { NSLog("FERB: modifier shortcut monitor unavailable (%d).", status) }

        localMonitor = NSEvent.addLocalMonitorForEvents(matching: .flagsChanged) { [weak self] event in
            self?.enqueueModifiers(event.modifierFlags, timestamp: event.timestamp,
                keyCount: CGEventSource.counterForEventType(.combinedSessionState, eventType: .keyDown))
            return event
        }
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(reset),
                                                          name: NSWorkspace.willSleepNotification, object: nil)
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(reset),
                                                          name: NSWorkspace.sessionDidResignActiveNotification, object: nil)
    }

    // Carbon callbacks are not Swift executor contexts, even on the main OS
    // thread. Queue both sources identically; snapshot the key counter now so
    // later keyboard events cannot change this chord's interpretation.
    private nonisolated func enqueueModifiers(_ flags: NSEvent.ModifierFlags,
                                               timestamp: TimeInterval, keyCount: UInt32) {
        DispatchQueue.main.async { [weak self] in
            guard let self, self.globalHandler != nil || self.localMonitor != nil else { return }
            let actions = self.recognition.update(Self.modifiers(from: flags), at: timestamp,
                                                   keyCount: keyCount)
            self.dispatch(actions)
        }
    }

    static func modifiers(from flags: NSEvent.ModifierFlags) -> ShortcutModifiers {
        var result: ShortcutModifiers = []
        // Device-specific masks from IOKit/hidsystem/IOLLEvent.h.
        if flags.rawValue & 0x40 != 0 { result.insert(.rightOption) }
        if flags.rawValue & 0x20 != 0 { result.insert(.leftOption) }
        if flags.rawValue & 0x10 != 0 { result.insert(.rightCommand) }
        if flags.rawValue & 0x08 != 0 { result.insert(.leftCommand) }
        if flags.contains(.option), result.intersection([.rightOption, .leftOption]).isEmpty {
            result.insert(.leftOption)
        }
        if flags.contains(.command), result.intersection([.rightCommand, .leftCommand]).isEmpty {
            result.insert(.leftCommand)
        }
        if flags.contains(.control) { result.insert(.control) }
        if flags.contains(.shift) { result.insert(.shift) }
        if flags.contains(.function) { result.insert(.function) }
        return result
    }

    private func dispatch(_ actions: [NotchShortcutAction]) {
        for action in actions {
            switch action {
            case .togglePanel: onDoubleTap?()
            case .startVoice: onHoldBegan?()
            case .finishVoice: onHoldEnded?()
            case .cancelVoice: onHoldCancelled?()
            }
        }
    }

    @objc private func reset() { dispatch(recognition.reset()) }

    func stop() {
        reset()
        if let globalHandler { RemoveEventHandler(globalHandler) }
        globalHandler = nil
        if let localMonitor { NSEvent.removeMonitor(localMonitor) }
        localMonitor = nil
        NSWorkspace.shared.notificationCenter.removeObserver(self)
    }
}
