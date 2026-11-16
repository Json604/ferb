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

/// HAND-531's last attempt for real: FERB's image-text reader (the `read_image_text` example, what
/// `ferb read-image-text` runs) started as a child process, the image over its input, the regions back over its output.
// decision: HAND-531
#[test]
#[ignore = "starts Vision in a child process on this Mac: run by hand (cargo builds the example with the tests)"]
fn a_child_process_reads_an_image() {
    let exe = std::env::current_exe().unwrap();
    // the profile folder above this test binary (target/<profile>/deps, or a build folder below it)
    let program = exe
        .ancestors()
        .map(|dir| dir.join("examples").join("read_image_text"))
        .find(|p| p.is_file())
        .unwrap_or_default();
    assert!(
        program.is_file(),
        "build it: cargo test --examples ({program:?})"
    );
    let child = ferb_mac_hands::vision_child::ChildVision {
        program,
        args: vec![],
        timeout: ferb_mac_hands::vision_child::CHILD_TIMEOUT,
        gate: Default::default(),
        wait: true,
    };
    let started = std::time::Instant::now();
    let regions = child
        .recognize(&native::vision::blank_png(160, 48))
        .expect("the child read the image");
    assert!(
        regions.is_empty(),
        "a blank image holds no text: {regions:?}"
    );
    println!("the child answered in {:?}", started.elapsed());
    // the same image through the CPU attempt in this process
    let cpu = native::vision::CpuVision.recognize(&native::vision::blank_png(160, 48));
    println!("Vision on the CPU in this process: {cpu:?}");
}

/// HAND-584, by hand only (it changes which app is in front): does macOS let this process put an app back in front,
/// and does the front-app read see it? Open TextEdit and Finder windows, click into another app (the "owner's"),
/// then run `cargo test -p ferb-mac-hands --test native put_back_by_hand -- --ignored --nocapture` from Terminal
/// and switch to TextEdit within 3 s. The test activates the app that was in front 3 s in, then prints the front app
/// at each of the put-back's reads. From FERB's daemon (a background agent) the result can differ from Terminal's:
/// run the same steps through FERB on a disposable window to see what the daemon gets.
#[test]
#[ignore = "changes the front app on this Mac: run by hand"]
fn put_back_by_hand() {
    let apps = native::apps::NativeApps::new(Default::default());
    let before = apps.frontmost().expect("a front app");
    println!("in front at start: {} ({})", before.name, before.pid);
    std::thread::sleep(std::time::Duration::from_secs(3));
    let raised = apps.frontmost().expect("a front app");
    println!("in front 3 s in: {} ({})", raised.name, raised.pid);
    let accepted = apps.activate(&before.bundle_id, Some(before.pid));
    println!("activate {} accepted: {accepted}", before.bundle_id);
    std::thread::sleep(std::time::Duration::from_millis(150));
    for i in 0..ferb_mac_hands::tools::PUT_BACK_READS {
        let f = apps.frontmost().map(|a| a.name).unwrap_or_default();
        println!("read {i}: {f}");
        std::thread::sleep(std::time::Duration::from_secs_f64(
            ferb_mac_hands::tools::PUT_BACK_STEP_S,
        ));
    }
}
