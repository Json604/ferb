package com.ferb.app.model

import com.ferb.core.FerbPhone
import com.ferb.core.KeyHealth
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Cancelled
import com.ferb.mobile.Change
import com.ferb.mobile.FerbException
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.OlderState
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.Signer
import com.ferb.mobile.TranscriptView
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * What the app's screens ask of the phone: one seam over [FerbPhone], so the view models are tested with a fake
 * and the real one is the library. Every call runs off the main thread (FerbPhone.call) and comes back typed.
 */
interface PhoneApi {
    val state: StateFlow<PhoneState>
    val link: StateFlow<LinkStatus?>
    val changes: SharedFlow<Change>

    /** The current conversation as the phone holds it, or null before anything was fetched. */
    suspend fun transcript(): Outcome<TranscriptView?>
    suspend fun sync(): Outcome<Unit>
    suspend fun send(text: String): Outcome<OutboxItem>
    suspend fun cancel(outboxId: String): Outcome<Cancelled>
    suspend fun answer(approvalId: String, input: AnswerInput, signer: Signer?): Outcome<String>
    suspend fun stop(taskIds: List<String>): Outcome<Unit>
    suspend fun loadOlder(conversation: String): Outcome<OlderState>
    suspend fun unlock(signer: Signer): Outcome<Unit>
    suspend fun pair(code: String): Outcome<Unit>
    suspend fun forget(): Outcome<Unit>
}

/** The library's own error code and the link's (ferb-link errors.rs), when there is one. */
val FerbException.linkCode: String?
    get() = when (this) {
        is FerbException.Closed -> linkCode
        is FerbException.BadKey -> linkCode
        is FerbException.WrongKey -> linkCode
        is FerbException.StoreNewer -> linkCode
        is FerbException.Store -> linkCode
        is FerbException.Offline -> linkCode
        is FerbException.NeedsBetterPath -> linkCode
        is FerbException.NeedsUnlock -> linkCode
        is FerbException.Refused -> linkCode
        is FerbException.NeedsSigner -> linkCode
        is FerbException.NotSigned -> linkCode
        is FerbException.NotFound -> linkCode
        is FerbException.InFlight -> linkCode
        is FerbException.BadReply -> linkCode
        is FerbException.BadArgument -> linkCode
        is FerbException.Internal -> linkCode
    }

val PhoneState.keys: KeyHealth? get() = (this as? PhoneState.Open)?.keys

/** The real phone. */
class LibraryPhone(private val phone: FerbPhone) : PhoneApi {
    override val state get() = phone.state
    override val link get() = phone.link
    override val changes get() = phone.changes

    override suspend fun transcript(): Outcome<TranscriptView?> = phone.call { m ->
        val conv = m.currentConversation() ?: m.conversations().firstOrNull { it.current }?.id
        conv?.let(m::transcript)
    }

    override suspend fun sync() = phone.call { it.sync() }
    override suspend fun send(text: String) = phone.call { it.send(text) }
    override suspend fun cancel(outboxId: String) = phone.call { it.cancel(outboxId) }
    override suspend fun answer(approvalId: String, input: AnswerInput, signer: Signer?) = phone.call { it.answer(approvalId, input, signer) }
    override suspend fun stop(taskIds: List<String>) = phone.call { it.stop(taskIds) }
    override suspend fun loadOlder(conversation: String) = phone.call { it.loadOlder(conversation) }
    override suspend fun unlock(signer: Signer) = phone.unlock(signer)
    override suspend fun pair(code: String) = phone.pair(code)
    override suspend fun forget() = phone.forgetThisMac()
}
