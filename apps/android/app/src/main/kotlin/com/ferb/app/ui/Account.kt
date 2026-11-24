package com.ferb.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Logout
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.UiState
import com.ferb.app.model.Provider
import com.ferb.app.model.SignInStep
import com.ferb.design.FerbIcons
import com.ferb.design.Ink
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.Opens
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.RowIcon
import com.ferb.design.components.Sheet as SheetFrame
import com.ferb.design.components.SheetTitle
import com.ferb.design.screens.MacFirstPage
import com.ferb.design.screens.MacFirstWords
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.SignInPage
import com.ferb.design.screens.SignInWords

/** The sign-in page (kb PHN-827): after the lock, before anything else, while signed out of FERB's account. */
@Composable
internal fun SignIn(vm: FerbViewModel, ui: UiState, insets: PhoneInsets) {
    val a = ui.account
    val waiting = a.step != SignInStep.IDLE || "signin" in ui.busy
    SignInPage(
        SignInWords(stringResource(R.string.signin_title), stringResource(R.string.signin_text), stringResource(R.string.signin_google),
            stringResource(R.string.signin_apple), stringResource(R.string.signin_footnote)),
        insets,
        line = a.note?.let { noteText(it) },
        finishing = if (waiting) stringResource(R.string.signin_finishing) else null,
        onGoogle = { vm.signIn(Provider.GOOGLE) },
        onApple = { vm.signIn(Provider.APPLE) },
    )
}

/** Signed in, no Mac paired yet (kb PHN-829): FERB runs on the Mac; send the link; then pair. */
@Composable
internal fun MacFirst(vm: FerbViewModel, ui: UiState, insets: PhoneInsets) {
    val email = ui.account.account?.email.orEmpty()
    MacFirstPage(
        MacFirstWords(stringResource(R.string.macfirst_title), stringResource(R.string.macfirst_text, email), stringResource(R.string.macfirst_site),
            stringResource(R.string.macfirst_send), stringResource(R.string.macfirst_pair), stringResource(R.string.signin_signed_in_as, email)),
        insets,
        line = ui.linkNote?.let { noteText(it) },
        sending = "send-link" in ui.busy,
        onSend = { vm.sendLink() },
        onPair = { vm.pairNow(true) },
        onAccount = { vm.show(Sheet.Account) },
    )
}

/** "with Google", "with Apple" or both, from the account's providers (the server's words for them). */
@Composable
internal fun providerWords(providers: List<String>): String? {
    val google = "google" in providers
    val apple = "apple" in providers
    return when {
        google && apple -> stringResource(R.string.account_with_both)
        google -> stringResource(R.string.account_with_google)
        apple -> stringResource(R.string.account_with_apple)
        else -> null
    }
}

/** This phone's settings' Account row: who is signed in; a tap opens the account page. */
@Composable
internal fun AccountRow(vm: FerbViewModel, ui: UiState) {
    val a = ui.account.account ?: return
    CardRow(stringResource(R.string.account_signed_in_as, a.email), providerWords(a.providers), leading = { RowIcon(FerbIcons.person) },
        onClick = { vm.show(Sheet.Account) }, trailing = { Opens() })
}

/** The account page: who is signed in and how, Sign out, Delete account…, and what the last try said. */
@Composable
internal fun AccountSheet(vm: FerbViewModel, ui: UiState, insets: PhoneInsets, close: () -> Unit) {
    val a = ui.account.account ?: return
    val title = stringResource(R.string.account_title)
    val busy = "account" in ui.busy
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = close) {
        SheetTitle(title, close)
        Column(Modifier.padding(top = 8.dp).outset(14.dp)) {
            CardRow(stringResource(R.string.account_signed_in_as, a.email), providerWords(a.providers), leading = { RowIcon(FerbIcons.person) })
            Divide()
            CardRow(stringResource(R.string.account_sign_out), onClick = if (busy) null else ({ vm.show(Sheet.SignOut) }),
                leading = { RowIcon(Icons.AutoMirrored.Outlined.Logout) })
            Divide()
            CardRow(stringResource(R.string.account_delete), danger = true, onClick = if (busy) null else ({ vm.show(Sheet.DeleteAccount) }),
                leading = { RowIcon(Icons.Outlined.Delete, danger = true) })
        }
        val line = ui.accountNote?.let { noteText(it) } ?: if (ui.account.checkFailed) stringResource(R.string.account_check_failed) else null
        if (line != null) {
            PhoneText(line, 14f, color = Ink.detail, lineSpacing = 2f,
                modifier = Modifier.fillMaxWidth().padding(top = 12.dp).semantics { liveRegion = LiveRegionMode.Polite })
        }
    }
}

/** Sign out's question (the spec's words, as the sheet's title and text). */
@Composable
internal fun SignOutSheet(vm: FerbViewModel, ui: UiState, insets: PhoneInsets) {
    val back = { vm.show(Sheet.Account) }
    Plain(stringResource(R.string.signout_title), stringResource(R.string.signout_text), insets, back) {
        Pill(stringResource(R.string.signout_yes), Modifier.padding(top = 18.dp), onClick = if ("account" in ui.busy) null else ({ vm.signOut() }))
        Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = back)
    }
}

/** Delete account's question: the server must delete it before anything here goes. */
@Composable
internal fun DeleteAccountSheet(vm: FerbViewModel, ui: UiState, insets: PhoneInsets) {
    val email = ui.account.account?.email ?: return
    val back = { vm.show(Sheet.Account) }
    Plain(stringResource(R.string.delete_title), stringResource(R.string.delete_text, email), insets, back) {
        Pill(stringResource(R.string.delete_yes), Modifier.padding(top = 18.dp), onClick = if ("account" in ui.busy) null else ({ vm.deleteAccount() }))
        Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = back)
    }
}
