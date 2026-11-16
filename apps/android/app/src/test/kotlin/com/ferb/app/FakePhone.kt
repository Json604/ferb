package com.ferb.app

import com.ferb.app.model.PhoneApi
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Cancelled
import com.ferb.mobile.Change
import com.ferb.mobile.FerbException
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.OlderState
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.Signer
import com.ferb.mobile.TranscriptView
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow

/** A phone for the view models' tests: flows the test sets, calls it records, answers it scripts. No library. */
class FakePhone : PhoneApi {
    override val state = MutableStateFlow<PhoneState>(PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK)))
    override val link = MutableStateFlow<LinkStatus?>(status())
    override val changes = MutableSharedFlow<Change>(extraBufferCapacity = 16)

    var view: TranscriptView? = null
    val outbox = ArrayList<OutboxItem>()
    val calls = ArrayList<String>()
    val answers = ArrayList<Pair<AnswerInput, Signer?>>()

    /** What each answer call says, in turn (then Ok). */
    val answerScript = ArrayDeque<Outcome<String>>()
    var unlockResult: Outcome<Unit> = Outcome.Ok(Unit)
    var stopResult: Outcome<Unit> = Outcome.Ok(Unit)
    var sendWaitingFor: String? = null

    override suspend fun transcript(): Outcome<TranscriptView?> = Outcome.Ok(view?.copy(outbox = outbox.toList()))
    override suspend fun sync(): Outcome<Unit> { calls += "sync"; return Outcome.Ok(Unit) }
    override suspend fun send(text: String): Outcome<OutboxItem> {
        calls += "send:$text"
        val item = OutboxItem("o${outbox.size + 1}", text, "2026-01-02T10:00:00+00:00", "waiting", sendWaitingFor ?: "link", null, false)
        outbox += item
        return Outcome.Ok(item)
    }
    override suspend fun cancel(outboxId: String): Outcome<Cancelled> { calls += "cancel:$outboxId"; outbox.removeAll { it.id == outboxId }; return Outcome.Ok(Cancelled(false)) }
    override suspend fun answer(approvalId: String, input: AnswerInput, signer: Signer?): Outcome<String> {
        calls += "answer:$approvalId:${signer != null}"
        answers += input to signer
        signer?.sign("payload")
        return answerScript.removeFirstOrNull() ?: Outcome.Ok("{}")
    }
    override suspend fun stop(taskIds: List<String>): Outcome<Unit> { calls += "stop:${taskIds.joinToString()}"; return stopResult }
    override suspend fun loadOlder(conversation: String): Outcome<OlderState> { calls += "older"; return Outcome.Ok(OlderState.Complete) }
    override suspend fun unlock(signer: Signer): Outcome<Unit> {
        calls += "unlock"
        val r = unlockResult
        if (r is Outcome.Ok && signer.sign("challenge") == null) return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        return r
    }
    override suspend fun pair(code: String): Outcome<Unit> { calls += "pair:$code"; return Outcome.Ok(Unit) }
    override suspend fun forget(): Outcome<Unit> { calls += "forget"; return Outcome.Ok(Unit) }

    companion object {
        fun status(phase: LinkPhase = LinkPhase.ONLINE, paired: Boolean = true, path: LinkPath = LinkPath.DIRECT, code: String? = null, problem: com.ferb.mobile.LinkProblem? = null) =
            LinkStatus(phase, path, null, code, paired, path != LinkPath.PUBLIC_RELAY, false, null, false, null, problem, null, false, false)

        fun refused(linkCode: String) = Outcome.Failed(FerbException.NeedsUnlock("The Mac wants the phone's proof.", linkCode))
    }
}

/** A signer standing in for the prompt: it says which key it was asked for and signs, or the owner cancels. */
class FakeSigner(val kind: com.ferb.core.KeyKind, private val cancel: Boolean = false) : Signer {
    var asked = 0
    override fun sign(payload: String): String? { asked++; return if (cancel) null else "sig-$kind" }
}
