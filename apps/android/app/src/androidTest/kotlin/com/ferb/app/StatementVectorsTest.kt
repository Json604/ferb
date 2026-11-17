package com.ferb.app

import android.util.Log
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.ferb.app.model.UnlockStatement
import com.ferb.core.KeystoreKeys
import org.junit.Assert.assertNotNull
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Unlock statements (kb PHN-649) as this phone makes and signs them: the app's own statement text, signed by an
 * Android Keystore P-256 key in the Mac's wire form, logged as vectors for engine/fixtures/phone/android-signatures.json,
 * where the Mac's real verifier and its statement check take them (ferb-daemon proof.rs, tests/phone_open.rs). The
 * device key signs here (no prompt in a test); the lock key's signature is the same algorithm and form.
 */
@RunWith(AndroidJUnit4::class)
class StatementVectorsTest {
    // decision: PHN-649
    @Test
    fun statements_are_signed_in_the_macs_form() {
        val keys = KeystoreKeys(ApplicationProvider.getApplicationContext())
        keys.deleteAll()
        try {
            val pubs = keys.ensure()
            assertNotNull(pubs.device)
            val vectors = (1..2).map { i ->
                val payload = UnlockStatement.make(1_700_000_000_000L + i)
                val sig = keys.deviceSign(payload)!!
                """{"what":"android unlock statement $i","statement":true,"key":"${pubs.device}","payload":${quote(payload)},"sig":"$sig","valid":true}"""
            }
            Log.i("ferb-statement-vectors", vectors.joinToString(","))
        } finally {
            keys.deleteAll()
        }
    }

    private fun quote(s: String) = "\"" + s.replace("\\", "\\\\").replace("\"", "\\\"") + "\""
}
