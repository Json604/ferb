package com.ferb.design.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.clickable
import androidx.compose.ui.semantics.Role
import com.ferb.design.sheetPanelMotion
import com.ferb.design.sheetScrimMotion
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.LabColors
import com.ferb.design.Phone

// What slides up (Mobile.swift Sheet; MobileScreens.swift's sheets): a white sheet inset from the screen's edges over
// a dimmed page.

/**
 * Mobile.swift Sheet. [bottomInset] is the system's gesture area (the lab's os.bottom); [lift] raises the sheet over
 * something below it (the keypad), which also gives it 32 corners instead of 44.
 */
@Composable
fun Sheet(
    modifier: Modifier = Modifier,
    title: String = "",
    bottomInset: Dp = Phone.gestureArea,
    lift: Dp = 0.dp,
    onDismiss: (() -> Unit)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    // Shown by a sheet switch (MotionHosts.kt), the dim fades and the panel comes up from below and goes back down; a
    // dismissible one follows a drag down. Going, the dim lets touches through to the page and the panel takes none.
    val presence = com.ferb.design.LocalMotionPresence.current
    val leaving = presence?.leaving == true
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        // the dimmed page: a tap there closes a sheet that can be closed (the app passes [onDismiss])
        Box(
            Modifier.fillMaxSize()
                .then(if (presence != null) Modifier.sheetScrimMotion(presence) else Modifier)
                .background(Color.Black.copy(alpha = com.ferb.design.Motion.SCRIM_ALPHA))
                .then(
                    if (onDismiss == null || leaving) Modifier
                    else Modifier.clickable(interactionSource = null, indication = null, onClickLabel = "Close", role = Role.Button, onClick = onDismiss),
                ),
        )
        Column(
            Modifier.then(if (presence != null) Modifier.sheetPanelMotion(presence, onDismiss) else Modifier)
                .padding(start = Phone.sheetInset, end = Phone.sheetInset, bottom = Phone.sheetInset + lift)
                .fillMaxWidth()
                .background(Ink.white, RoundedCornerShape(if (lift > 0.dp) Phone.sheetCornerLifted else Phone.sheetCorner))
                .semantics { paneTitle = title }
                .padding(start = Phone.sheetPadding, end = Phone.sheetPadding, bottom = if (lift > 0.dp) 22.dp else bottomInset + 4.dp),
        ) {
            Box(Modifier.fillMaxWidth().padding(top = 10.dp, bottom = 16.dp), contentAlignment = Alignment.Center) {
                Box(Modifier.size(38.dp, 5.dp).background(LabColors.grabber, CircleShape))
            }
            content()
        }
    }
}

/** The line under a question that says what Yes will ask for (MobileScreens.swift QuestionSheet). */
@Composable
fun FingerprintNote(text: String = "Yes asks for your fingerprint.", modifier: Modifier = Modifier) {
    Row(modifier.semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
        FerbIcon(FerbIcons.fingerprint, 14f, Ink.detail)
        PhoneText(text, 14f, color = Ink.detail)
    }
}

/** The exact thing a question is about, on black: a command in a fixed-width face (QuestionSheet). */
@Composable
fun ExactCommand(command: String, modifier: Modifier = Modifier) {
    androidx.compose.foundation.text.BasicText(
        command,
        modifier.fillMaxWidth().background(Ink.black, RoundedCornerShape(16.dp)).padding(15.dp)
            .semantics { contentDescription = "Command: $command" },
        style = FerbType.style(15f, color = Ink.white).copy(fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace, letterSpacing = 0.sp),
    )
}

/** The exact thing a question is about, on black: an item and its price (Conversation.swift CartSheet). */
@Composable
fun ExactItem(title: String, detail: String, value: String, modifier: Modifier = Modifier) {
    Row(
        modifier.fillMaxWidth().background(Ink.black, RoundedCornerShape(16.dp)).semantics(mergeDescendants = true) {}.padding(15.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PhoneText(title, 16f, FerbType.Semibold, Ink.white)
            PhoneText(detail, 14f, color = Ink.onDark)
        }
        PhoneText(value, 20f, FerbType.Semibold, Ink.white)
    }
}

