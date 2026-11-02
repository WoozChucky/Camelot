//! A disk on a Lake server: a [`BlockBackend`] that keeps one Avalon protocol
//! connection to Lake (`docs/avalon-protocol.md`, section 6.5) and turns every read,
//! write and flush of the guest's disk into block requests on it.
//!
//! The disk attaches its image when it is opened (`BlockOpen`) and detaches it when it
//! is dropped (`BlockClose`, and waits for the answer), so once the machine that owns
//! it is gone, Lake has released the image and another machine can attach it at once.
//! Lake lets an image have one writer or any number of read-only attachments; opening
//! an image someone else is writing fails.
//!
//! Requests are synchronous, like every Grail disk: the guest's `BLK_CMD` store waits
//! for Lake's answer. A transfer bigger than one request may carry
//! ([`MAX_BLOCK_SECTORS`]) is split into several. Every failure is an `io::Error` for
//! the device to report as `BLK_ERROR` 5: Lake's error replies (the disk stays usable),
//! and a broken connection or a server that does not answer within [`IO_TIMEOUT`]
//! (then every later request fails too: the connection can no longer be trusted to
//! pair replies with requests).

use crate::block::{byte_range, BlockBackend, SECTOR_SIZE};
use avalon_proto::{Connection, Error, ErrorCode, Message, MAX_BLOCK_SECTORS};
use std::fmt;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// How long opening a disk waits for each address of the server to accept.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a request waits for Lake's answer before the disk gives up on the server.
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// What the disk calls itself in its `Hello`.
const AGENT: &str = concat!("grail ", env!("CARGO_PKG_VERSION"));

/// An image on a Lake server, attached for the life of this value.
pub struct LakeDisk {
    conn: Connection<TcpStream>,
    /// `lake://addr/name`, for messages.
    url: String,
    handle: u32,
    sectors: u64,
    read_only: bool,
    /// Set once the connection failed: why.
    broken: Option<String>,
}

impl fmt::Debug for LakeDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LakeDisk")
            .field("url", &self.url)
            .field("handle", &self.handle)
            .field("sectors", &self.sectors)
            .field("read_only", &self.read_only)
            .field("broken", &self.broken)
            .finish()
    }
}

impl LakeDisk {
    /// Connects to the Lake server at `addr` (`host:port`) and attaches image `name`,
    /// read-write or read-only. Errors are Lake's (no such disk, a writer already
    /// attached, ...) or the connection's, as text.
    pub fn open(addr: &str, name: &str, read_only: bool) -> Result<LakeDisk, String> {
        let url = format!("{}{addr}/{name}", avalon_proto::LAKE_SCHEME);
        let fail = |e: &dyn fmt::Display| format!("{url}: {e}");
        let stream = connect(addr).map_err(|e| fail(&e))?;
        let mut conn = Connection::new(stream);
        conn.client_handshake(AGENT).map_err(|e| fail(&e))?;
        let reply = conn
            .request(&Message::BlockOpen {
                name: name.to_string(),
                read_only,
            })
            .map_err(|e| fail(&e))?;
        match reply {
            Message::BlockOpened {
                handle,
                sectors,
                read_only: opened_ro,
            } if opened_ro == read_only => Ok(LakeDisk {
                conn,
                url,
                handle,
                sectors,
                read_only,
                broken: None,
            }),
            other => Err(fail(&format!("unexpected answer to BlockOpen: {}", other.name()))),
        }
    }

    /// The handle Lake gave this attachment.
    pub fn handle(&self) -> u32 {
        self.handle
    }

