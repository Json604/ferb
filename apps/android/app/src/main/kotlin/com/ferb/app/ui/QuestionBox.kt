package com.ferb.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicSecureTextField
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.TextObfuscationMode
import androidx.compose.foundation.text.input.clearText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.model.Question
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Phone
import com.ferb.design.components.CardInk
import com.ferb.design.components.Mark
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.SecretCodeBoxes
import com.ferb.design.components.SecretKeypad
import com.ferb.design.components.pressable
import com.ferb.mobile.AnswerInput

/**
 * One of FERB's questions as a box in the chat, as FERB.app draws it (kb PHN-770; ApprovalView.swift, placed by
 * ConversationTranscript.swift:205-208 at the end of the chat and TurnCardView.swift:298-301 in its row): its header,
 * the question, why, the exact thing, the task's reason, the evidence, the buttons with the Mac's words, the note, and
 * the typed answer's box. It stays until it is answered anywhere or withdrawn (the transcript closes it); back never
 * dismisses it. The buttons act in place: a Yes that needs the fingerprint asks for it when pressed (the system's
 * prompt over the chat), No needs nothing. A hidden input is entered here: a code on the lab's own keypad (the system
 * keyboard never sees it), anything else in a password field; the screen stays out of screenshots while its field
 * shows, and the value is cleared once sent (PHN-110, PHN-502).
 *
 * [title]/[reason] are the transcript's words, shown alone while the live question isn't known (answered elsewhere, or
 * the status not in yet): FERB.app then draws the words only (TurnCardView.swift:302-305).
 */
@Composable
fun QuestionBox(vm: FerbViewModel, approvalId: String, title: String?, reason: String?, modifier: Modifier = Modifier) {
    // read from the chat's locals: only the boxes recompose when a question or a sending changes (kb PHN-714)
    val q = LocalQuestions.current.firstOrNull { it.approvalId == approvalId }
    val busy = "answer:$approvalId" in LocalBusy.current
    val ink = CardInk.ferb
    Column(
        modifier.fillMaxWidth().padding(end = 28.dp).background(ink.card, RoundedCornerShape(Phone.cardCorner))
            .padding(horizontal = 16.dp, vertical = 14.dp).testTag("question:$approvalId"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val live = q
        if (live == null || live.held) {
            Words(ink, header = reason, question = title ?: stringResource(R.string.question_needs_answer), detail = null)
            return@Column
        }
        val m = live.shown
        // FERB's review of what an import brought in (kb MEM-460): its list stands where the detail line would
        val review = m?.review == true
        Words(
            ink,
            header = m?.header?.ifEmpty { null } ?: stringResource(if (live.hidden) R.string.question_private_title else R.string.question_needs_answer),
            question = m?.question ?: live.title ?: live.reason,
            detail = if (review) null
                else if (m != null) listOfNotNull(m.pictureProblem, m.detail).joinToString("\n\n").ifEmpty { null } else live.reason.takeIf { live.title != null },
            fromTask = m?.fromTask == true,
        )
        if (review) ReviewIn(vm, ink)
        (m?.exact ?: live.exact)?.let { ExactBox(it, ink) }
        m?.why?.let { PhoneText(it, 13.5f, color = ink.secondary) }
        if (m != null && m.evidence.isNotEmpty()) Evidence(m.evidence, ink)
        when {
            live.onlyOnMac -> PhoneText(listOfNotNull(live.needs?.problem, stringResource(R.string.question_on_mac)).joinToString("\n\n"), 14.5f, color = ink.secondary)
            live.hidden -> Hidden(vm, live, busy, ink)
            else -> Answers(vm, live, busy, ink)
        }
        if (busy) PhoneText("Sending your answer…", 13f, color = ink.secondary)
    }
}

/** The review's list in its question's box: read from the Mac when the box first shows, answered in place. */
@Composable
private fun ReviewIn(vm: FerbViewModel, ink: CardInk) {
    val review by vm.review.collectAsState()
    LaunchedEffect(Unit) { vm.loadReview() }
    review?.let { r ->
        MemoryReviewList(r, LocalBusy.current, ink) { part, ids, answer, key -> vm.answerReview(part, ids, answer, key) }
    }
}

@Composable
private fun Words(ink: CardInk, header: String?, question: String, detail: String?, fromTask: Boolean = false) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mark(MarkKind.ASKING)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                header?.let { PhoneText(it, 13f, FerbType.Medium, ink.secondary) }
                // D33, GATE-225: a task's question, tagged by structure
                if (fromTask) {
                    PhoneText("from the task", 11.5f, FerbType.Medium, ink.secondary,
                        Modifier.background(ink.fill, RoundedCornerShape(50)).padding(horizontal = 7.dp, vertical = 1.dp))
                }
                PhoneText(question, 16.5f, FerbType.Semibold, ink.primary, Modifier.semantics { contentDescription = question })
                detail?.let { PhoneText(it, 14.5f, color = ink.secondary) }
            }
        }
    }
}

