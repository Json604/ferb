package com.ferb.core

import android.content.Context
import android.hardware.biometrics.BiometricManager.Authenticators
import android.hardware.biometrics.BiometricPrompt
import android.os.CancellationSignal
import android.os.Looper
import android.util.Log
import com.ferb.mobile.DeviceSigner
import com.ferb.mobile.Signer
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/** The prompt's words: the app's to choose (owner-facing text). [cancel] labels the button of a biometric-only prompt. */
data class PromptText(val title: String, val subtitle: String? = null, val description: String? = null, val cancel: String)

/**
 * Signs one exact payload with the LOCK or STRONG key inside the system's biometric prompt (the platform
 * BiometricPrompt, Android 11's API: minSdk 30 has everything this needs, so no AndroidX layer). The library calls
 * [sign] on the thread that called `Mobile.unlock` or `Mobile.answer` (a background thread): this shows the prompt on
 * the main thread and waits for it. Returns null when the owner cancelled, the prompt failed, or the key is gone
 * ([onGone] is told which: the app then says "pair again"). kb PHN-497.
 */
class PromptSigner(
    private val context: Context,
    private val keys: PhoneKeys,
    private val kind: KeyKind,
    private val text: PromptText,
    private val onGone: (KeyKind) -> Unit = {},
    private val waitMs: Long = 5 * 60_000L,
) : Signer {
    init {
        require(kind != KeyKind.DEVICE) { "the device key signs without a prompt: DeviceKeySigner" }
    }

    override fun sign(payload: String): String? {
        check(Looper.myLooper() != Looper.getMainLooper()) { "PromptSigner.sign waits for the prompt: never on the main thread" }
        val signature = try {
            keys.signatureFor(kind)
        } catch (e: KeyGone) {
            onGone(kind)
            return null
        }
        val done = CountDownLatch(1)
        val out = AtomicReference<String?>(null)
        val cancel = CancellationSignal()
        val callback = object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                try {
                    val s = result.cryptoObject?.signature ?: return
                    s.update(payload.toByteArray(Charsets.UTF_8))
                    out.set(Proofs.signature(s.sign()))
                } catch (e: Exception) {
                    Log.w(Keystore.TAG, "the ${kind.name} key did not sign: ${e.javaClass.simpleName}")
                } finally {
                    done.countDown()
                }
            }

            override fun onAuthenticationError(code: Int, message: CharSequence) {
                done.countDown()
            }
            // onAuthenticationFailed (a finger not recognised) keeps the prompt open: nothing to do
        }
        context.mainExecutor.execute {
            try {
                val builder = BiometricPrompt.Builder(context).setTitle(text.title)
                text.subtitle?.let(builder::setSubtitle)
                text.description?.let(builder::setDescription)
                if (kind == KeyKind.STRONG) {
                    // biometric only: no PIN fallback (PHN-137); a biometric-only prompt needs its own cancel button
                    builder.setAllowedAuthenticators(Authenticators.BIOMETRIC_STRONG)
                    builder.setNegativeButton(text.cancel, context.mainExecutor) { _, _ -> done.countDown() }
                } else {
                    builder.setAllowedAuthenticators(Authenticators.BIOMETRIC_STRONG or Authenticators.DEVICE_CREDENTIAL)
                }
                builder.build().authenticate(BiometricPrompt.CryptoObject(signature), cancel, context.mainExecutor, callback)
            } catch (e: Exception) {
                Log.w(Keystore.TAG, "the prompt could not be shown: ${e.javaClass.simpleName}")
                done.countDown()
            }
        }
        if (!done.await(waitMs, TimeUnit.MILLISECONDS)) cancel.cancel()
        return out.get()
    }
}

/** The device key for the link (`link.present`, `prove`): no prompt, answers on the link's own thread. */
class DeviceKeySigner(private val keys: PhoneKeys) : DeviceSigner {
    override fun sign(payload: String): String? = keys.deviceSign(payload)
}
