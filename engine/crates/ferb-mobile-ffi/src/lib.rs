//! ferb-mobile-ffi: the phone library (stage 3, slice S8; kb T-1208, PHN-446). ferb-mobile (the chat, sync, outbox,
//! answers) and ferb-link (the link to the Mac, pairing) behind one set of UniFFI bindings, so each app links one
//! library: `libferb_mobile_ffi.so` on Android, the static archive on iPhone. README.md has the API as Kotlin sees it,
//! the threading rules and how the bindings are generated.
//!
//! - `mobile`: `Mobile`, the core (ferb-mobile `api::Mobile`).
//! - `link`: `Link`, the phone link (ferb-link `ffi::LinkClient`) with pairing and status, plugged into the core as
//!   its transport through ferb-mobile's `door`.
//! - `callbacks`: what the app supplies (clock, ids, listeners, signers, a transport of its own).
//! - `records`: what the app reads (the transcript and the rest), total copies of the core's types.
//! - `error`: `FerbError`, one enum with the stable codes, and the panic guard every call runs in.
//!
//! No global state: every object owns what it uses, and a process may hold several of each.

uniffi::setup_scaffolding!();

#[cfg(target_os = "android")]
mod android;
pub mod callbacks;
pub mod error;
pub mod link;
pub mod mobile;
pub mod records;

pub use callbacks::{
    Clock, DeviceSigner, Ids, Listener, Signer, TransferListener, TransferProgress, Transport,
    WatchSink,
};
pub use error::{FerbError, TransportError};
pub use link::{Link, LinkConfig, LinkListener, LinkPhase, LinkProblem, LinkStatus, RelayStatus};
pub use mobile::{AnswerInput, LivePreview, Mobile, MobileConfig, PreviewSink};

/// This library's version (the engine's).
#[uniffi::export]
pub fn library_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Test hooks: present only in a build with the `testing` feature (the host library the bindings checks load) and in
/// this crate's own tests; never in a phone library.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use crate::error::{FerbError, guard};

    /// A call that panics inside the boundary guard: the caller gets `FerbError::Internal`.
    #[uniffi::export]
    pub fn testing_panic() -> Result<(), FerbError> {
        guard(|| panic!("a deliberate panic (test hook)"))
    }

    /// A call that panics outside any guard of this crate: UniFFI's own scaffolding catches it (an unexpected error
    /// on the foreign side, never an abort).
    #[uniffi::export]
    pub fn testing_unguarded_panic() {
        panic!("a deliberate panic past the guard (test hook)")
    }
}

/// The panic paths through UniFFI's own scaffolding: the exported C functions the generated Kotlin and Swift call.
#[cfg(test)]
mod tests {
    use std::mem::ManuallyDrop;

    use uniffi::{Lift, RustCallStatus, RustCallStatusCode};

    use crate::error::FerbError;

    unsafe extern "C" {
        fn uniffi_ferb_mobile_ffi_fn_func_testing_panic(status: &mut RustCallStatus);
        fn uniffi_ferb_mobile_ffi_fn_func_testing_unguarded_panic(status: &mut RustCallStatus);
    }

    /// A panic inside a call reaches the app as the typed error `internal` (a Kotlin `FerbException.Internal`), and
    /// the process goes on.
    // decision: PHN-446
    // decision: PHN-492
    #[test]
    fn a_panic_becomes_the_typed_error() {
        let mut status = RustCallStatus::default();
        // SAFETY: the scaffolding function UniFFI generated for `testing_panic`, called as the bindings call it: with
        // a fresh status whose error buffer is empty.
        unsafe { uniffi_ferb_mobile_ffi_fn_func_testing_panic(&mut status) };
        assert!(matches!(status.code, RustCallStatusCode::Error));
        let buf = ManuallyDrop::into_inner(status.error_buf);
        let e = <FerbError as Lift<crate::UniFfiTag>>::try_lift_from_rust_buffer(buf).unwrap();
        assert_eq!(e.code(), "internal");
        assert!(matches!(e, FerbError::Internal { .. }));
    }

    /// One library per phone (PHN-514): ferb-link builds no library of its own (its C and JNI door is gone), and this
    /// one carries the Android DNS context's JNI init the lab's library had.
    // decision: PHN-514
    #[test]
    fn the_phone_ships_one_library() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let link = std::fs::read_to_string(root.join("../ferb-link/Cargo.toml")).unwrap();
        assert!(
            !link.contains("\"cdylib\""),
            "ferb-link builds a library again"
        );
        let door = std::fs::read_to_string(root.join("../ferb-link/src/ffi.rs")).unwrap();
        assert!(!door.contains("extern \"C\"") && !door.contains("extern \"system\""));
        let init = std::fs::read_to_string(root.join("src/android.rs")).unwrap();
        assert!(init.contains("Java_com_ferb_core_NativeInit_init"));
        assert!(init.contains("initialize_android_context"));
    }

    /// A panic past every guard of this crate is still caught by UniFFI's scaffolding (the bindings raise their
    /// internal error), never an abort.
    // decision: PHN-446
    #[test]
    fn a_panic_past_the_guard_is_still_caught() {
        let mut status = RustCallStatus::default();
        // SAFETY: as above, for `testing_unguarded_panic`.
        unsafe { uniffi_ferb_mobile_ffi_fn_func_testing_unguarded_panic(&mut status) };
        assert!(matches!(status.code, RustCallStatusCode::UnexpectedError));
        let buf = ManuallyDrop::into_inner(status.error_buf);
        // an unexpected error's buffer holds the panic's message as plain UTF-8
        let message = String::from_utf8(buf.destroy_into_vec()).unwrap();
        assert!(message.contains("deliberate panic"), "{message}");
    }
}
