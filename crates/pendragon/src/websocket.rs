//! The WebSocket protocol (RFC 6455), server side: the opening handshake on top of
//! Pendragon's HTTP parser, and the framing of messages after it. The screen viewer
//! (`screen.rs`) is its only user: a browser page has no other way to hold a two-way
//! stream open to the server.
//!
//! - **Handshake** ([`Handshake::check`], section 4.2.1): a `GET` over HTTP/1.1 with
//!   `Upgrade: websocket`, `Connection: Upgrade`, a `Sec-WebSocket-Key` that is the
//!   base64 of 16 bytes and `Sec-WebSocket-Version: 13`; a request for another version
//!   gets 426 with the version this server speaks (section 4.4). The answer
//!   ([`accept_response`]) is `101 Switching Protocols` with `Sec-WebSocket-Accept`,
//!   the base64 of the SHA-1 of the key and the protocol's GUID, which tells the
//!   browser the server read the handshake rather than echoed it. Extensions
//!   (`permessage-deflate`, which browsers offer) are not taken: the reply names none,
//!   so none is in use (section 9.1).
//! - **Frames** (section 5): every frame from the client must be masked (section 5.1;
//!   an unmasked one fails the connection), the reserved bits must be clear (no
//!   extension gave them a meaning), control frames are at most 125 bytes and not
//!   fragmented (section 5.5), a text message must be UTF-8 (section 8.1) and a
//!   message is at most [`Reader::new`]'s limit, its length judged from the frame
//!   header before the payload arrives (1009). Frames from the server are never
//!   masked (section 5.1) and never fragmented.
//! - **Closing** (section 7): a close frame is answered with one carrying the same
//!   status code, then the TCP connection is closed. A violation of the rules above
//!   is a [`Violation`]: the code and reason to close with.

use crate::base64;
use crate::http::Request;
use crate::sha1;
use std::io::{self, Write};

/// The GUID RFC 6455 section 1.3 appends to the client's key before hashing it.
pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The only protocol version (section 4.1, `Sec-WebSocket-Version`).
pub const VERSION: &str = "13";

// Opcodes (section 5.2).
pub const OP_CONTINUATION: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xa;

// Status codes of close frames (section 7.4.1).
/// The purpose of the connection is fulfilled (the VM ended, the page left).
pub const CLOSE_NORMAL: u16 = 1000;
/// The server is going away (shutting down), or the peer stopped answering.
pub const CLOSE_GOING_AWAY: u16 = 1001;
/// A frame broke the protocol.
pub const CLOSE_PROTOCOL_ERROR: u16 = 1002;
/// A kind of message the endpoint does not take (binary where it takes text).
pub const CLOSE_UNSUPPORTED: u16 = 1003;
/// A text message that is not UTF-8.
pub const CLOSE_INVALID_DATA: u16 = 1007;
/// A message that breaks the application's rules (`screen.rs`'s messages).
pub const CLOSE_POLICY: u16 = 1008;
/// A message larger than the limit.
pub const CLOSE_TOO_BIG: u16 = 1009;
/// The server cannot go on (Avalon went away, a broken frame from it).
pub const CLOSE_INTERNAL_ERROR: u16 = 1011;

/// What a valid opening handshake asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// `Sec-WebSocket-Key`, as sent.
    pub key: String,
    /// The subprotocols the client offers (`Sec-WebSocket-Protocol`), in order.
    pub protocols: Vec<String>,
}

/// A handshake refused: the HTTP status, why, and headers the answer must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub message: String,
    pub headers: Vec<(&'static str, &'static str)>,
}

impl Refused {
    fn new(status: u16, message: impl Into<String>) -> Refused {
        Refused {
            status,
            message: message.into(),
            headers: Vec::new(),
        }
    }
}

