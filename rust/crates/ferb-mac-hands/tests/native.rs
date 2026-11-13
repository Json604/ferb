//! Read-only checks of the native layer on this Mac, for a human to run (`cargo test -- --ignored`): they list
//! apps and windows, read the frontmost app's title through Accessibility, encode, paint and OCR an image made in
//! memory, and read permission states without prompting. None of them posts an event, changes focus or a Space,
//! launches an app, opens a window, writes the clipboard, starts a recording or asks for a permission.
#![cfg(target_os = "macos")]

use ferb_mac_hands::ax::{Ax as AxWalk, AxSource};
use ferb_mac_hands::driver::{Apps, Shots};
use ferb_mac_hands::native;
use ferb_mac_hands::ocr::{Paint, Recognize};
use ferb_mac_hands::shots::WindowList;

#[test]
#[ignore = "reads this Mac: run by hand"]
fn lists_running_apps_and_windows() {
    let apps = native::apps::NativeApps::new(Default::default());
    let running = apps.running();
    assert!(!running.is_empty(), "some regular app is running");
    assert!(
        running.iter().any(|a| a.bundle_id == "com.apple.finder"),
        "Finder runs"
    );
    let windows = native::windows::NativeWindows
        .list(true)
        .expect("a window list");
    assert!(!windows.is_empty());
    let shots = native::capture::NativeShots;
    let w = &windows[0];
    assert_eq!(shots.window(w.id).map(|x| x.id), Some(w.id));
    println!(
        "{} apps, {} windows; screen off: {:?}",
        running.len(),
        windows.len(),
        native::windows::screen_off()
    );
}

#[test]
#[ignore = "reads this Mac: run by hand (needs Accessibility for the test binary to see a title)"]
fn reads_the_frontmost_apps_window_title() {
    let apps = native::apps::NativeApps::new(Default::default());
    let front = apps.frontmost().expect("an app is in front");
    let ax = AxWalk::new(native::ax::NativeAx::default());
    let app = ax.src.app(front.pid);
    let role = ax.role_of(&app);
    let windows = ax.ax_windows(front.pid);
    let titles: Vec<String> = windows.iter().map(|w| ax.window_title(Some(w))).collect();
    println!(
        "{} ({}): role {role:?}, windows {titles:?}",
        front.name, front.bundle_id
    );
}

#[test]
#[ignore = "uses ImageIO and Vision on this Mac: run by hand"]
fn encodes_paints_and_reads_an_image_made_in_memory() {
    let png = native::vision::blank_png(160, 48);
    let img = native::paint::decode(&png).expect("our PNG decodes");
    let jpeg = native::paint::jpeg(&img).expect("encodes");
    assert_eq!(native::paint::size(&jpeg).unwrap(), (160, 48));
    let painted = native::paint::NativePaint
        .paint(&jpeg, &[(10, 10, 20, 20)])
        .expect("paints");
    assert_eq!(native::paint::size(&painted).unwrap(), (160, 48));
    let regions = native::vision::NativeVision
        .recognize(&png)
        .expect("Vision runs");
    assert!(
        regions.is_empty(),
        "a blank image holds no text: {regions:?}"
    );
}

#[test]
#[ignore = "reads TCC state without prompting: run by hand"]
fn reads_permission_states_without_asking() {
    println!("{}", native::permissions::check_all());
    println!(
        "clipboard access: {}",
        native::pasteboard::access_behavior()
    );
}
