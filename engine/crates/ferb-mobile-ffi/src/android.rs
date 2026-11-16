//! Android only: the one JNI function the phone library exports, `com.ferb.core.NativeInit.init(context)`, which hands
//! the process's JavaVM and application context to `ndk_context` once. iroh's DNS resolver (n0-dns-resolver's
//! `system_config/android.rs`) reads the network's DNS servers through it; without it the resolver falls back to its
//! public resolvers. The lab's own library did this in `local.ferb.link.Native.init` (ferb-link's JNI door, removed
//! in T-1221 with ferb-link's cdylib). :core calls it once (System.loadLibrary of this same library, then `init`)
//! before the first link starts. Everything else crosses through UniFFI.

use jni::JNIEnv;
use jni::objects::{JClass, JObject};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ferb_core_NativeInit_init(
    env: JNIEnv,
    _class: JClass,
    context: JObject,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let (Ok(vm), Ok(global)) = (env.get_java_vm(), env.new_global_ref(&context)) else {
                return;
            };
            // SAFETY: the JavaVM pointer and a global reference to the application context stay valid for the life of
            // the process (the global reference is leaked on purpose below, so ndk_context's raw pointer stays valid).
            unsafe {
                ndk_context::initialize_android_context(
                    vm.get_java_vm_pointer() as *mut std::ffi::c_void,
                    global.as_obj().as_raw() as *mut std::ffi::c_void,
                );
            }
            std::mem::forget(global);
        });
    }));
}
