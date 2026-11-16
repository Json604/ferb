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
    exact: @Composable () -> Unit,
) {
    Sheet(title = title, bottomInset = insets.bottom, onDismiss = onDismiss) {
        Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            SheetIcon(icon)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                PhoneText(title, 22f, FerbType.Semibold)
                PhoneText(reason, 15.5f, color = Ink.detail)
            }
            if (count != null) PhoneText(count, 14f, FerbType.Medium, Ink.faint, maxLines = 1)
        }
        Box(Modifier.padding(top = 14.dp)) { exact() }
        if (note != null) FingerprintNote(note, Modifier.padding(top = 12.dp))
        Row(Modifier.fillMaxWidth().padding(top = 18.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            // The lab gives Yes the layout priority: No takes what its word needs, Yes the rest.
            Pill(no, kind = PillKind.GREY, fill = false, onClick = onNo)
            Pill(yes, Modifier.weight(1f), PillKind.BLACK, onClick = onYes)
        }
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
) {
    Sheet(title = "This phone", bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle("This phone", onClose)
        Column(Modifier.padding(top = 8.dp).outset(14.dp)) {
            CardRow("Fingerprint to open", "Every time FERB opens", leading = { RowIcon(FerbIcons.lock) }, trailing = { PhoneSwitch(lockOn) })
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
