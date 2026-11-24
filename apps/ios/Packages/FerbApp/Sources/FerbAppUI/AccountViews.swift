import AuthenticationServices
import FerbAppModel
import FerbDesign
import SwiftUI
import UIKit

/// The system's web sign-in sheet (kb PHN-827): ASWebAuthenticationSession on the site's start page, its callback
/// `ferb-auth://callback` caught by the session (no URL scheme registered by the app), the browser's own cookies kept
/// (prefersEphemeralWebBrowserSession = false: the person's Google or Apple sign-in in Safari can be reused), shown over
/// the key window. Apple goes through this same page (one path on every platform).
@MainActor
final class SystemWebSignIn: NSObject, WebSignIn, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?

    func open(_ url: URL, callbackScheme: String) async -> WebSignInResult {
        await withCheckedContinuation { (cont: CheckedContinuation<WebSignInResult, Never>) in
            let done: (URL?, Error?) -> Void = { [weak self] callback, error in
                Task { @MainActor in self?.session = nil }
                if let callback { cont.resume(returning: .callback(callback)); return }
                if let e = error as? ASWebAuthenticationSessionError, e.code == .canceledLogin {
                    cont.resume(returning: .cancelled)
                } else {
                    cont.resume(returning: .failed)
                }
            }
            let s: ASWebAuthenticationSession
            if #available(iOS 17.4, *) {
                s = ASWebAuthenticationSession(url: url, callback: .customScheme(callbackScheme), completionHandler: done)
            } else {
                s = ASWebAuthenticationSession(url: url, callbackURLScheme: callbackScheme, completionHandler: done)
            }
            s.prefersEphemeralWebBrowserSession = false
            s.presentationContextProvider = self
            session = s
            if !s.start() {
                session = nil
                cont.resume(returning: .failed)
            }
        }
    }

    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
            return windows.first(where: \.isKeyWindow) ?? windows.first ?? ASPresentationAnchor()
        }
    }
}

/// The sign-in page with the app's words (kb PHN-827).
struct SignInView: View {
    let account: AccountModel
    let insets: PhoneInsets

    var body: some View {
        SignInPage(title: L.s("signin_title"), text: L.s("signin_text"), google: L.s("signin_google"),
                   apple: L.s("signin_apple"), footnote: L.s("signin_footnote"), line: Self.line(account),
                   working: account.phase != .idle, insets: insets,
                   onGoogle: { account.signIn(.google) }, onApple: { account.signIn(.apple) })
    }

    static func line(_ account: AccountModel) -> String? {
        account.phase != .idle ? L.s("signin_finishing") : account.problem?.text
    }
}

/// "FERB runs on your Mac" with the app's words (kb PHN-840): phone first, signed in, no Mac paired yet.
struct MacFirstView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        let a = model.account
        MacFirstPage(title: L.s("macfirst_title"), text: L.s("macfirst_text", a.account?.email ?? ""),
                     address: L.s("macfirst_site"), pairTitle: L.s("macfirst_pair"), otherTitle: L.s("account_other"),
                     insets: insets, onPair: { model.choosePair() }, onOther: { model.show(.signOut) })
    }
}

/// The account's sheet (This phone → the account's row): who is signed in and how, the refresh's line when it
/// couldn't check, Sign out, Delete account….
struct AccountSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let close: () -> Void

    var body: some View {
        let a = model.account
        Sheet(bottomInset: insets.bottom, onDismiss: close) {
            SheetTitle(L.s("account_title"), onClose: close)
            if a.checkFailed {
                Text(L.s("account_check_failed")).type(15, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 6)
            }
            Card {
                CardRow(title: L.s("account_signed_in_as", a.account?.email ?? ""),
                        detail: AccountModel.providerWords(a.account?.providers ?? [])) { RowIcon(symbol: "person.crop.circle") }
                Divide()
                CardRow(title: L.s("account_sign_out"), onClick: { model.show(.signOut) }) { RowIcon(symbol: "rectangle.portrait.and.arrow.right") }
                Divide()
                CardRow(title: L.s("account_delete"), danger: true, onClick: { model.show(.deleteAccount) }) { RowIcon(symbol: "trash", danger: true) }
            }
            .padding(.horizontal, -14)
            .padding(.top, 8)
        }
    }
}

/// Sign out's confirmation (the phone leaves its Mac with it) and Delete account's (the site must delete it first).
struct SignOutSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let close: () -> Void

    var body: some View {
        let busy = model.ui.busy.contains("account")
        Plain(title: L.s("signout_title"), text: L.s("signout_text"), insets: insets, close: close) {
            Pill(title: L.s("signout_yes"), kind: .black, onClick: busy ? nil : { model.signOut() }).padding(.top, 18)
            Pill(title: L.s("cancel"), kind: .plain, height: 44, onClick: close).padding(.top, 4)
        }
    }
}

struct DeleteAccountSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let close: () -> Void

    var body: some View {
        let busy = model.ui.busy.contains("account")
        Plain(title: L.s("delete_title"), text: L.s("delete_text", model.account.account?.email ?? ""),
              insets: insets, close: close) {
            if let p = model.ui.accountProblem {
                Text(p.text).type(15, .medium, Ink.red).fixedSize(horizontal: false, vertical: true).padding(.top, 12)
            }
            Pill(title: L.s("delete_yes"), kind: .black, onClick: busy ? nil : { model.deleteAccount() }).padding(.top, 18)
            Pill(title: L.s("cancel"), kind: .plain, height: 44, onClick: close).padding(.top, 4)
        }
    }
}
