//! FERB's image-text reader as its own program, for the by-hand test of the child path
//! (`tests/native.rs` `a_child_process_reads_an_image`): what `ferb read-image-text` runs in FERB.app.

fn main() {
    #[cfg(target_os = "macos")]
    std::process::exit(ferb_mac_hands::native::vision::child_main());
    #[cfg(not(target_os = "macos"))]
    std::process::exit(64);
}
