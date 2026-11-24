import AppKit
import NotchCore

/// The browser step of FERB's account sign-in (APP-1138, REL-452): the site's start page opens in the person's default
/// browser, like other apps' sign-ins, with the Google or Apple account already signed in there; the site sends the
/// browser back to FERB.app's loopback address (`LoopbackReturn`, 127.0.0.1 only). No system sign-in window and no
/// "wants to use … to sign in" alert. Only the start address and the callback pass through FERB; the person's password
/// and two-factor codes stay with Google or Apple.
enum AccountWebSignIn {
    @MainActor
    static func make() -> BrowserSignIn {
        // FERB comes forward only where that can't switch the owner's Space (APP-796); the account step's answer shows
        // in the notch, on every Space, either way.
        BrowserSignIn(open: { NSWorkspace.shared.open($0) },
                      bringForward: { AppWindowPresentation.comeForward("the account sign-in") })
    }
}
