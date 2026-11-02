//! The Avalon protocol (`docs/avalon-protocol.md`): what the Avalon hypervisor, the
//! Round Table control CLI, the Herald VM client and the Lake storage server say to each
//! other over TCP.
//!
//! Three layers, each usable on its own:
//!
//! - [`frame`]: length-prefixed binary frames (`u32` length, `u16` message type,
//!   payload) read from and written to any byte stream, with a maximum frame size.
//! - [`message`]: the message catalogue as Rust types, and [`encode`]/[`decode`]
//!   between a [`Message`] and the bytes of one frame.
//! - [`Connection`]: a blocking wrapper over any `Read + Write` (a `TcpStream`, or an
//!   in-memory pipe in tests) that sends and receives messages, does the version
//!   handshake and matches requests to replies.
//!
//! Plus [`Screen`], a VM's screen as a server and its viewers keep it, with the change
//! detection that turns a framebuffer into `ScreenFrame` rectangles; and what every
//! program's command line shares: [`parse_listen`] for a server's `--listen`, and
//! [`DiskSource::parse`] for a disk argument (a path or `lake://addr/name`).
//!
//! Everything a peer sends is checked: a truncated, oversize or otherwise malformed
//! frame is an [`Error`], never a panic or an unbounded allocation.

use std::fmt;
use std::io;

mod addr;
pub mod conn;
pub mod frame;
pub mod message;
pub mod screen;
mod wire;

pub use addr::{default_port, parse_listen, LAKE_SCHEME};
pub use conn::Connection;
pub use frame::{decode, encode, read_frame, write_frame, Frame};
pub use message::*;
pub use screen::{Rect, Screen};

/// Protocol version carried by the handshake. Both sides must speak the same one; any
/// change to the message catalogue (a new message, field or enum value) bumps it.
pub const PROTOCOL_VERSION: u16 = 3;

/// First four payload bytes of `Hello`, so a server can tell an Avalon client from a
/// stray connection.
pub const MAGIC: [u8; 4] = *b"AVLN";

/// Bytes before a frame's payload: the `u32` length and the `u16` message type.
pub const HEADER_LEN: usize = 6;

/// Largest allowed value of a frame's length field (type + payload), 16 MiB. A full
/// 640x480 XRGB8888 screen is 1.2 MB and a block request at most 1 MiB, so this leaves
/// room for program images while bounding what one frame can make a peer allocate.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Disk sector size for block I/O.
pub const SECTOR_SIZE: usize = 512;

/// Most sectors one `BlockRead` or `BlockWrite` may move (1 MiB).
pub const MAX_BLOCK_SECTORS: u32 = 2048;

/// Longest VM or disk name, in bytes. See [`validate_name`].
pub const MAX_NAME_LEN: usize = 64;

/// Where Avalon listens by default. Listening on any other interface takes an explicit
/// flag, since the protocol has no authentication.
pub const DEFAULT_AVALON_ADDR: &str = "127.0.0.1:7400";

/// Where Lake listens by default.
pub const DEFAULT_LAKE_ADDR: &str = "127.0.0.1:7401";

/// Checks a VM or disk name (protocol doc, section 3): 1 to [`MAX_NAME_LEN`] bytes of
/// lowercase ASCII letters, digits, `.`, `_` and `-`; not starting with `.` or `-`, not
/// ending with `.`, not all digits, and not a Windows device name (`con`, `prn`, `aux`,
/// `nul`, `com0`-`com9`, `lpt0`-`lpt9`, alone or before a `.`).
///
/// Disk names become file names on Lake's host, so the rule keeps every name a plain
/// file of its own on Windows as well: no path separators, no device, and no two names
/// that reach the same file (Windows ignores case and trailing dots). All-digit names
/// are left out because a command-line VM argument that is all digits is an id
/// ([`VmRef::parse`]). Servers check names; the decoder does not, so a bad name gets a
/// precise `InvalidName` error instead of `Malformed`.
pub fn validate_name(name: &str) -> Result<(), String> {
    let ok_byte = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-');
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(format!("name must be 1 to {MAX_NAME_LEN} characters long"));
    }
    if name.starts_with(['.', '-']) || name.ends_with('.') || !name.bytes().all(ok_byte) {
        return Err(format!(
            "'{name}' is not a valid name (lowercase letters, digits, '.', '_' and '-', \
             not starting with '.' or '-', not ending with '.')"
        ));
    }
    if name.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "'{name}' is not a valid name (all digits would read as a VM id)"
        ));
    }
    let stem = name.split('.').next().unwrap_or(name);
    let device = match stem.as_bytes() {
        b"con" | b"prn" | b"aux" | b"nul" => true,
        [b'c', b'o', b'm', d] | [b'l', b'p', b't', d] => d.is_ascii_digit(),
        _ => false,
    };
    if device {
        return Err(format!(
            "'{name}' is not a valid name ('{stem}' is a Windows device)"
        ));
    }
    Ok(())
}

