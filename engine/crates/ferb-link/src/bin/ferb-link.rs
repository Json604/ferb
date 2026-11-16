//! The harness the owner drives (`ferb_link_mac.rs`): the Mac bridge with a prompt, a phone simulator and the relay
//! test. The Mac side of the link exists only on macOS (kb ENG-5).

#[cfg(target_os = "macos")]
#[path = "ferb_link_mac.rs"]
mod mac;

#[cfg(target_os = "macos")]
fn main() {
    mac::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("ferb-link: the Mac side of the phone link runs only on macOS");
    std::process::exit(2);
}
