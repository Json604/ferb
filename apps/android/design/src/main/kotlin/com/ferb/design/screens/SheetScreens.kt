package com.ferb.design.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.ExactCommand
import com.ferb.design.components.FingerprintNote
import com.ferb.design.components.Mark
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneSwitch
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.RowIcon
import com.ferb.design.components.SecretCodeBoxes
import com.ferb.design.components.SecretKeypad
import com.ferb.design.components.Sheet
import com.ferb.design.components.SheetIcon
import com.ferb.design.components.SheetTitle

// The sheets (MobileScreens.swift "Sheets", Conversation.swift CartSheet).

/**
 * MobileScreens.swift QuestionSheet: a question slides up over whatever is open and shows the exact thing; Yes asks
 * for the fingerprint. [exact] is the thing itself (a command, an item and its price).
 */
@Composable
fun QuestionSheet(
    icon: ImageVector,
    title: String,
    reason: String,
    yes: String,
    insets: PhoneInsets = PhoneInsets.Lab,
    count: String? = null,
    no: String = "No",
    onNo: (() -> Unit)? = {},
    onYes: (() -> Unit)? = {},
    note: String? = "Yes asks for your fingerprint.",
    onDismiss: (() -> Unit)? = null,
    /** More answers the Mac offers beyond Yes and No (kb PHN-607), each a grey pill under them; none by default. */
    more: List<Pair<String, (() -> Unit)?>> = emptyList(),
    /** One short line under the further answers (what "until I'm back at my Mac" means, PHN-611); none by default. */
    moreNote: String? = null,
    /** FERB.app's header over the question ("Check before FERB goes on", kb PHN-739); none in the lab's. */
    header: String? = null,
    /** What FERB.app shows under the exact thing: the task's reason, the evidence (kb PHN-739). */
    below: @Composable () -> Unit = {},
    /** FERB.app's line under its buttons (what the 30-minute answer covers). */
    answerNote: String? = null,
    exact: @Composable () -> Unit,
) {
    Sheet(title = title, bottomInset = insets.bottom, onDismiss = onDismiss) {
        Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            SheetIcon(icon)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (header != null) PhoneText(header, 13.5f, FerbType.Medium, Ink.detail)
                PhoneText(title, 22f, FerbType.Semibold)
                if (reason.isNotEmpty()) PhoneText(reason, 15.5f, color = Ink.detail)
            }
            if (count != null) PhoneText(count, 14f, FerbType.Medium, Ink.faint, maxLines = 1)
        }
        Box(Modifier.padding(top = 14.dp)) { exact() }
        below()
        if (note != null) FingerprintNote(note, Modifier.padding(top = 12.dp))
        Row(Modifier.fillMaxWidth().padding(top = 18.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            // The lab gives Yes the layout priority: No takes what its word needs, Yes the rest.
            Pill(no, kind = PillKind.GREY, fill = false, onClick = onNo)
            Pill(yes, Modifier.weight(1f), PillKind.BLACK, onClick = onYes)
        }
        for ((label, onClick) in more) Pill(label, Modifier.padding(top = 10.dp), PillKind.GREY, onClick = onClick)
        if (moreNote != null) PhoneText(moreNote, 14f, color = Ink.detail, modifier = Modifier.fillMaxWidth().padding(top = 8.dp, start = 4.dp, end = 4.dp))
        if (answerNote != null) PhoneText(answerNote, 13.5f, color = Ink.detail, modifier = Modifier.fillMaxWidth().padding(top = 10.dp, start = 4.dp, end = 4.dp))
    }
}

/** The lab's question: run a command in Terminal (MobileScreens.swift QuestionSheet's invented example). */
@Composable
fun TerminalQuestion(command: String, reason: String, count: String?, insets: PhoneInsets = PhoneInsets.Lab) {
    QuestionSheet(FerbIcons.terminal, "Run this in Terminal?", reason, "Yes, run it", insets, count) { ExactCommand(command) }
}

/**
 * MobileScreens.swift SecretSheet: a private answer goes straight to the Mac and is not kept on the phone. The sheet
 * sits over the keypad.
 */
@Composable
fun SecretSheet(title: String, detail: String, filled: Int, insets: PhoneInsets = PhoneInsets.Lab) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Sheet(title = title, bottomInset = insets.bottom, lift = (4 * 57 + 18).dp + insets.bottom) {
            Row(horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
                SheetIcon(FerbIcons.key)
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    PhoneText(title, 19f, FerbType.Semibold)
                    PhoneText(detail, 15f, color = Ink.detail)
                }
            }
            SecretCodeBoxes(filled, Modifier.padding(top = 16.dp))
            Row(Modifier.padding(top = 12.dp).semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                FerbIcon(FerbIcons.lockFill, 14f, Ink.detail, Modifier.padding(top = 1.dp))
                PhoneText("Private: it goes straight to your Mac and is not kept on this phone.", 14f, color = Ink.detail)
            }
        }
        SecretKeypad(bottomInset = insets.bottom)
    }
}

/** What the relay sheet says: the explanation, what works there (what, how), and the line about full speed. */
data class RelayCopy(val explanation: String, val works: List<Pair<String, String>>, val fullSpeed: String)

/**
 * MobileScreens.swift RelaySheet: on the shared relay, what works there and how to get everything. The words are the
 * app's (the lab's sample in preview/ follows PHN-410 and PHN-444: text, questions and Stop only).
 */
