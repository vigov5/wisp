//! Hands Android's `JavaVM` and application `Context` to `ndk-context`.
//!
//! Three dependencies reach for that context: `iroh-dns`, `hickory-resolver`
//! and `netdev`.
//!
//! The confirmed cost is DNS. Without the context the resolver cannot read the
//! system config, so every lookup goes to Google's public DNS —
//! `Failed to read the system's DNS config, using Google DNS servers as
//! fallback. reason=ndk_context not initialized`. Measured on a Pixel 4 over
//! matched 25 s app-launch windows: that warning fires once per endpoint build
//! without this call and never with it, with a `creating pkarr publisher` line
//! as the witness that the endpoint really was built in both. So the app now
//! resolves through the network's own DNS instead of a third party's.
//!
//! Two things this does **not** fix, both of which an earlier draft of this
//! comment asserted:
//!
//! * `ndk_context::android_context()` panics rather than degrading when
//!   uninitialized, and `thread 'tokio-rt-worker' panicked ... android context
//!   was not initialized` is real — but it came from the standalone `wisp` CLI
//!   binary, not from the app, which never logged it either before or after.
//! * The SELinux `avc: denied ... sysfs_net` records are unrelated: 5 before
//!   and 9 after, varying run to run in both builds. Whatever probes
//!   `/sys/class/net` still does, and still floods logcat.
//!
//! `JNI_OnLoad` cannot be used for this. flutter_rust_bridge loads the library
//! through `DynamicLibrary.open`, i.e. `dlopen`, and `dlopen` does not invoke
//! `JNI_OnLoad` — only `System.loadLibrary` does. So the call has to come from
//! Kotlin; see `WispNative.install`.

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JObject};
use jni::JNIEnv;

/// Holds the `Context` for the life of the process. `ndk-context` keeps only
/// the bare pointer, so a local JNI reference would dangle as soon as this
/// call returned.
static CONTEXT: OnceLock<GlobalRef> = OnceLock::new();

/// Called once from Kotlin at activity startup, long before any transfer
/// touches the network.
///
/// The second parameter is typed as `JObject` rather than `JClass` on purpose:
/// that slot is a `jclass` for a static native method and a `jobject` for an
/// instance one, both are the same pointer, and typing it loosely means a
/// change on the Kotlin side (dropping `@JvmStatic`, say) cannot turn into a
/// silent mismatch here.
///
/// Errors are swallowed rather than unwrapped. A panic across an `extern "system"`
/// boundary is undefined behaviour, and the failure mode without this call is
/// the public-DNS fallback described above — working, just worse — which is not
/// worth aborting the process for.
#[no_mangle]
pub extern "system" fn Java_dev_vigov5_wisp_WispNative_installAndroidContext(
    env: JNIEnv,
    _this_or_class: JObject,
    context: JObject,
) {
    // Idempotent: a second call would leak another global reference and
    // overwrite the pointer with an equivalent one for no gain.
    if CONTEXT.get().is_some() {
        return;
    }
    let Ok(vm) = env.get_java_vm() else {
        return;
    };
    let Ok(global) = env.new_global_ref(&context) else {
        return;
    };
    let vm_ptr = vm.get_java_vm_pointer().cast();
    let context_ptr = global.as_raw().cast();
    // Publish the reference before the pointer, so the pointer ndk-context
    // stores can never outlive the reference keeping it alive.
    if CONTEXT.set(global).is_err() {
        return;
    }
    // SAFETY: both pointers come from a live JNI environment on this thread;
    // the VM pointer is process-wide and the context is now a global ref held
    // in `CONTEXT` for the life of the process.
    unsafe { ndk_context::initialize_android_context(vm_ptr, context_ptr) };
}
