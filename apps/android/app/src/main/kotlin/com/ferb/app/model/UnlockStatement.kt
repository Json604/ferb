package com.ferb.app.model

import java.security.SecureRandom

/**
 * The unlock the lock key signs in the app's opening prompt before the link is up (kb PHN-649): the phone's own
 * statement `{domain, nonce, made}`, its signature, and when it was signed by the phone's steady clock (the age the
 * Mac is told; the wall clock in `made` is only reported). One per prompt; it goes to the Mac once.
 */
class UnlockStatement(val payload: String, val sig: String, val signedAt: Long) {
    companion object {
        /** The domain the Mac checks (ferb-daemon phone::gate::STATEMENT_DOMAIN); nothing else the phone signs has it. */
        const val DOMAIN = "ferb.unlock-statement.v1"
        private val random = SecureRandom()

        /** A new statement's text: a fresh 128-bit nonce and the phone's wall time in ms. */
        fun make(wallMs: Long): String {
            val nonce = ByteArray(16).also { random.nextBytes(it) }.joinToString("") { "%02x".format(it) }
            return """{"domain":"$DOMAIN","nonce":"$nonce","made":$wallMs}"""
        }
    }
}