@Composable
fun RelaySheet(copy: RelayCopy, insets: PhoneInsets = PhoneInsets.Lab, onClose: (() -> Unit)? = {}, trying: Boolean = true) {
    Sheet(title = "Through the shared relay", bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle("Through the shared relay", onClose)
        PhoneText(copy.explanation, 15.5f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 6.dp))
        Column(Modifier.padding(top = 14.dp).fillMaxWidth().background(Ink.fill, RoundedCornerShape(18.dp))) {
            copy.works.forEachIndexed { i, (what, how) ->
                if (i > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(Ink.line))
                Works(what, how)
            }
        }
        if (trying) {
            Row(Modifier.padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                Mark(MarkKind.RUNNING, 20.dp)
                PhoneText("Trying for a direct connection", 15f, color = Ink.detail)
            }
        }
        PhoneText(copy.fullSpeed, 14f, color = Ink.detail, modifier = Modifier.padding(top = 8.dp))
    }
}

@Composable
private fun Works(what: String, how: String) {
    Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) {}.padding(horizontal = 14.dp, vertical = 12.dp)) {
        PhoneText(what, 15f, modifier = Modifier.weight(1f))
        PhoneText(how, 15f, color = Ink.detail)
    }
}

/** MobileScreens.swift ProfileSheet: this phone's own settings, the lock, staying connected, forgetting the Mac. */
@Composable
fun ProfileSheet(
    macName: String,
    pairedSince: String,
    insets: PhoneInsets = PhoneInsets.Lab,
    lockOn: Boolean = true,
    stayConnected: Boolean = true,
    onClose: (() -> Unit)? = {},
    onStayConnected: ((Boolean) -> Unit)? = null,
    onForget: (() -> Unit)? = null,
    more: (@Composable () -> Unit)? = null,
    /** Not from the lab (kb PHN-656): the lock can't be turned off, so it says so ("Always on") instead of a switch that
     *  does nothing, and a tap on the row says why. Null: the lab's switch. */
    lockAlways: String? = null,
    onLockInfo: (() -> Unit)? = null,
    /** PHN-801: the connection's row, first (its state as the top bar's badge shows it; its sheet on a tap). */
    connection: (@Composable () -> Unit)? = null,
) {
    Sheet(title = "This phone", bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle("This phone", onClose)
        Column(Modifier.padding(top = 8.dp).outset(14.dp)) {
            if (connection != null) {
                connection()
                Divide()
            }
            if (lockAlways != null) {
                CardRow("Fingerprint to open", "Every time FERB opens", onClick = onLockInfo, leading = { RowIcon(FerbIcons.lock) },
                    trailing = { PhoneText(lockAlways, 15f, FerbType.Medium, Ink.detail) })
            } else {
                CardRow("Fingerprint to open", "Every time FERB opens", leading = { RowIcon(FerbIcons.lock) }, trailing = { PhoneSwitch(lockOn) })
            }
            Divide()
            CardRow("Stay connected", "Questions arrive while the app is closed", leading = { RowIcon(FerbIcons.bell) },
                trailing = { PhoneSwitch(stayConnected, onStayConnected, "Stay connected") })
            if (more != null) more()
            Divide()
            CardRow("Paired with $macName", "Since $pairedSince", leading = { RowIcon(FerbIcons.laptop) })
            Divide()
            CardRow("Forget this Mac", danger = true, onClick = onForget, leading = { RowIcon(FerbIcons.link, danger = true) })
        }
    }
}

/**
 * The connection's row on the profile page (PHN-801): the link's mark in a black round, the state, the path, and the
 * connection sheet on a tap (Try again, the relay, the Mac tab).
 */
@Composable
fun ConnectionRow(mark: com.ferb.design.components.LinkMark, state: String, path: String?, onClick: (() -> Unit)?) {
    com.ferb.design.components.CardRow(state, path, onClick = onClick, leading = {
        Box(Modifier.size(28.dp).background(Ink.black, androidx.compose.foundation.shape.CircleShape), contentAlignment = Alignment.Center) {
            com.ferb.design.components.LinkMarkView(mark, 9.dp)
        }
    }, trailing = { com.ferb.design.components.Opens() })
}

/**
 * The connection sheet (kb PHN-752): which Mac, how the link stands in plain words with its mark, the path and what it
 * means for him (on the shared relay, the relay sheet's own list of what works there), the link's own words for why it
 * can't connect, Try again while it isn't connected, and the way to the Mac tab for the rest.
 */
@Composable
fun ConnectionSheet(
    mac: String,
    mark: com.ferb.design.components.LinkMark,
    state: String,
    path: String?,
    reason: String?,
    relay: RelayCopy?,
    tryAgain: String?,
    onTryAgain: (() -> Unit)?,
    more: String,
    onMore: () -> Unit,
    insets: PhoneInsets = PhoneInsets.Lab,
    onClose: (() -> Unit)? = {},
) {
    Sheet(title = mac, bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle(mac, onClose)
        Row(Modifier.padding(top = 8.dp).semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(28.dp).background(Ink.black, androidx.compose.foundation.shape.CircleShape), contentAlignment = Alignment.Center) {
                com.ferb.design.components.LinkMarkView(mark, 9.dp)
            }
            PhoneText(state, 17f, FerbType.Semibold)
        }
        if (path != null) PhoneText(path, 15.5f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 10.dp))
        if (reason != null) PhoneText(reason, 15.5f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 10.dp))
        if (relay != null) {
            Column(Modifier.padding(top = 14.dp).fillMaxWidth().background(Ink.fill, RoundedCornerShape(18.dp))) {
                relay.works.forEachIndexed { i, (what, how) ->
                    if (i > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(Ink.line))
                    Works(what, how)
                }
            }
        }
        Column(Modifier.padding(top = 18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (tryAgain != null) Pill(tryAgain, Modifier.fillMaxWidth(), PillKind.BLACK, onClick = onTryAgain)
            Pill(more, Modifier.fillMaxWidth(), PillKind.GREY, onClick = onMore)
        }
    }
}
