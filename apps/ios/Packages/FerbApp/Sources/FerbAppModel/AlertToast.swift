import Foundation

/// FERB's alerts as a toast (kb APP-1109; FERB.app NotchCore/OwnerAlerts.swift; Android model/AlertToast.kt): the newest
/// one floats over the chat's foot and leaves by itself after `seconds` in front of the owner (put away on every
/// device), unless the stack is open or his finger is on it. Every phone alert comes from the Mac and none has a button,
/// so every one leaves by itself.
public enum AlertToast {
    /// FERB.app's `OwnerAlerts.toastSeconds`: the six seconds of its planning preview (APP-624).
    public static let seconds: Double = 6
}
