package com.ferb.app.model

import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.linkCode
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.FerbException
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.QuestionItem
import com.ferb.mobile.QuestionNeeds
import com.ferb.mobile.SecretInput
import com.ferb.mobile.Signer
import com.ferb.mobile.StatusQuestion

/**
 * One of FERB's questions as the phone shows it, read from the question entry's body (the gateway's own fields:
 * GATE-566's title and summary, the exact thing, the options and their button words). Nothing here judges what a
 * question is about: which answers need the fingerprint is the Mac's to say (PHN-470), and the phone follows its
 * refusal (PHN-13: refused as locked, it unlocks and asks once more).
 */
data class Question(
    val approvalId: String,
    val taskId: String?,
    /** GATE-566's summary or title, else null (the app's own words). */
    val title: String?,
    val reason: String,
    val exact: String?,
    val kind: String,
    val capability: String?,
    val options: List<String>,
    val labels: List<String>,
    val hidden: Boolean,
    /** What answering needs, as the Mac's status says (null: no status yet). */
    val needs: QuestionNeeds? = null,
    /** The words were held back (a locked phone, PHN-126): only that a question waits. */
    val held: Boolean = false,
) {
    /** PHN-417, PHN-470: answered from the notification's buttons with no prompt. Unknown needs are not routine. */
    val routine: Boolean get() = needs?.let { it.canAnswer && !it.needsUnlock && !it.hidden } == true && !hidden

    /** Only on the Mac (a hidden input on a phone that can't sign it, …): its words say why. */
    val onlyOnMac: Boolean get() = needs?.canAnswer == false

    /** The Yes needs the phone's signed proof: the fingerprint-only key when [QuestionNeeds.fingerprint]. */
    val needsProof: Boolean get() = needs?.let { it.needsUnlock && it.canAnswer } == true

    /** A hidden input's kind is a code (the keypad) rather than a password or a secret (the system keyboard). */
    val isCode: Boolean get() = kind == "owner_login_code"

    /** One of FERB's own checks (the gateway's OWN_CHECKS by capability): the class whose Yes may need the fingerprint. */
    val ownCheck: Boolean get() = capability in OWN_CHECKS

    /** The answers as buttons, in the order the Mac draws them; empty: the owner types the answer. */
    val choices: List<Choice>
        get() {
            if (hidden || options.isEmpty()) return emptyList()
            if (options == listOf("Yes", "No")) {
                // the gateway's yes/no: No is a plain No (it needs nothing, PHN-470); Yes is option 1, as FERB.app sends it
                return listOf(Choice(label(1), AnswerInput.Approve(false, null), yes = false), Choice(label(0), AnswerInput.Approve(true, 1L), yes = true))
            }
            return options.indices.map { Choice(label(it), AnswerInput.Approve(true, (it + 1).toLong()), yes = it == 0) }
        }

    private fun label(i: Int): String = labels.getOrNull(i)?.trim()?.takeIf { it.isNotEmpty() } ?: options[i]

    data class Choice(val label: String, val input: AnswerInput, val yes: Boolean)

    companion object {
        /** ferb-link role.rs OWN_CHECKS (the gateway's): FERB's own consequential checks. */
        val OWN_CHECKS = setOf("ferb.confirm", "ferb.retry", "conversation.approve", "memory.forget", "downloads.delete", "browsing.choose")

        /** ferb-link role.rs SECRET_KINDS: answered only through the sealed path (PHN-109, PHN-110). */
        val SECRET_KINDS = setOf("owner_login_code", "owner_admin_password", "owner_secret")
    }
}

object Questions {
    fun parse(q: QuestionItem): Question {
        val body = runCatching { Json.parse(q.bodyJson) }.getOrNull() as? Map<String, Any?> ?: emptyMap()
        val kind = body.str("kind") ?: "owner_question"
        return Question(
            approvalId = q.approvalId.orEmpty(),
            taskId = q.taskId,
            title = body.str("summary") ?: body.str("title"),
            reason = body.str("reason").orEmpty(),
            exact = body.str("exact"),
            kind = kind,
            capability = body.str("capability"),
            options = body.strings("options"),
            labels = body.strings("labels"),
            hidden = kind in Question.SECRET_KINDS || body.bool("secret"),
        )
    }

    /** The open questions of a transcript, oldest first. */
    fun open(items: List<QuestionItem>): List<Question> =
        items.filter { it.open && it.approvalId != null }.map(::parse)

