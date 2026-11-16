package com.ferb.core

import android.content.Context
import android.util.Log

/**
 * Hands the JavaVM and the application context to the phone library (`Java_com_ferb_core_NativeInit_init`, its one
 * JNI function: ndk_context), so iroh's DNS resolver can read the network's DNS servers on Android. Called once, from
 * [FerbPhone.create]'s start, before the link starts; never on the JVM tests (they load the host library through JNA,
 * and `System.loadLibrary` would look for an Android library there). R8 keeps the class and the native method's name
 * (consumer-rules.pro).
 */
object NativeInit {
    init {
        System.loadLibrary("ferb_mobile_ffi")
    }

    @JvmStatic external fun init(context: Context)

    @Volatile private var done = false

    /** [init] once per process. A library without the function only logs: the link then uses its own resolver. */
    @Synchronized
    fun once(context: Context) {
        if (done) return
        try {
            init(context.applicationContext)
            done = true
        } catch (e: UnsatisfiedLinkError) {
            Log.e(Keystore.TAG, "the phone library could not be given the app's context: ${e.message}")
        }
    }
}