/** GATE-513: the exact thing approved, word for word, in a fixed-width face, in its own box. */
@Composable
private fun ExactBox(text: String, ink: CardInk) {
    androidx.compose.foundation.text.BasicText(
        text,
        Modifier.fillMaxWidth().background(ink.fill, RoundedCornerShape(12.dp)).padding(12.dp).semantics { contentDescription = "Exactly: $text" },
        style = FerbType.style(14.5f, color = ink.primary).copy(fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace),
    )
}

@Composable
private fun Evidence(lines: List<String>, ink: CardInk) {
    var open by remember(lines) { mutableStateOf(false) }
    PhoneText((if (open) "▾ " else "▸ ") + "App-use evidence", 13.5f, FerbType.Medium, ink.secondary, Modifier.pressable({ open = !open }))
    if (open) for (line in lines) PhoneText(line, 13.5f, color = ink.secondary, modifier = Modifier.padding(start = 12.dp))
}

/** The answers in place: the Mac's buttons, its "until I'm back", its note, and the typed answer's box. */
@Composable
private fun Answers(vm: FerbViewModel, q: Question, busy: Boolean, ink: CardInk) {
    val m = q.shown
    val choices = q.choices
    val act = { input: AnswerInput -> if (busy) null else ({ vm.answer(q.approvalId, input); Unit }) }
    if (q.yesNo) {
        val (no, yes) = choices[0] to choices[1]
        // the phone's own line, said only where the Mac's status says the Yes needs the fingerprint-only key
        if (q.needsProof && q.needs?.fingerprint == true) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                FerbIcon(FerbIcons.fingerprint, 14f, ink.secondary)
                PhoneText(stringResource(R.string.question_yes_fingerprint), 13.5f, color = ink.secondary)
            }
        }
        Row(Modifier.fillMaxWidth().padding(top = 2.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Pill(no.label, kind = PillKind.GREY, fill = false, height = 44.dp, onClick = act(no.input))
            Pill(yes.label, Modifier.weight(1f), PillKind.BLACK, height = 44.dp, onClick = act(yes.input))
        }
        for (c in choices.drop(2)) Pill(c.label, kind = PillKind.GREY, height = 44.dp, onClick = act(c.input))
        if (q.untilBack) {
            Pill(stringResource(R.string.until_back_yes), kind = PillKind.GREY, height = 44.dp, onClick = act(AnswerInput.UntilBack))
            PhoneText(stringResource(R.string.until_back_note), 13.5f, color = ink.secondary)
        }
    } else {
        choices.forEachIndexed { i, c -> Pill(c.label, kind = if (i == 0) PillKind.BLACK else PillKind.GREY, height = 44.dp, onClick = act(c.input)) }
    }
    m?.note?.let { PhoneText(it, 13f, color = ink.secondary) }
    // FERB.app's box: his own words (a question without choices, or a task's question beside its choices)
    if (choices.isEmpty() || m?.answerBox == "answer") {
        var words by remember(q.approvalId) { mutableStateOf("") }
        DarkField(words, { words = it }, m?.field ?: stringResource(R.string.question_answer_hint), ink)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (choices.isEmpty()) {
                Pill(m?.decline ?: stringResource(R.string.question_skip), kind = PillKind.GREY, fill = false, height = 44.dp, onClick = act(AnswerInput.Words("")))
            }
            Pill(m?.send ?: stringResource(R.string.question_send), Modifier.weight(1f), height = 44.dp,
                onClick = if (busy || words.isBlank()) null else ({ vm.answer(q.approvalId, AnswerInput.Words(words.trim())); words = "" }))
        }
    }
}

@Composable
private fun DarkField(value: String, onChange: (String) -> Unit, hint: String, ink: CardInk) {
    Box(Modifier.fillMaxWidth().heightIn(min = 48.dp).background(ink.fill, RoundedCornerShape(14.dp)).padding(horizontal = 14.dp, vertical = 12.dp)) {
        BasicTextField(value, onChange, Modifier.fillMaxWidth().semantics { contentDescription = hint },
            textStyle = FerbType.style(16f, color = ink.primary), cursorBrush = SolidColor(ink.primary), maxLines = 4)
        if (value.isEmpty()) PhoneText(hint, 16f, color = ink.faint)
    }
}