    /** A question as the Mac's status gives it (the app is locked, or the chat hasn't brought it yet). */
    fun fromStatus(s: StatusQuestion): Question = Question(
        approvalId = s.approvalId, taskId = s.taskId, title = null, reason = if (s.held) "" else s.reason, exact = null,
        kind = if (s.needs.hidden) "owner_secret" else "owner_question", capability = s.capability,
        options = if (s.held) emptyList() else s.options, labels = emptyList(), hidden = s.needs.hidden, needs = s.needs, held = s.held,
    )

    /**
     * The transcript's open questions, each with its needs from [status], then the status's questions the transcript
     * doesn't hold yet (each question once).
     */
    fun merged(transcript: List<Question>, status: PhoneStatus?): List<Question> {
        val byId = status?.questions.orEmpty().associateBy { it.approvalId }
        val known = transcript.map { q -> byId[q.approvalId]?.let { q.copy(needs = it.needs) } ?: q }
        val ids = known.map { it.approvalId }.toSet()
        return known + status?.questions.orEmpty().filter { it.approvalId !in ids }.map(::fromStatus)
    }
}

/** How an answer went. */
sealed interface Answered {
    data object Sent : Answered
    /** The owner cancelled the prompt: nothing was sent. */
    data object NotSent : Answered
    /** The Mac or the link said no: [error]'s words and code. */
    data class Failed(val error: FerbException) : Answered
    /** The Mac wants the fingerprint key, which Android destroyed when the fingerprints changed: pair again. */
    data object NeedsPairingAgain : Answered
}

/**
 * Sends one answer the way PHN-13 and PHN-470 have it: first as it is (a routine Yes, an ordinary answer and a plain
 * No need no proof from a session that proved the phone); if the Mac says this answer needs the phone's own proof
 * (`unlock_required`), it is signed once, by the fingerprint key where the phone has one (PHN-137), else the lock
 * key; if the Mac's unlock window had closed (`locked`), the phone unlocks toward the Mac and asks once more. A hidden
 * value always goes signed (the sealed path, either key; [answerHidden]). Never queued (PHN-448).
 */
class AnswerFlow(
    private val api: PhoneApi,
    private val keys: () -> KeyHealth?,
    private val signer: (KeyKind) -> Signer,
) {
    suspend fun answer(approvalId: String, input: AnswerInput, upfront: KeyKind? = null): Answered {
        var unlockedOnce = false
        var signedOnce = false
        // the Mac's status said this answer needs the phone's proof: asked for at once, not after a refusal
        var proof: Signer? = null
        if (upfront != null) {
            if (upfront == KeyKind.STRONG && keys()?.strong != KeyState.OK) return Answered.NeedsPairingAgain
            proof = signer(upfront)
            signedOnce = true
        }
        while (true) {
            val r = api.answer(approvalId, input, proof)
            val err = (r as? Outcome.Failed)?.error ?: return Answered.Sent
            if (err !is FerbException.NeedsUnlock) return done(r)
            when (err.linkCode) {
                "unlock_required", "unlock_invalid", "fresh_unlock_required" -> {
                    if (signedOnce) return Answered.Failed(err)
                    signedOnce = true
                    val k = keys()
                    proof = when {
                        k?.strong == KeyState.OK -> signer(KeyKind.STRONG)
                        k?.strong == KeyState.GONE -> return Answered.NeedsPairingAgain
                        k?.lock == KeyState.OK -> signer(KeyKind.LOCK)
                        else -> return Answered.NeedsPairingAgain
                    }
                }
                else -> {
                    if (unlockedOnce) return Answered.Failed(err)
                    unlockedOnce = true
                    when (val u = api.unlock(signer(KeyKind.LOCK))) {
                        is Outcome.Ok -> Unit
                        is Outcome.Failed -> return if (u.error is FerbException.NotSigned) Answered.NotSent else Answered.Failed(u.error)
                    }
                }
            }
        }
    }

    /**
     * A hidden value always goes signed, on the sealed path (PHN-110), from the library's [SecretInput] (never a
     * String): the library empties it whatever happens; the caller destroys it.
     */
    suspend fun answerHidden(approvalId: String, secret: SecretInput): Answered =
        done(api.answerHidden(approvalId, secret, signer(KeyKind.LOCK)))

    private fun done(r: Outcome<String>): Answered = when (r) {
        is Outcome.Ok -> Answered.Sent
        is Outcome.Failed -> if (r.error is FerbException.NotSigned) Answered.NotSent else Answered.Failed(r.error)
    }
}
