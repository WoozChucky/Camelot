//! Closing a connection without losing the last thing said on it.
//!
//! A socket closed while bytes from its peer wait unread in its receive buffer does
//! not end with an orderly FIN: the operating system sends a reset instead (Windows
//! and Linux alike, RFC 1122 section 4.2.2.13). A reset can overtake what was sent
//! just before it: the peer's stack throws away whatever it has received and the
//! program has not yet read (Windows does; Linux hands over what it has, then fails
//! the next read with "connection reset"). So a server that answers a bad request with
//! an `Error` and closes at once, while the client's next request (or the rest of an
//! oversize frame, or a stray HTTP client's headers) is still unread, can make the
//! client lose that `Error`.
//!
//! [`close_gracefully`] closes the way HTTP servers do ("lingering close"): it sends
//! FIN after the last frame (`shutdown(Write)`), then reads and drops what the peer
//! still sends until the peer closes its side, for at most [`LINGER`] and
//! [`LINGER_BYTES`] all told, so a peer that keeps sending cannot keep the
//! connection. Then the socket can be dropped with nothing unread, and closes cleanly.

use std::io::{self, Read};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

/// Most time [`close_gracefully`] spends reading what the peer still sends.
pub const LINGER: Duration = Duration::from_secs(1);

/// Most bytes [`close_gracefully`] reads (and drops) from the peer.
pub const LINGER_BYTES: usize = 4 * 1024 * 1024;

/// How often a lingering read wakes to ask `stop` whether to give up.
const LINGER_POLL: Duration = Duration::from_millis(100);

/// Ends a connection after its last frame: says no more is coming, then reads and
/// drops the peer's input until it closes its side, [`LINGER`] has passed,
/// [`LINGER_BYTES`] have arrived, a read fails, or `stop` says to give up (a server
/// shutting down, which also shuts the socket down from another thread; `|| false`
/// when there is no such thing). The caller then drops the socket (all its handles).
///
/// Everything written before this call reaches the peer ahead of the FIN. If the peer
/// is still sending when the time or the bytes run out, closing resets the connection
/// after all: a bounded wait is the price of a server that cannot be held open.
pub fn close_gracefully(stream: &TcpStream, stop: impl Fn() -> bool) {
    if stream.shutdown(Shutdown::Write).is_err() {
        // Not connected any more: nothing to send a FIN on, nothing to wait for.
        return;
    }
    let deadline = Instant::now() + LINGER;
    let mut sink = [0u8; 8192];
    let mut drained = 0;
    while drained < LINGER_BYTES && !stop() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        // A zero timeout means "wait forever" to set_read_timeout.
        let wait = left.min(LINGER_POLL).max(Duration::from_millis(1));
        if stream.set_read_timeout(Some(wait)).is_err() {
            break;
        }
        match (&mut &*stream).read(&mut sink) {
            Ok(0) => break,
            Ok(n) => drained += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    /// Everything the server sent, then a clean end, though the server never read
    /// what the client had sent it.
    fn read_to_clean_end(mut s: &TcpStream) -> Vec<u8> {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).expect("a clean end, not a reset");
        got
    }

    /// Why: dropping a socket with input unread resets the connection, and the reply
    /// written just before is lost (Windows) or followed by a reset (Linux).
    #[test]
    fn closing_at_once_with_input_unread_resets() {
        let (client, server) = pair();
        (&client).write_all(b"unread").unwrap();
        // It has arrived, and is still unread.
        server.peek(&mut [0u8; 1]).unwrap();
        (&server).write_all(b"error").unwrap();
        drop(server);
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let e = (&client).read_to_end(&mut Vec::new()).unwrap_err();
        assert!(
            matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ),
            "{e}"
        );
    }

    #[test]
    fn the_last_reply_arrives_though_input_was_unread() {
        let (client, server) = pair();
        (&client)
            .write_all(b"request, then more the server never reads")
            .unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        (&server).write_all(b"error").unwrap();
        let start = Instant::now();
        close_gracefully(&server, || false);
        assert!(
            start.elapsed() < LINGER,
            "ended by the client's FIN, not the time"
        );
        drop(server);
        assert_eq!(read_to_clean_end(&client), b"error");
    }

    #[test]
    fn a_peer_that_never_closes_is_waited_for_only_so_long() {
        let (client, server) = pair();
        (&client).write_all(b"unread").unwrap();
        let start = Instant::now();
        close_gracefully(&server, || false);
        let took = start.elapsed();
        assert!(took >= LINGER - Duration::from_millis(50), "{took:?}");
        assert!(took < LINGER * 5, "{took:?}");
        drop(server);
        // The unread bytes were read, so this end is clean too.
        assert_eq!(read_to_clean_end(&client), b"");
    }

    #[test]
    fn stop_ends_the_wait() {
        let (client, server) = pair();
        let start = Instant::now();
        close_gracefully(&server, || true);
        assert!(start.elapsed() < LINGER);
        drop(client);
    }
}