/// Whether the comma-separated header `name` lists `token`, compared without regard
/// to case (RFC 9110 section 5.6.1: `Connection: keep-alive, Upgrade`).
pub fn lists(req: &Request, name: &str, token: &str) -> bool {
    req.headers
        .iter()
        .filter(|(n, _)| n == name)
        .flat_map(|(_, v)| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// Whether `req` asks to become a WebSocket (it says `Upgrade: websocket`), right or
/// wrong in the rest: [`Handshake::check`] judges it.
pub fn is_upgrade(req: &Request) -> bool {
    lists(req, "upgrade", "websocket")
}

impl Handshake {
    /// Checks an opening handshake (RFC 6455 section 4.2.1), its head read. `GET` is
    /// left to the caller, which answers other methods with its own 405.
    pub fn check(req: &Request) -> Result<Handshake, Refused> {
        let http11 = req.version == "HTTP/1.1";
        if !is_upgrade(req) {
            let mut refused = Refused::new(426, "this is a WebSocket: it needs Upgrade: websocket");
            refused.headers.push(("Upgrade", "websocket"));
            return Err(refused);
        }
        if !http11 {
            return Err(Refused::new(400, "a WebSocket handshake is HTTP/1.1"));
        }
        if !lists(req, "connection", "upgrade") {
            return Err(Refused::new(
                400,
                "a WebSocket handshake needs Connection: Upgrade",
            ));
        }
        if req.has_body() {
            return Err(Refused::new(400, "a WebSocket handshake has no body"));
        }
        if req.header_count("sec-websocket-version") != 1
            || req.header("sec-websocket-version") != Some(VERSION)
        {
            // Section 4.4: say which version this server speaks.
            let mut refused = Refused::new(426, "this server speaks WebSocket version 13");
            refused.headers.push(("Sec-WebSocket-Version", VERSION));
            return Err(refused);
        }
        let key = match (
            req.header_count("sec-websocket-key"),
            req.header("sec-websocket-key"),
        ) {
            (1, Some(key)) if base64::decode(key).is_ok_and(|k| k.len() == 16) => key.to_string(),
            _ => {
                return Err(Refused::new(
                    400,
                    "Sec-WebSocket-Key must be the base64 of 16 bytes, once",
                ))
            }
        };
        let protocols = req
            .headers
            .iter()
            .filter(|(n, _)| n == "sec-websocket-protocol")
            .flat_map(|(_, v)| v.split(','))
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        Ok(Handshake { key, protocols })
    }
}

/// `Sec-WebSocket-Accept` for a client's `key` (section 4.2.2, item 5.4).
pub fn accept_key(key: &str) -> String {
    base64::encode(&sha1::digest(format!("{key}{GUID}").as_bytes()))
}

/// The `101 Switching Protocols` that completes a handshake, choosing `protocol` (one
/// of those the client offered) if there is one. A 1xx response has no body, so no
/// `Content-Length`, and the connection stays open.
pub fn accept_response(key: &str, protocol: Option<&str>) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n",
        accept_key(key)
    );
    if let Some(protocol) = protocol {
        out += &format!("Sec-WebSocket-Protocol: {protocol}\r\n");
    }
    out += "\r\n";
    out.into_bytes()
}

/// Writes one unfragmented, unmasked frame (section 5.2): FIN set, `opcode`, the
/// payload's length in the shortest form that holds it, the payload.
pub fn write_frame<W: Write>(w: &mut W, opcode: u8, payload: &[u8]) -> io::Result<()> {
    w.write_all(&frame_header(opcode, payload.len()))?;
    w.write_all(payload)?;
    w.flush()
}