/// Everything that can go wrong reading, writing or decoding Avalon frames, plus the
/// errors a peer reports back ([`Error::Remote`]).
#[derive(Debug)]
pub enum Error {
    /// The transport failed. The peer closing the connection in the middle of a frame
    /// is an `UnexpectedEof` here.
    Io(io::Error),
    /// The peer closed the connection cleanly, between two frames.
    Closed,
    /// A length field above [`MAX_FRAME_LEN`] (when receiving), or a message too big
    /// to fit in one frame (when sending).
    FrameTooLarge(u64),
    /// A length field below 2, so there is no room for the message type.
    FrameTooShort(u32),
    /// The payload ended before the message did.
    Truncated,
    /// The payload has this many bytes left over after the message.
    TrailingBytes(usize),
    /// A message type that is not in the catalogue.
    UnknownType(u16),
    /// A field holds a value the protocol does not allow (bad enum tag, bool other than
    /// 0/1, invalid UTF-8, a block of the wrong size, ...).
    Invalid(String),
    /// `Hello` did not start with [`MAGIC`].
    BadMagic([u8; 4]),
    /// The handshake found different protocol versions.
    VersionMismatch { ours: u16, theirs: u16 },
    /// The peer answered a request with an `Error` message.
    Remote { code: ErrorCode, message: String },
    /// The peer sent a well-formed message that is not allowed here (anything but
    /// `Hello` before the handshake, a reply of the wrong type, ...). Carries the
    /// message type.
    Unexpected(u16),
    /// [`Connection::request`] read a frame, while waiting for a reply, that it cannot
    /// tell apart from the reply (a request, an unknown type, a too-short frame).
    /// Replies match requests only by order, so this connection can no longer pair
    /// them and must be closed.
    OutOfStep(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Whether the connection is unusable after this error. After the other errors the
    /// whole frame has been consumed and accounted for, so the stream is still in step:
    /// a server can reply with an `Error` message (unless the frame was an event) and
    /// carry on, and a client's next request still gets its own reply.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Error::Io(_)
                | Error::Closed
                | Error::FrameTooLarge(_)
                | Error::VersionMismatch { .. }
                | Error::OutOfStep(_)
        )
    }

    /// The error code to report to the peer for this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            Error::FrameTooLarge(_)
            | Error::FrameTooShort(_)
            | Error::Truncated
            | Error::TrailingBytes(_)
            | Error::Invalid(_)
            | Error::BadMagic(_) => ErrorCode::Malformed,
            Error::UnknownType(_) => ErrorCode::UnknownMessage,
            Error::Unexpected(_) | Error::OutOfStep(_) => ErrorCode::Unexpected,
            Error::Remote { code, .. } => *code,
            Error::VersionMismatch { .. } | Error::Io(_) | Error::Closed => ErrorCode::Internal,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Closed => write!(f, "connection closed"),
            Error::FrameTooLarge(n) => {
                write!(
                    f,
                    "frame of {n} bytes is larger than the maximum ({MAX_FRAME_LEN})"
                )
            }
            Error::FrameTooShort(n) => write!(f, "frame length {n} is too short for a message type"),
            Error::Truncated => write!(f, "message is truncated"),
            Error::TrailingBytes(n) => write!(f, "message has {n} trailing bytes"),
            Error::UnknownType(t) => write!(f, "unknown message type {t:#06x}"),
            Error::Invalid(why) => write!(f, "invalid message: {why}"),
            Error::BadMagic(m) => write!(f, "not an Avalon client (magic {m:02x?})"),
            Error::VersionMismatch { ours, theirs } => write!(
                f,
                "protocol version mismatch: we speak version {ours}, the peer speaks {theirs}"
            ),
            Error::Remote { code, message } => write!(f, "{code}: {message}"),
            Error::Unexpected(t) => write!(f, "unexpected message {}", message::type_name(*t)),
            Error::OutOfStep(why) => write!(f, "replies out of step with requests: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}
