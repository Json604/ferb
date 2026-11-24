package com.ferb.design.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.Card
import com.ferb.design.components.FerbMark
import com.ferb.design.components.LinkKind
import com.ferb.design.components.LinkPicture
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.Mark
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.pressable

// FERB's account on the phone (kb PHN-839, PHN-841). Not from the lab: built from its pieces, the lock page's mark
// and title over the sky, the pair page's white card at the bottom, the black pills. Stateless: the app passes its
// words (new words live in the app's strings for the owner's approval).

/** What the sign-in page says: its title and line, the two ways in, the footnote. */
class SignInWords(val title: String, val text: String, val google: String, val apple: String, val footnote: String)

/**
 * The sign-in page: FERB's mark, "Welcome to FERB" and its line, then a card with the two ways in and the footnote.
 * [line]: what happened last (cancelled, can't reach, …), or null; [finishing]: the browser has the sign-in now, the
 * buttons wait.
 */
@Composable
fun SignInPage(
    words: SignInWords,
    insets: PhoneInsets = PhoneInsets.Lab,
    line: String? = null,
    finishing: String? = null,
    onGoogle: (() -> Unit)? = {},
    onApple: (() -> Unit)? = {},
) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Spacer(Modifier.height(150.dp))
        FerbMark(84.dp)
        PhoneText(words.title, FerbType.Role.lockTitle, FerbType.Semibold, ink.top,
            Modifier.padding(top = 6.dp, start = 24.dp, end = 24.dp).semantics { heading() }, align = TextAlign.Center)
        PhoneText(words.text, 16f, FerbType.Medium, ink.top, Modifier.padding(horizontal = 32.dp), lineSpacing = 2f, align = TextAlign.Center)
        Spacer(Modifier.weight(1f))
        Card(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), padding = 16.dp) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                if (finishing != null) {
                    Row(Modifier.fillMaxWidth().padding(bottom = 4.dp).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
                        horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically) {
                        Mark(MarkKind.RUNNING, 20.dp)
                        PhoneText(finishing, 15f, color = Ink.detail)
                    }
                } else if (line != null) {
                    PhoneText(line, 15.5f, color = Ink.black, lineSpacing = 2f,
                        modifier = Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, bottom = 4.dp).semantics { liveRegion = LiveRegionMode.Polite })
                }
                Pill(words.google, onClick = if (finishing != null) null else onGoogle)
                Pill(words.apple, onClick = if (finishing != null) null else onApple)
                PhoneText(words.footnote, 13f, color = Ink.detail, lineSpacing = 2f,
                    modifier = Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 2.dp), align = TextAlign.Center)
            }
        }
    }
}

/** What the phone-first page says (its text already names the account's e-mail); [other]: "Use another account". */
class MacFirstWords(val title: String, val text: String, val site: String, val pair: String, val signedInAs: String, val other: String)

/**
 * Signed in with no Mac paired yet (the phone came first, kb PHN-841): FERB runs on the Mac. The download address, the
 * way on to pairing, who is signed in (a tap opens the account) and another account (kb PHN-842: the phone may be
 * signed in to another account than the Mac's). [signingOut]: the pending state's words while signing out; the page's
 * taps wait meanwhile.
 */
@Composable
fun MacFirstPage(
    words: MacFirstWords,
    insets: PhoneInsets = PhoneInsets.Lab,
    signingOut: String? = null,
    onPair: (() -> Unit)? = {},
    onAccount: (() -> Unit)? = {},
    onOther: (() -> Unit)? = {},
) {
    val waiting = signingOut != null
    Column(Modifier.fillMaxSize()) {
        Spacer(Modifier.weight(1f))
        LinkPicture(LinkKind.SEARCH)
        Spacer(Modifier.weight(1f))
        Card(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), padding = 20.dp) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                PhoneText(words.title, 24f, FerbType.Semibold, modifier = Modifier.semantics { heading() })
                PhoneText(words.text, 15.5f, color = Ink.detail, lineSpacing = 2f)
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    FerbIcon(FerbIcons.laptop, 18f, Ink.black)
                    PhoneText(words.site, 16.5f, FerbType.Semibold)
                }
                Pill(words.pair, onClick = if (waiting) null else onPair)
                AccountLines(words.signedInAs, words.other, signingOut, if (waiting) null else onAccount, if (waiting) null else onOther)
            }
        }
    }
}

/**
 * Who is signed in and, under it, "Use another account" (kb PHN-842), centred; while signing out the second line is
 * its pending state (FERB's running mark and [signingOut]), said to TalkBack as it changes.
 */
@Composable
fun AccountLines(signedInAs: String?, other: String, signingOut: String?, onAccount: (() -> Unit)?, onOther: (() -> Unit)?) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        if (signedInAs != null) {
            PhoneText(signedInAs, 14f, FerbType.Medium, Ink.detail,
                Modifier.pressable(onAccount, label = signedInAs).padding(horizontal = 12.dp, vertical = 4.dp), align = TextAlign.Center)
        }
        Box(Modifier.semantics { liveRegion = LiveRegionMode.Polite }, contentAlignment = Alignment.Center) {
            if (signingOut != null) {
                Row(Modifier.padding(horizontal = 12.dp, vertical = 6.dp).semantics(mergeDescendants = true) {},
                    horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Mark(MarkKind.RUNNING, 16.dp)
                    PhoneText(signingOut, 15f, FerbType.Semibold, Ink.detail)
                }
            } else {
                PhoneText(other, 15f, FerbType.Semibold, Ink.black,
                    Modifier.pressable(onOther, label = other).padding(horizontal = 12.dp, vertical = 6.dp), align = TextAlign.Center)
            }
        }
    }
}