/// The header of a frame from the server (FIN set, no mask) with `len` bytes of
/// payload.
pub fn frame_header(opcode: u8, len: usize) -> Vec<u8> {
    let mut header = vec![0x80 | opcode];
    match len {
        0..=125 => header.push(len as u8),
        126..=0xffff => {
            header.push(126);
            header.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            header.push(127);
            header.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    header
}

/// The payload of a close frame: the status code, big-endian, and a reason in UTF-8,
/// cut to fit the 125 bytes of a control frame (at a character boundary).
pub fn close_payload(code: u16, reason: &str) -> Vec<u8> {
    let mut end = reason.len().min(123);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let mut payload = code.to_be_bytes().to_vec();
    payload.extend_from_slice(&reason.as_bytes()[..end]);
    payload
}

/// A message from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Text(String),
    Binary(Vec<u8>),
    /// A ping: answer with a pong carrying the same bytes (section 5.5.2).
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// A close frame: its status code (none if the frame had no payload) and reason.
    Close(Option<u16>, String),
}

/// The client broke the protocol: close the connection with this code and reason
/// (section 7.1.7, "Fail the WebSocket Connection").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub code: u16,
    pub reason: String,
}

fn violation(code: u16, reason: impl Into<String>) -> Violation {
    Violation {
        code,
        reason: reason.into(),
    }
}

/// Whether a close frame may carry status `code` (section 7.4): the codes the RFC
/// defines for an endpoint to send, and 3000 to 4999 for libraries and applications.
/// 1004, 1005, 1006 and 1015 are reserved or never sent; the rest of 1000 to 2999 is
/// for the RFC and its updates.
fn close_code_ok(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1011 | 3000..=4999)
}

/// Decodes the client's frames into messages, from the bytes as they arrive
/// ([`Reader::push`]), so a read that times out in the middle of a frame loses
/// nothing.
#[derive(Debug)]
pub struct Reader {
    /// Bytes received and not yet decoded.
    buf: Vec<u8>,
    /// A fragmented message being put together: its opcode and payload so far.
    partial: Option<(u8, Vec<u8>)>,
    /// The most payload bytes a message may have.
    max_message: usize,
}

impl Reader {
    /// A reader of messages of at most `max_message` bytes.
    pub fn new(max_message: usize) -> Reader {
        Reader {
            buf: Vec::new(),
            partial: None,
            max_message,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, `None` while it has not all arrived. A violation ends
    /// the connection: the reader is of no more use.
    pub fn message(&mut self) -> Result<Option<Incoming>, Violation> {
        loop {
            let Some((fin, opcode, payload)) = self.frame()? else {
                return Ok(None);
            };
            match opcode {
                OP_CLOSE | OP_PING | OP_PONG => {
                    if !fin {
                        return Err(violation(CLOSE_PROTOCOL_ERROR, "a fragmented control frame"));
                    }
                    return Ok(Some(match opcode {
                        OP_PING => Incoming::Ping(payload),
                        OP_PONG => Incoming::Pong(payload),
                        _ => close_frame(payload)?,
                    }));
                }
                OP_TEXT | OP_BINARY => {
                    if self.partial.is_some() {
                        return Err(violation(
                            CLOSE_PROTOCOL_ERROR,
                            "a new message inside a fragmented one",
                        ));
                    }
                    if fin {
                        return message(opcode, payload).map(Some);
                    }
                    self.partial = Some((opcode, payload));
                }
                OP_CONTINUATION => {
                    let Some((first, mut so_far)) = self.partial.take() else {
                        return Err(violation(CLOSE_PROTOCOL_ERROR, "a continuation of no message"));
                    };
                    so_far.extend_from_slice(&payload);
                    if fin {
                        return message(first, so_far).map(Some);
                    }
                    self.partial = Some((first, so_far));
                }
                _ => {
                    return Err(violation(
                        CLOSE_PROTOCOL_ERROR,
                        format!("unknown opcode {opcode:#x}"),
                    ))
                }
            }
        }
    }

    /// The next whole frame, unmasked: FIN, opcode and payload.
    fn frame(&mut self) -> Result<Option<(bool, u8, Vec<u8>)>, Violation> {
        let b = &self.buf;
        if b.len() < 2 {
            return Ok(None);
        }
        let (fin, rsv, opcode) = (b[0] & 0x80 != 0, b[0] & 0x70, b[0] & 0x0f);
        let masked = b[1] & 0x80 != 0;
        if rsv != 0 {
            return Err(violation(
                CLOSE_PROTOCOL_ERROR,
                "reserved bits set (no extension was agreed)",
            ));
        }
        if !masked {
            return Err(violation(
                CLOSE_PROTOCOL_ERROR,
                "an unmasked frame from the client",
            ));
        }
        let (len, mut at) = match b[1] & 0x7f {
            126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as u64, 4),
            127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()), 10),
            126 | 127 => return Ok(None),
            n => (n as u64, 2),
        };
        if opcode & 0x8 != 0 && len > 125 {
            return Err(violation(CLOSE_PROTOCOL_ERROR, "a control frame over 125 bytes"));
        }
        // Judged before the payload arrives, so a client cannot make the server hold
        // more than a message's worth.
        let so_far = self.partial.as_ref().map_or(0, |(_, p)| p.len()) as u64;
        let data = opcode & 0x8 == 0;
        if len > self.max_message as u64 || (data && so_far + len > self.max_message as u64) {
            return Err(violation(
                CLOSE_TOO_BIG,
                format!("a message over {} bytes", self.max_message),
            ));
        }
        let len = len as usize;
        if b.len() < at + 4 + len {
            return Ok(None);
        }
        let mask = [b[at], b[at + 1], b[at + 2], b[at + 3]];
        at += 4;
        let payload: Vec<u8> = b[at..at + len]
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i % 4])
            .collect();
        self.buf.drain(..at + len);
        Ok(Some((fin, opcode, payload)))
    }
}

