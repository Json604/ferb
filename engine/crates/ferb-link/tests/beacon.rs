//! PHN-590, PHN-591: the shared relays' second endpoint (PHN-220) is open while a paired phone is still to come, and
//! at most BEACON_MAX_DAYS after the relay setting last changed.

use ferb_link::bridge::{BEACON_MAX_DAYS, beacon_open};

// decision: PHN-590
// decision: PHN-591
#[test]
fn the_shared_relays_are_kept_open_for_a_phone_at_most_the_bound() {
    let set = 1_000_000u64;
    let closes = Some(set + BEACON_MAX_DAYS * 86_400);
    assert!(beacon_open(1, closes, set + 29 * 86_400));
    assert!(!beacon_open(1, closes, set + 30 * 86_400), "past the bound");
    assert!(!beacon_open(0, closes, set), "every phone came back");
    assert!(beacon_open(2, None, set), "no own relay time: as before");
    assert_eq!(BEACON_MAX_DAYS, 30);
}
