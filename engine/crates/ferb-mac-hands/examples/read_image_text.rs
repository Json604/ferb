//! FERB's image-text reader as its own program, for the by-hand test of the child path
//! (`tests/native.rs` `a_child_process_reads_an_image`): what `ferb read-image-text` runs in FERB.app.

fn main() {
    // `--serve`: the kept child (HAND-626), as `ferb read-image-text --serve`
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some(ferb_mac_hands::vision_child::SERVE) {
        std::process::exit(ferb_mac_hands::native::vision::serve_main());
    }
    #[cfg(target_os = "macos")]
    std::process::exit(ferb_mac_hands::native::vision::child_main());
    #[cfg(not(target_os = "macos"))]
    std::process::exit(64);
}
