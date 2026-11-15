//! Who is on the other end of a Unix socket, as POSIX says: the user. Port of `server.py:106-108`
//! (`peer_uid`). The process and its audit token are the platform's to say (`Server::set_peer_lookup`;
//! `ferb-platform-macos` on the Mac).

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
