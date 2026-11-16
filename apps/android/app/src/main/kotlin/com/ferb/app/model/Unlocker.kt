package com.ferb.app.model

import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.linkCode
import com.ferb.mobile.FerbException
import com.ferb.mobile.Signer

/** What a prompt is for: its words (the system prompt's subtitle). */
enum class Why { OPEN, YES, FILE, SETTING, KEYS }

/**
 * The one path every call to the Mac takes when it may need the owner (PHN-13): a call refused because the Mac's
 * unlock window closed is unlocked (one prompt, the lock key) and made once more; a call that needs the phone's fresh
 * proof (a key place, a sensitive change, PHN-121, PHN-129, PHN-133) is signed by the fingerprint-only key where the
 * phone has one, else the lock key. Nothing here decides what needs a proof: the Mac says so, or the listing marks
 * the entry (`locked`) and the proof is asked up front.
 */
class Unlocker(
    private val api: PhoneApi,
    private val keys: () -> KeyHealth?,
    private val signer: (KeyKind, Why) -> Signer?,
) {
    /** The key for a fresh proof, or null: this phone's keys changed and it must pair again. */
    fun freshKind(): KeyKind? {
        val k = keys() ?: return null
        return when {
            k.strong == KeyState.OK -> KeyKind.STRONG
            k.strong == KeyState.GONE -> null
            k.lock == KeyState.OK -> KeyKind.LOCK
            else -> null
        }
    }

    /** A signer for a fresh proof, or null (pair again; [PAIR_AGAIN]). */
    fun fresh(why: Why): Signer? = freshKind()?.let { signer(it, why) }

    /** [block], and once more after the Mac's unlock when the Mac's window had closed. */
    suspend fun <T> run(block: suspend () -> Outcome<T>): Outcome<T> {
        val r = block()
        val e = (r as? Outcome.Failed)?.error
        if (e !is FerbException.NeedsUnlock || e.linkCode in PROOF_CODES) return r
        val s = signer(KeyKind.LOCK, Why.OPEN) ?: return r
        when (val u = api.unlock(s)) {
            is Outcome.Ok -> Unit
            is Outcome.Failed -> return u
        }
        return block()
    }

    /**
     * [block] with a fresh proof: asked up front when [upfront] (the listing marked the place), else only when the Mac
     * asks for one. Each try with a signer is its own prompt (the Mac's challenge is one-time).
     */
    suspend fun <T> withProof(upfront: Boolean, why: Why, block: suspend (Signer?) -> Outcome<T>): Outcome<T> {
        var proof: Signer? = null
        if (upfront) proof = fresh(why) ?: return pairAgain()
        val r = run { block(proof) }
        val e = (r as? Outcome.Failed)?.error
        if (proof != null || e !is FerbException.NeedsUnlock || e.linkCode !in PROOF_CODES) return r
        proof = fresh(why) ?: return pairAgain()
        return run { block(proof) }
    }

    private fun pairAgain(): Outcome.Failed = Outcome.Failed(FerbException.NeedsSigner("Pair this phone again.", PAIR_AGAIN))

    companion object {
        /** The Mac's codes for "this call needs the phone's own proof" (not "the window closed"). */
        val PROOF_CODES = setOf("unlock_required", "unlock_invalid", "fresh_unlock_required")

        /** The app's own code: the key that would sign is gone (the fingerprints changed): pair again. */
        const val PAIR_AGAIN = "pair_again"
    }
}