/**
 * A hidden input in the box (PHN-109, PHN-110, PHN-502): the value goes straight to the Mac on the sealed path and is
 * kept nowhere (plain remember, never saved state; cleared when sent and when the box goes); the screen stays out of
 * screenshots while the field shows ([FerbViewModel.hiddenShown]). A code takes the lab's own keypad, so the system
 * keyboard never sees it; a password or other secret the system keyboard in its password mode.
 */
@Composable
private fun Hidden(vm: FerbViewModel, q: Question, busy: Boolean, ink: CardInk) {
    val m = q.shown
    DisposableEffect(q.approvalId) {
        vm.hiddenShown(q.approvalId, true)
        onDispose { vm.hiddenShown(q.approvalId, false) }
    }
    val decline = m?.decline ?: stringResource(R.string.private_decline)
    val sendWord = m?.send ?: stringResource(R.string.private_send)
    if (q.isCode) {
        val digits = remember(q.approvalId) { CharArray(MAX_CODE) }
        var count by remember(q.approvalId) { mutableIntStateOf(0) }
        DisposableEffect(q.approvalId) { onDispose { digits.fill('\u0000'); count = 0 } }
        SecretCodeBoxes(count, length = maxOf(6, minOf(MAX_CODE, count + 1)))
        SecretKeypad(Modifier.background(ink.fill, RoundedCornerShape(14.dp)), bottomInset = 0.dp, onKey = { k ->
            if (k == "⌫") { if (count > 0) { count--; digits[count] = '\u0000' } } else if (count < MAX_CODE) { digits[count] = k[0]; count++ }
        })
        PrivateLine(ink)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Pill(decline, kind = PillKind.GREY, fill = false, height = 44.dp, onClick = if (busy) null else ({ vm.answer(q.approvalId, AnswerInput.Words("")) }))
            Pill(sendWord, Modifier.weight(1f), height = 44.dp, onClick = if (busy || count == 0) null else ({
                // the digits go into the library's SecretInput one by one and this array is zeroed at once (kb PHN-536)
                val n = count
                count = 0
                vm.answerHidden(q.approvalId, digits, n)
                Unit
            }))
        }
        return
    }
    val field = remember(q.approvalId) { TextFieldState() }
    DisposableEffect(q.approvalId) { onDispose { field.clearText() } }
    Box(Modifier.fillMaxWidth().heightIn(min = 48.dp).background(ink.fill, RoundedCornerShape(14.dp)).padding(horizontal = 14.dp, vertical = 12.dp)) {
        BasicSecureTextField(
            field, Modifier.fillMaxWidth().semantics { contentDescription = "Private answer" },
            textStyle = FerbType.style(17f, color = ink.primary), textObfuscationMode = TextObfuscationMode.Hidden,
            keyboardOptions = KeyboardOptions(KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
            cursorBrush = SolidColor(ink.primary),
        )
        if (field.text.isEmpty()) PhoneText(m?.field ?: stringResource(R.string.private_field), 16f, color = ink.faint)
    }
    PrivateLine(ink)
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Pill(decline, kind = PillKind.GREY, fill = false, height = 44.dp,
            onClick = if (busy) null else ({ field.clearText(); vm.answer(q.approvalId, AnswerInput.Words("")) }))
        Pill(sendWord, Modifier.weight(1f), height = 44.dp, onClick = if (busy || field.text.isEmpty()) null else ({
            // read character by character (no String of it here), the field cleared, the copy zeroed by the view model
            // once its units are in the library's SecretInput (kb PHN-536)
            val t = field.text
            val chars = CharArray(t.length) { t[it] }
            field.clearText()
            vm.answerHidden(q.approvalId, chars, chars.size)
            Unit
        }))
    }
}

@Composable
private fun PrivateLine(ink: CardInk) {
    Row(Modifier.semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        FerbIcon(FerbIcons.lockFill, 13f, ink.secondary)
        PhoneText("Private: it goes straight to your Mac and is not kept on this phone.", 13.5f, color = ink.secondary)
    }
}

private const val MAX_CODE = 10

/** The open questions as the view model merged them (their needs from the Mac's status), for the boxes in the chat. */
val LocalQuestions = androidx.compose.runtime.compositionLocalOf<List<Question>> { emptyList() }

/** What is on its way (`answer:<id>` while a box's answer is sent). */
val LocalBusy = androidx.compose.runtime.compositionLocalOf<Set<String>> { emptySet() }
