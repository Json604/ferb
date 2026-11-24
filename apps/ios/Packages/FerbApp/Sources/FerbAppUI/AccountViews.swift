import AuthenticationServices
import FerbAppModel
import FerbDesign
import SwiftUI
import UIKit

/// The system's web sign-in sheet (kb PHN-841): ASWebAuthenticationSession on the site's start page, its
/// callback `ferb-auth://callback` caught by the session (no URL scheme registered by the app), Safari's own sign-ins
/// shared (prefersEphemeralWebBrowserSession = false: Google's page lists the accounts already signed in, as the owner
/// asked). iOS asks "FERB wants to use … to sign in" first for a sheet that shares Safari's sign-ins; that alert is the
/// system's and can't be left out of a shared sheet (SFSafariViewController hasn't shared Safari's cookies since iOS
/// 11). Shown over the key window. Apple uses it only when the system's own Sign in with Apple can't show.
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

/// The system's own Sign in with Apple (kb PHN-841): Apple's sheet with Face ID, no web page and no alert. The request
/// asks for the name and email and carries the nonce's SHA-256; Apple's identity token comes back for the site to check
/// (/api/app/apple). Cancel is `.cancelled`; anything else the system can't do is `.failed`, and the web sheet is used.
@MainActor
final class SystemAppleSignIn: NSObject, AppleSignIn, ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding {
    private var pending: CheckedContinuation<AppleSignInResult, Never>?
    private var controller: ASAuthorizationController?

    func authorize(nonceHash: String) async -> AppleSignInResult {
        settle(.cancelled)
        let request = ASAuthorizationAppleIDProvider().createRequest()
        request.requestedScopes = [.fullName, .email]
        request.nonce = nonceHash
        let c = ASAuthorizationController(authorizationRequests: [request])
        c.delegate = self
        c.presentationContextProvider = self
        controller = c
        return await withCheckedContinuation { (cont: CheckedContinuation<AppleSignInResult, Never>) in
            pending = cont
            c.performRequests()
        }
    }

    private func settle(_ result: AppleSignInResult) {
        controller = nil
        pending?.resume(returning: result)
        pending = nil
    }

    nonisolated func authorizationController(controller: ASAuthorizationController,
                                             didCompleteWithAuthorization authorization: ASAuthorization) {
        let credential = authorization.credential as? ASAuthorizationAppleIDCredential
        let token = credential?.identityToken.flatMap { String(data: $0, encoding: .utf8) }
        let name = credential?.fullName.map { PersonNameComponentsFormatter.localizedString(from: $0, style: .default) }
        MainActor.assumeIsolated {
            guard let token, !token.isEmpty else { return settle(.failed) }
            settle(.credential(identityToken: token, name: name?.trimmingCharacters(in: .whitespacesAndNewlines)))
        }
    }

    nonisolated func authorizationController(controller: ASAuthorizationController, didCompleteWithError error: Error) {
        let cancelled = (error as? ASAuthorizationError)?.code == .canceled
        MainActor.assumeIsolated { settle(cancelled ? .cancelled : .failed) }
    }

    nonisolated func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
            return windows.first(where: \.isKeyWindow) ?? windows.first ?? ASPresentationAnchor()
        }
    }
}

/// The sign-in page with the app's words (kb PHN-839).
struct SignInView: View {
    let account: AccountModel
    let insets: PhoneInsets

    var body: some View {
        SignInPage(title: L.s("signin_title"), text: L.s("signin_text"), google: L.s("signin_google"),
                   apple: L.s("signin_apple"), footnote: L.s("signin_footnote"), line: account.line,
                   working: account.phase != .idle, insets: insets,
                   onGoogle: { account.signIn(.google) }, onApple: { account.signIn(.apple) })
    }
}

/// "FERB runs on your Mac" with the app's words (kb PHN-841): phone first, signed in, no Mac paired yet. "Use another
/// account" signs out at once, nothing being paired to lose (owner kb PHN-842), with its pending state.
struct MacFirstView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        let email = model.account.account?.email ?? ""
        MacFirstPage(title: L.s("macfirst_title"), text: L.s("macfirst_text", email),
                     address: L.s("macfirst_site"), pairTitle: L.s("macfirst_pair"),
                     signedInAs: L.s("signin_signed_in_as", email), otherTitle: L.s("account_other"),
                     signingOut: model.signingOut ? L.s("account_signing_out") : nil, insets: insets,
                     onPair: { model.choosePair() }, onAccount: { model.show(.account) }, onOther: { model.signOut() })
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