fn message(opcode: u8, payload: Vec<u8>) -> Result<Incoming, Violation> {
    if opcode == OP_BINARY {
        return Ok(Incoming::Binary(payload));
    }
    String::from_utf8(payload)
        .map(Incoming::Text)
        .map_err(|_| violation(CLOSE_INVALID_DATA, "a text message that is not UTF-8"))
}

fn close_frame(payload: Vec<u8>) -> Result<Incoming, Violation> {
    match payload.len() {
        0 => Ok(Incoming::Close(None, String::new())),
        1 => Err(violation(
            CLOSE_PROTOCOL_ERROR,
            "a close frame with half a status code",
        )),
        _ => {
            let code = u16::from_be_bytes([payload[0], payload[1]]);
            if !close_code_ok(code) {
                return Err(violation(CLOSE_PROTOCOL_ERROR, format!("close status {code}")));
            }
            let reason = String::from_utf8(payload[2..].to_vec())
                .map_err(|_| violation(CLOSE_INVALID_DATA, "a close reason that is not UTF-8"))?;
            Ok(Incoming::Close(Some(code), reason))
        }
    }
}

/// A frame as a client sends it, masked with `mask`: for the tests, here and in
/// `tests/`.
pub fn client_frame(fin: bool, opcode: u8, mask: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut frame = frame_header(opcode, payload.len());
    if !fin {
        frame[0] &= 0x7f;
    }
    frame[1] |= 0x80;
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{read_request_head, Limits};

    const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

    fn request(head: &str) -> Request {
        let text = format!("{head}\r\n\r\n");
        let mut bytes = std::io::Cursor::new(text.into_bytes());
        read_request_head(&mut bytes, &Limits::default(), |_, _| 100)
            .unwrap()
            .request
    }

    const GOOD: &str = "GET /api/vms/1/screen HTTP/1.1\r\nHost: localhost:7402\r\n\
                        Upgrade: websocket\r\nConnection: keep-alive, Upgrade\r\n\
                        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
                        Sec-WebSocket-Protocol: pendragon.screen, pendragon.token.ab";

    /// RFC 6455 section 1.3's example: the key "dGhlIHNhbXBsZSBub25jZQ==" is answered
    /// with "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".
    #[test]
    fn the_rfcs_handshake() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        let hs = Handshake::check(&request(GOOD)).unwrap();
        assert_eq!(hs.key, "dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(hs.protocols, ["pendragon.screen", "pendragon.token.ab"]);
        let reply = String::from_utf8(accept_response(&hs.key, Some("pendragon.screen"))).unwrap();
        assert_eq!(
            reply,
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\nSec-WebSocket-Protocol: \
             pendragon.screen\r\n\r\n"
        );
        // Header names and the tokens in them are not case-sensitive.
        let shouty = GOOD
            .replace("Upgrade: websocket", "UPGRADE: WebSocket")
            .replace("Upgrade\r", "upgrade\r");
        assert!(Handshake::check(&request(&shouty)).is_ok());
    }

    #[test]
    fn bad_handshakes() {
        let without = |line: &str| GOOD.replace(&format!("\r\n{line}"), "");
        let with = |from: &str, to: &str| GOOD.replace(from, to);
        for (head, status, headers) in [
            (without("Upgrade: websocket"), 426, vec![("Upgrade", "websocket")]),
            (with("HTTP/1.1\r\nHost", "HTTP/1.0\r\nHost"), 400, vec![]),
            (with("keep-alive, Upgrade", "keep-alive"), 400, vec![]),
            (
                without("Sec-WebSocket-Version: 13"),
                426,
                vec![("Sec-WebSocket-Version", "13")],
            ),
            (
                with("Version: 13", "Version: 8"),
                426,
                vec![("Sec-WebSocket-Version", "13")],
            ),
            (
                without("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ=="),
                400,
                vec![],
            ),
            // The base64 of 15 bytes, and text that is not base64.
            (
                with("dGhlIHNhbXBsZSBub25jZQ==", "dGhlIHNhbXBsZSBub25j"),
                400,
                vec![],
            ),
            (with("dGhlIHNhbXBsZSBub25jZQ==", "not a key"), 400, vec![]),
            (
                format!("{GOOD}\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ=="),
                400,
                vec![],
            ),
            (format!("{GOOD}\r\nContent-Length: 3"), 400, vec![]),
        ] {
            let refused = Handshake::check(&request(&head)).unwrap_err();
            assert_eq!((refused.status, refused.headers), (status, headers), "{head}");
        }
    }

    fn read_all(reader: &mut Reader) -> Result<Vec<Incoming>, Violation> {
        let mut out = Vec::new();
        while let Some(m) = reader.message()? {
            out.push(m);
        }
        Ok(out)
    }

    /// Messages of each kind and length form, fragmented ones, and bytes that arrive
    /// one at a time.
    #[test]
    fn messages() {
        let long = vec![b'x'; 300];
        let longer = vec![7u8; 70_000];
        let mut bytes = client_frame(true, OP_TEXT, MASK, b"Hello");
        bytes.extend(client_frame(true, OP_BINARY, MASK, &long));
        bytes.extend(client_frame(true, OP_BINARY, MASK, &longer));
        bytes.extend(client_frame(false, OP_TEXT, MASK, b"Hel"));
        bytes.extend(client_frame(true, OP_PING, MASK, b"in between"));
        bytes.extend(client_frame(false, OP_CONTINUATION, MASK, b"l"));
        bytes.extend(client_frame(true, OP_CONTINUATION, MASK, b"o"));
        bytes.extend(client_frame(true, OP_PONG, MASK, b""));
        bytes.extend(client_frame(true, OP_CLOSE, MASK, &close_payload(1000, "bye")));
        bytes.extend(client_frame(true, OP_CLOSE, MASK, b""));
        let want = vec![
            Incoming::Text("Hello".into()),
            Incoming::Binary(long),
            Incoming::Binary(longer),
            Incoming::Ping(b"in between".to_vec()),
            Incoming::Text("Hello".into()),
            Incoming::Pong(vec![]),
            Incoming::Close(Some(1000), "bye".into()),
            Incoming::Close(None, String::new()),
        ];
        let mut whole = Reader::new(100_000);
        whole.push(&bytes);
        assert_eq!(read_all(&mut whole).unwrap(), want);

        let mut trickle = Reader::new(100_000);
        let mut got = Vec::new();
        for b in &bytes {
            trickle.push(&[*b]);
            got.extend(read_all(&mut trickle).unwrap());
        }
        assert_eq!(got, want);
    }

    /// RFC 6455 section 5.7's examples. The client's frames are masked: the masked
    /// "Hello" and the masked pong as the RFC gives them, and its fragmented "Hello"
    /// and its ping, which it gives unmasked, masked with the same key (each frame's
    /// masking starts afresh, section 5.3); unmasked, they are refused (section 5.1).
    /// The server's frames are not: its unmasked "Hello", ping and lengths.
    #[test]
    fn the_rfcs_frames() {
        const HELLO_MASKED: [u8; 9] = [0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
        let mut r = Reader::new(1000);
        r.push(&[[0x81, 0x85].as_slice(), &HELLO_MASKED].concat());
        assert_eq!(r.message().unwrap(), Some(Incoming::Text("Hello".into())));
        r.push(&[[0x8a, 0x85].as_slice(), &HELLO_MASKED].concat());
        assert_eq!(r.message().unwrap(), Some(Incoming::Pong(b"Hello".to_vec())));
        r.push(&[[0x89, 0x85].as_slice(), &HELLO_MASKED].concat());
        assert_eq!(r.message().unwrap(), Some(Incoming::Ping(b"Hello".to_vec())));
        // "Hel" then "lo", masked with the RFC's key: 'l' ^ 0x37, 'o' ^ 0xfa.
        let fragmented = [
            [0x01, 0x83, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d].as_slice(),
            &[0x80, 0x82, 0x37, 0xfa, 0x21, 0x3d, 0x5b, 0x95],
        ]
        .concat();
        assert_eq!(
            fragmented,
            [
                client_frame(false, OP_TEXT, [0x37, 0xfa, 0x21, 0x3d], b"Hel"),
                client_frame(true, OP_CONTINUATION, [0x37, 0xfa, 0x21, 0x3d], b"lo"),
            ]
            .concat()
        );
        r.push(&fragmented[..9]);
        assert_eq!(r.message().unwrap(), None, "the first fragment alone");
        r.push(&fragmented[9..]);
        assert_eq!(r.message().unwrap(), Some(Incoming::Text("Hello".into())));
        assert_eq!(r.message().unwrap(), None);
        // The RFC's unmasked fragmented "Hello" and ping, as a client must not send them.
        for unmasked in [
            [0x01, 0x03, 0x48, 0x65, 0x6c, 0x80, 0x02, 0x6c, 0x6f].as_slice(),
            &[0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f],
        ] {
            let mut r = Reader::new(1000);
            r.push(unmasked);
            assert_eq!(r.message().unwrap_err().code, CLOSE_PROTOCOL_ERROR);
        }
        // An unmasked "Hello" and ping are how the server writes them.
        let mut out = Vec::new();
        write_frame(&mut out, OP_TEXT, b"Hello").unwrap();
        assert_eq!(out, [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]);
        out.clear();
        write_frame(&mut out, OP_PING, b"Hello").unwrap();
        assert_eq!(out, [0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]);
        out.clear();
        write_frame(&mut out, OP_BINARY, &[0; 256]).unwrap();
        assert_eq!(out[..4], [0x82, 0x7e, 0x01, 0x00]);
        out.clear();
        write_frame(&mut out, OP_BINARY, &[0; 65536]).unwrap();
        assert_eq!(out[..10], [0x82, 0x7f, 0, 0, 0, 0, 0, 1, 0, 0]);
    }

    #[test]
    fn violations() {
        let mut long_close = close_payload(1000, "");
        long_close.extend([b'x'; 124]);
        for (bytes, code) in [
            // Unmasked, as a server would send it.
            (
                vec![0x81, 0x05, b'H', b'e', b'l', b'l', b'o'],
                CLOSE_PROTOCOL_ERROR,
            ),
            // RSV1 set (permessage-deflate was not agreed).
            (
                {
                    let mut f = client_frame(true, OP_TEXT, MASK, b"x");
                    f[0] |= 0x40;
                    f
                },
                CLOSE_PROTOCOL_ERROR,
            ),
            (client_frame(true, 0x3, MASK, b""), CLOSE_PROTOCOL_ERROR),
            (client_frame(true, 0xb, MASK, b""), CLOSE_PROTOCOL_ERROR),
            (client_frame(false, OP_PING, MASK, b""), CLOSE_PROTOCOL_ERROR),
            (client_frame(true, OP_PING, MASK, &[0; 126]), CLOSE_PROTOCOL_ERROR),
            (
                client_frame(true, OP_CLOSE, MASK, &long_close),
                CLOSE_PROTOCOL_ERROR,
            ),
            (
                client_frame(true, OP_CONTINUATION, MASK, b"x"),
                CLOSE_PROTOCOL_ERROR,
            ),
            (
                [
                    client_frame(false, OP_TEXT, MASK, b"x"),
                    client_frame(true, OP_TEXT, MASK, b"y"),
                ]
                .concat(),
                CLOSE_PROTOCOL_ERROR,
            ),
            (
                client_frame(true, OP_TEXT, MASK, &[0xff, 0xfe]),
                CLOSE_INVALID_DATA,
            ),
            (client_frame(true, OP_CLOSE, MASK, &[0x03]), CLOSE_PROTOCOL_ERROR),
            (
                client_frame(true, OP_CLOSE, MASK, &close_payload(1005, "")),
                CLOSE_PROTOCOL_ERROR,
            ),
            (
                client_frame(true, OP_CLOSE, MASK, &close_payload(999, "")),
                CLOSE_PROTOCOL_ERROR,
            ),
            (
                client_frame(true, OP_CLOSE, MASK, &[0x03, 0xe8, 0xff]),
                CLOSE_INVALID_DATA,
            ),
            (client_frame(true, OP_BINARY, MASK, &[0; 101]), CLOSE_TOO_BIG),
            (
                [
                    client_frame(false, OP_TEXT, MASK, &[b'a'; 60]),
                    client_frame(true, OP_CONTINUATION, MASK, &[b'a'; 60]),
                ]
                .concat(),
                CLOSE_TOO_BIG,
            ),
        ] {
            let mut r = Reader::new(100);
            r.push(&bytes);
            let got = read_all(&mut r).unwrap_err();
            assert_eq!(got.code, code, "{bytes:02x?}: {}", got.reason);
        }
        // A frame that says it is huge is refused from its header alone.
        let mut r = Reader::new(100);
        r.push(&[0x82, 0xff, 0, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(r.message().unwrap_err().code, CLOSE_TOO_BIG);
        // Every code an endpoint may send is taken.
        for code in [1000, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011, 3000, 4999] {
            let mut r = Reader::new(100);
            r.push(&client_frame(true, OP_CLOSE, MASK, &close_payload(code, "")));
            assert_eq!(
                r.message().unwrap(),
                Some(Incoming::Close(Some(code), String::new()))
            );
        }
    }

    /// A close reason is cut to fit a control frame, at a character boundary.
    #[test]
    fn close_payloads_fit() {
        let p = close_payload(1000, &"é".repeat(100));
        assert_eq!(p.len(), 124);
        assert!(std::str::from_utf8(&p[2..]).is_ok());
        assert_eq!(close_payload(1011, "gone"), [0x03, 0xf3, b'g', b'o', b'n', b'e']);
    }
}