/**
 * MobileScreens.swift SecretSheet: six boxes, [filled] of them holding a dot and the next one ringed with the caret.
 * The digits themselves are never drawn.
 */
@Composable
fun SecretCodeBoxes(filled: Int, modifier: Modifier = Modifier, length: Int = 6) {
    Row(
        modifier.fillMaxWidth().semantics { contentDescription = "$filled of $length digits entered" },
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (i in 0 until length) {
            val shape = RoundedCornerShape(Phone.secretBoxCorner)
            val current = i == filled
            Box(
                Modifier.weight(1f).height(Phone.secretBoxHeight)
                    .background(if (current) Ink.white else Ink.fill, shape)
                    .then(if (current) Modifier.border(2.5.dp, Color.Black, shape) else Modifier),
                contentAlignment = Alignment.Center,
            ) {
                if (i < filled) Box(Modifier.size(11.dp).background(Color.Black, CircleShape))
                if (current) Box(Modifier.size(2.dp, 22.dp).background(Color.Black))
            }
        }
    }
}

/**
 * MobileScreens.swift SecretSheet's keypad: digits with their letters on white keys over a grey ground. The app
 * decides whether a private answer uses this keypad or the system's (S11).
 */
@Composable
fun SecretKeypad(modifier: Modifier = Modifier, bottomInset: Dp = Phone.gestureArea, onKey: ((String) -> Unit)? = null) {
    val keys = listOf("1" to "", "2" to "ABC", "3" to "DEF", "4" to "GHI", "5" to "JKL", "6" to "MNO",
        "7" to "PQRS", "8" to "TUV", "9" to "WXYZ", "" to "", "0" to "", "⌫" to "")
    // under a sheet switch it comes up and goes down with its sheet (MotionHosts.kt)
    val presence = com.ferb.design.LocalMotionPresence.current
    Column(
        modifier.then(if (presence != null) Modifier.sheetPanelMotion(presence, null) else Modifier)
            .fillMaxWidth().background(LabColors.keypad).padding(start = 6.dp, end = 6.dp, top = 8.dp, bottom = bottomInset + 10.dp),
        verticalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        for (row in keys.chunked(3)) {
            Row(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                for ((digit, letters) in row) {
                    val real = digit.isNotEmpty() && digit != "⌫"
                    Column(
                        Modifier.weight(1f).heightIn(min = Phone.keyHeight)
                            .then(if (real) Modifier.background(Color.White, RoundedCornerShape(Phone.keyCorner)) else Modifier)
                            .pressable(if (digit.isEmpty()) null else onKey?.let { { it(digit) } }, label = if (digit == "⌫") "Delete" else digit)
                            .semantics { if (digit == "⌫") contentDescription = "Delete" },
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.Center,
                    ) {
                        if (digit == "⌫") FerbIcon(FerbIcons.backspace, 20f, Ink.black)
                        else if (digit.isNotEmpty()) {
                            PhoneText(digit, 25f, color = Ink.black)
                            if (letters.isNotEmpty()) {
                                androidx.compose.foundation.text.BasicText(
                                    letters, Modifier.padding(top = 0.dp),
                                    style = FerbType.style(9f, FontWeight.Bold, Ink.detail).copy(letterSpacing = 1.6.sp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

/** A sheet's title row: a title, and the close button when it can be dismissed (RelaySheet, ProfileSheet). */
@Composable
fun SheetTitle(title: String, onClose: (() -> Unit)? = null) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
        PhoneText(title, 22f, FerbType.Semibold, Ink.black, Modifier.weight(1f).semantics { heading() })
        if (onClose != null) CloseButton(onClose)
    }
}
