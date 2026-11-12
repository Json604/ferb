//! Who is on the other end of a Unix socket, as the kernel says. Port of `server.py:91-108` (`peer_pid`,
//! `peer_uid`) and `app_peer.py:21-33` (`peer_token`).

use std::mem::{size_of, MaybeUninit};
use std::os::fd::AsRawFd;

const SOL_LOCAL: libc::c_int = 0; // <sys/un.h>
const LOCAL_PEERPID: libc::c_int = 0x002;
const LOCAL_PEERTOKEN: libc::c_int = 0x006; // the peer's audit_token_t

/// The size of the kernel's `audit_token_t`: its pid and pid version among others, so a reused pid can't stand
/// in for the process that connected.
pub const AUDIT_TOKEN_SIZE: usize = 32;
pub type AuditToken = [u8; AUDIT_TOKEN_SIZE];

/// This process's user: the owner, unless a test says otherwise (`os.getuid()`, server.py:130).
pub fn current_uid() -> u32 {
    // SAFETY: getuid takes no arguments, touches no memory and cannot fail.
    unsafe { libc::getuid() }
}

/// The connected process's uid, as the socket recorded it at connect (server.py:106-108 reads LOCAL_PEERCRED's
/// `xucred`; tokio asks `getpeereid`, which reads the same credential).
pub fn peer_uid(stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    Ok(stream.peer_cred()?.uid())
}

fn local_option<T: Copy>(socket: &impl AsRawFd, option: libc::c_int) -> Option<T> {
    let mut value = MaybeUninit::<T>::zeroed();
    let mut len = size_of::<T>() as libc::socklen_t;
    // SAFETY: `value` is a writable buffer of exactly `len` bytes that outlives the call, `len` is a valid
    // in/out length, and the descriptor is open for as long as `socket` is borrowed. The kernel writes at most
    // `len` bytes.
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            SOL_LOCAL,
            option,
            value.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 || len as usize != size_of::<T>() {
        return None;
    }
    // SAFETY: the buffer was zero-initialised and the kernel filled all `size_of::<T>()` bytes; the `T`s used
    // here (an integer, a byte array) are valid for every bit pattern.
    Some(unsafe { value.assume_init() })
}

/// The connected process's pid, or `None` when the socket can't say (server.py:94-103).
pub fn peer_pid(socket: &impl AsRawFd) -> Option<i32> {
    local_option::<libc::pid_t>(socket, LOCAL_PEERPID)
}

/// The connected process's audit token, or `None` when the socket can't say (app_peer.py:27-33).
pub fn peer_token(socket: &impl AsRawFd) -> Option<AuditToken> {
    local_option::<AuditToken>(socket, LOCAL_PEERTOKEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_pair_s_peer_is_this_process() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(peer_pid(&a), Some(std::process::id() as i32));
        let token = peer_token(&a).expect("an audit token");
        // audit_token_t.val[5] is the pid
        assert_eq!(
            u32::from_ne_bytes(token[20..24].try_into().unwrap()),
            std::process::id()
        );
    }

    #[test]
    fn a_file_is_not_a_socket_and_says_nothing() {
        let file = tempfile::tempfile().unwrap();
        assert_eq!(peer_pid(&file), None);
        assert_eq!(peer_token(&file), None);
    }
}
