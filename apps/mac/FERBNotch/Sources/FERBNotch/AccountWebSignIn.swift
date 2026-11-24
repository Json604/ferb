import AppKit
import AuthenticationServices
import NotchCore

/// The browser step of FERB's account sign-in (APP-1128): ASWebAuthenticationSession, the system's own sign-in
/// browser. It catches `ferb-auth://callback` itself (no URL scheme is registered in Info.plist, so no page can open
/// FERB.app with one), and it shares the browser's cookies (not ephemeral), so a Google or Apple account already
/// signed in there is offered at once. Only the start address and the callback pass through FERB; the person's
/// password and two-factor codes stay with Google or Apple.
@MainActor
final class AccountWebSignIn: NSObject, WebSignIn, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?

    func run(_ url: URL, callbackScheme: String) async -> WebSignInResult {
        session?.cancel()
        return await withCheckedContinuation { (continuation: CheckedContinuation<WebSignInResult, Never>) in
            let session = ASWebAuthenticationSession(url: url, callbackURLScheme: callbackScheme) { callback, error in
                let result: WebSignInResult
                if let callback {
                    result = .callback(callback)
                } else if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
                    result = .cancelled
                } else {
                    result = .failed
                }
                continuation.resume(returning: result)
            }
            session.prefersEphemeralWebBrowserSession = false
            session.presentationContextProvider = self
            self.session = session
            if !session.start() {
                self.session = nil
                continuation.resume(returning: .failed)
            }
        }
    }

    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            NSApp.keyWindow ?? NSApp.windows.first { $0.isVisible } ?? NSWindow()
        }
    }
}