    /// Why the connection failed, once it has: every request fails from then on.
    pub fn broken(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    /// Changes how long each later request waits for Lake's answer ([`IO_TIMEOUT`] by
    /// default). Tests use it to time a request out quickly.
    pub fn set_io_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        let stream = self.conn.get_ref();
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))
    }

    /// Sends one request and returns its reply, as an `io::Error` if it failed.
    fn request(&mut self, msg: &Message) -> io::Result<Message> {
        if let Some(why) = &self.broken {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                format!("{}: {why}", self.url),
            ));
        }
        match self.conn.request(msg) {
            Ok(reply) => Ok(reply),
            Err(Error::Remote { code, message }) => Err(io::Error::new(
                match code {
                    ErrorCode::OutOfRange => io::ErrorKind::InvalidInput,
                    ErrorCode::ReadOnly => io::ErrorKind::PermissionDenied,
                    _ => io::ErrorKind::Other,
                },
                format!("{}: {code}: {message}", self.url),
            )),
            Err(e) => {
                // A reply that never came (a timeout), a closed socket, a reply out of
                // step: the stream can no longer be trusted. After a timeout the reply
                // may still arrive, and the next request would take it for its own
                // (a late `Ok` for an earlier write would pass for a newer write's).
                let why = e.to_string();
                if e.is_fatal() || matches!(e, Error::Unexpected(_)) {
                    self.broken = Some(why.clone());
                }
                Err(io::Error::other(format!("{}: {why}", self.url)))
            }
        }
    }

    fn unexpected(&mut self, request: &str, reply: &Message) -> io::Error {
        // A well-formed reply that does not answer the request: Lake and this disk
        // disagree about something, so stop using the connection.
        let why = format!("unexpected answer to {request}: {}", reply.name());
        self.broken = Some(why.clone());
        io::Error::other(format!("{}: {why}", self.url))
    }
}

/// Connects to `addr`, trying each address it resolves to, with [`CONNECT_TIMEOUT`]
/// each, and sets the socket up for requests.
fn connect(addr: &str) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::InvalidInput, "the address resolves to nothing");
    for a in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(stream) => {
                stream.set_nodelay(true)?;
                stream.set_read_timeout(Some(IO_TIMEOUT))?;
                stream.set_write_timeout(Some(IO_TIMEOUT))?;
                return Ok(stream);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

impl BlockBackend for LakeDisk {
    fn capacity(&self) -> u64 {
        self.sectors
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn read(&mut self, sector: u64, buf: &mut [u8]) -> io::Result<()> {
        byte_range(self.sectors, sector, buf.len())?;
        let chunk = MAX_BLOCK_SECTORS as usize * SECTOR_SIZE;
        for (i, part) in buf.chunks_mut(chunk).enumerate() {
            let at = sector + (i * MAX_BLOCK_SECTORS as usize) as u64;
            let count = (part.len() / SECTOR_SIZE) as u32;
            let request = Message::BlockRead {
                handle: self.handle,
                sector: at,
                count,
            };
            match self.request(&request)? {
                Message::BlockData { handle, sector, data }
                    if handle == self.handle && sector == at && data.len() == part.len() =>
                {
                    part.copy_from_slice(&data)
                }
                other => return Err(self.unexpected("BlockRead", &other)),
            }
        }
        Ok(())
    }

    fn write(&mut self, sector: u64, buf: &[u8]) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only disk"));
        }
        byte_range(self.sectors, sector, buf.len())?;
        let chunk = MAX_BLOCK_SECTORS as usize * SECTOR_SIZE;
        for (i, part) in buf.chunks(chunk).enumerate() {
            let request = Message::BlockWrite {
                handle: self.handle,
                sector: sector + (i * MAX_BLOCK_SECTORS as usize) as u64,
                data: part.to_vec(),
            };
            match self.request(&request)? {
                Message::Ok => {}
                other => return Err(self.unexpected("BlockWrite", &other)),
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.request(&Message::BlockFlush { handle: self.handle })? {
            Message::Ok => Ok(()),
            other => Err(self.unexpected("BlockFlush", &other)),
        }
    }
}

impl Drop for LakeDisk {
    /// Detaches the image and waits for Lake to confirm, so the next machine can
    /// attach it as soon as this one is gone. (Closing the connection would release it
    /// too, but Lake may notice that only after the next machine has asked.)
    fn drop(&mut self) {
        if self.broken.is_none() {
            let _ = self.conn.request(&Message::BlockClose { handle: self.handle });
        }
        let _ = self.conn.get_ref().shutdown(std::net::Shutdown::Both);
    }
}
