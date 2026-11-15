//! Named points inside the file work where a test can act as a racer (swap a link in, change a file) at exactly that
//! moment, deterministically: what the Python tests do by patching `os.access`, `_skip`, `trash_folder` or
//! `move_excl`. Each thread has its own hook; with none set a point costs one thread-local read.

use std::cell::RefCell;

type Hook = Box<dyn FnMut(&str)>;

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Run `f` at every point this thread reaches until [`clear`].
pub fn set(f: impl FnMut(&str) + 'static) {
    HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

pub fn clear() {
    HOOK.with(|h| *h.borrow_mut() = None);
}

/// A point in the file work, by name (`"atomic_text.opened"`, ...).
pub fn at(point: &str) {
    HOOK.with(|h| {
        let taken = h.borrow_mut().take();
        if let Some(mut f) = taken {
            f(point);
            let mut slot = h.borrow_mut();
            if slot.is_none() {
                *slot = Some(f);
            }
        }
    });
}

thread_local! {
    static OFF: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Make these system calls unavailable on this thread until [`clear_off`] (`clonefileat`, `clonefile`,
/// `fclonefileat`), as a volume that can't clone: what the Python tests do by setting the ctypes function to None.
pub fn set_off(names: &[&str]) {
    OFF.with(|o| *o.borrow_mut() = names.iter().map(|n| n.to_string()).collect());
}

pub fn clear_off() {
    OFF.with(|o| o.borrow_mut().clear());
}

/// Whether a test made this call unavailable.
pub fn off(name: &str) -> bool {
    OFF.with(|o| o.borrow().iter().any(|n| n == name))
}
