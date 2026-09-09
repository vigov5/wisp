package dev.vigov5.wisp

import android.content.Context
import android.util.Log

/**
 * Gives the Rust side Android's JavaVM and application Context.
 *
 * Needed because `dlopen` — which is how flutter_rust_bridge loads
 * libwisp_bridge.so — does not run `JNI_OnLoad`, so Rust has no way to reach
 * the VM on its own. Without it `iroh-dns` silently resolves through Google's
 * public DNS instead of the network's resolver. See android_context.rs for
 * what this does and does not fix.
 *
 * Safe to call more than once; the Rust side ignores repeat calls.
 */
object WispNative {
    private const val TAG = "WispNative"

    @Volatile
    private var installed = false

    fun install(context: Context) {
        if (installed) return
        try {
            // The library is normally already resident (Flutter opens it via
            // dlopen), but a `native` method only binds after a
            // System.loadLibrary for the same object, and loading twice is
            // harmless — the loader returns the existing handle.
            System.loadLibrary("wisp_bridge")
            installAndroidContext(context.applicationContext)
            installed = true
        } catch (error: Throwable) {
            // Never fatal: the app works without it, just with a public DNS
            // fallback and a noisier log.
            Log.w(TAG, "could not install the Android context for native code", error)
        }
    }

    @JvmStatic
    private external fun installAndroidContext(context: Context)
}
