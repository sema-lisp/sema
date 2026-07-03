//! Networking helpers shared across the servers Sema ships (`http/serve`, the
//! notebook server, the web dev server).
//!
//! `sema-core` is intentionally tokio-free, so this binds a synchronous
//! [`std::net::TcpListener`]. Async callers convert the result with
//! `tokio::net::TcpListener::from_std` after `set_nonblocking(true)`.

use std::io;
use std::net::TcpListener;

/// Bind a TCP listener to `host:start_port`, advancing to the next port on
/// `AddrInUse` up to `max_tries` attempts. Returns the bound listener together
/// with the port actually used.
///
/// Only [`io::ErrorKind::AddrInUse`] triggers a retry — any other error (an
/// unresolvable host, permission denied) fails fast rather than pointlessly
/// walking the whole range. If every attempted port is taken, the last
/// `AddrInUse` error is returned. `max_tries` is clamped to at least one, and
/// the walk stops early if the port would overflow past 65535.
///
/// # Examples
///
/// ```
/// // Occupy an OS-assigned port, then confirm the fallback skips past it.
/// let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
/// let taken = occupied.local_addr().unwrap().port();
/// let (_listener, port) =
///     sema_core::net::bind_with_fallback("127.0.0.1", taken, 50).unwrap();
/// assert_ne!(port, taken);
/// ```
pub fn bind_with_fallback(
    host: &str,
    start_port: u16,
    max_tries: u16,
) -> io::Result<(TcpListener, u16)> {
    let mut last_err = None;
    for offset in 0..max_tries.max(1) {
        let Some(port) = start_port.checked_add(offset) else {
            break; // walked past 65535 — nothing left to try
        };
        match TcpListener::bind((host, port)) {
            Ok(listener) => return Ok((listener, port)),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrInUse,
            "no available port found in the requested range",
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_when_start_port_is_taken() {
        // Hold an OS-assigned port open, then start the search exactly there.
        let occupier = TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = occupier.local_addr().unwrap().port();

        let (_listener, got) = bind_with_fallback("127.0.0.1", taken, 50)
            .expect("a free port should exist just above the occupied one");

        assert_ne!(got, taken, "must not hand back the occupied port");
        assert!(got > taken, "fallback advances upward from the start port");
    }

    #[test]
    fn binds_at_or_above_start_port() {
        // Find a free port, release it, then bind from there. Fallback only
        // advances upward, so the result must be >= the start (used the free
        // start, or advanced if a parallel test grabbed it in between — asserting
        // the exact port would be racy).
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let (_l, got) =
            bind_with_fallback("127.0.0.1", port, 50).expect("a free port should be bindable");
        assert!(got >= port, "fallback never binds below the start port");
    }

    #[test]
    fn fails_fast_on_non_addrinuse_error() {
        // An unresolvable host is not AddrInUse, so it must error immediately
        // instead of walking the whole range.
        let result = bind_with_fallback("no-such-host.invalid.", 12345, 100);
        assert!(
            result.is_err(),
            "unresolvable host should fail, not fall back"
        );
    }
}
