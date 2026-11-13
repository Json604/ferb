//! The general pasteboard (clipboard.py): mac.clipboard's write (no focus or keyboard input), the read before a
//! paste into a terminal (a concealed or transient item's text is never returned), and the clipboard row's access
//! behavior (never its contents).

use objc2_app_kit::{NSPasteboard, NSPasteboardAccessBehavior, NSPasteboardTypeString};
use objc2_foundation::NSString;

use crate::tools::{Pasteboard, CONCEALED};

pub struct NativePasteboard;

impl Pasteboard for NativePasteboard {
    fn write(&self, text: &str) -> Result<i64, String> {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            // SAFETY: the exported string type.
            let kind = unsafe { NSPasteboardTypeString };
            if !board.setString_forType(&NSString::from_str(text), kind) {
                return Err("the pasteboard did not accept the text".to_string());
            }
            Ok(board.changeCount() as i64)
        })
    }

    fn read(&self) -> (i64, Option<String>, bool) {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            let types: Vec<String> = board
                .types()
                .map(|t| t.to_vec().iter().map(|x| x.to_string()).collect())
                .unwrap_or_default();
            let concealed = types.iter().any(|t| CONCEALED.contains(&t.as_str()));
            // SAFETY: the exported string type.
            let kind = unsafe { NSPasteboardTypeString };
            let text = if concealed {
                None
            } else {
                board.stringForType(kind).map(|s| s.to_string())
            };
            (board.changeCount() as i64, text, concealed)
        })
    }
}

/// The clipboard setup row's state (permissions.py `clipboard_row`): "default", "ask", "allow", "deny".
pub fn access_behavior() -> &'static str {
    objc2::rc::autoreleasepool(
        |_| match NSPasteboard::generalPasteboard().accessBehavior() {
            NSPasteboardAccessBehavior::Default => "default",
            NSPasteboardAccessBehavior::Ask => "ask",
            NSPasteboardAccessBehavior::AlwaysAllow => "allow",
            _ => "deny",
        },
    )
}
