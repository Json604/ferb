//! Whether this Mac has a network route at all (`ferb/sense/network.py`): a fact for telling the owner why a model
//! service couldn't be reached. A UDP socket's connect() only asks the kernel's routing table: no packet leaves.

use std::io;
use std::net::{SocketAddr, UdpSocket};

/// network.py:12: public resolver addresses, used only as route-lookup targets (nothing is sent to them).
pub const TARGETS: [&str; 2] = ["1.1.1.1:53", "[2606:4700:4700::1111]:53"];

fn no_route(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENETUNREACH | libc::EHOSTUNREACH | libc::EADDRNOTAVAIL | libc::ENETDOWN)
    )
}

/// What one family's lookup did.
pub enum Lookup {
    /// The socket couldn't be made.
    NoSocket,
    /// connect() answered.
    Connected(io::Result<()>),
}

/// network.py:16 `_route` over a lookup per target: Some(true) some route exists, Some(false) no route for either
/// family, None the lookup said nothing about routes.
pub fn route(lookup: &dyn Fn(&str) -> Lookup) -> Option<bool> {
    let mut unknown = false;
    for target in TARGETS {
        match lookup(target) {
            Lookup::NoSocket => unknown = true,
            Lookup::Connected(Ok(())) => return Some(true),
            Lookup::Connected(Err(e)) => unknown = unknown || !no_route(&e),
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}

fn system_lookup(target: &str) -> Lookup {
    let Ok(addr) = target.parse::<SocketAddr>() else {
        return Lookup::NoSocket;
    };
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    match UdpSocket::bind(bind) {
        Err(_) => Lookup::NoSocket,
        Ok(s) => Lookup::Connected(s.connect(addr)),
    }
}

/// network.py:35.
pub fn has_route() -> Option<bool> {
    route(&system_lookup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(code: i32) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(code))
    }

    #[test]
    fn a_route_is_told_from_no_route_and_from_not_knowing() {
        assert_eq!(route(&|_| Lookup::Connected(Ok(()))), Some(true));
        assert_eq!(
            route(&|_| Lookup::Connected(err(libc::ENETUNREACH))),
            Some(false)
        );
        assert_eq!(
            route(&|t| if t.starts_with('[') {
                Lookup::Connected(Ok(()))
            } else {
                Lookup::Connected(err(libc::EHOSTUNREACH))
            }),
            Some(true)
        );
        assert_eq!(route(&|_| Lookup::Connected(err(libc::EACCES))), None);
        assert_eq!(route(&|_| Lookup::NoSocket), None);
        assert_eq!(
            route(&|t| if t.starts_with('[') {
                Lookup::NoSocket
            } else {
                Lookup::Connected(err(libc::ENETDOWN))
            }),
            None
        );
    }
}
