//! A small, strict HTTP/1.1 server side: one request per connection, read with hard
//! limits on its size and on the time it takes to arrive, and one response.
//!
//! Pendragon serves a browser on the same machine, not the open internet, but any
//! web page that browser opens can make it send requests, so the parser assumes
//! nothing about what arrives:
//!
//! - The request line and headers together are at most [`Limits::max_head`] bytes,
//!   with at most [`Limits::max_headers`] header lines (431 otherwise), and every line
//!   ends in CRLF. A bare LF, a header folded over two lines, whitespace before a
//!   header's colon, control characters, a fragment in the target or a target that is
//!   not a path are refused (400), rather than guessed at: two programs that guess
//!   differently are how request smuggling starts.
//! - The body is `Content-Length` bytes, at most what [`read_request`]'s `body_limit`
//!   allows for that method and path (413, decided before a byte of the body is read).
//!   `Transfer-Encoding` (chunked bodies) is not implemented (501); two
//!   `Content-Length` headers are refused (400).
//! - The whole request must arrive within [`Limits::timeout`] (408), however slowly it
//!   trickles in: [`Deadline`] gives each read only the time that is left.
//!
//! Every response closes the connection (`Connection: close`), so there is no
//! pipelining or keep-alive to get wrong.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// How much a request may be, and how long it may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Request line and headers, CRLFs included.
    pub max_head: usize,
    /// Header lines.
    pub max_headers: usize,
    /// The whole request, from the first byte to the last byte of the body.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_head: 16 * 1024,
            max_headers: 64,
            timeout: Duration::from_secs(30),
        }
    }
}

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET`, `POST`, ... (a valid token, case kept: methods are case-sensitive).
    pub method: String,
    /// `HTTP/1.1` or `HTTP/1.0`.
    pub version: String,
    /// The target's path as sent (still percent-encoded), always starting with `/`.
    pub path: String,
    /// The path's segments, percent-decoded: `/api/vms/3` is `["api", "vms", "3"]`,
    /// `/` is `[""]`. An encoded `/` (`%2F`) stays inside its segment.
    pub segments: Vec<String>,
    /// The query's `name=value` pairs, decoded (`+` is a space), in order.
    pub query: Vec<(String, String)>,
    /// Header names lowercased, values trimmed, in order.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The value of the first header named `name` (lowercase).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// How many headers are named `name` (lowercase).
    pub fn header_count(&self, name: &str) -> usize {
        self.headers.iter().filter(|(n, _)| n == name).count()
    }

    /// Whether the request has a body, or (its head read, the body not yet) says it
    /// will have one.
    pub fn has_body(&self) -> bool {
        !self.body.is_empty()
            || self
                .header("content-length")
                .is_some_and(|l| l.parse::<u64>().map_or(true, |n| n > 0))
    }

    /// The value of the first query parameter named `name`.
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Why a request could not be read. [`HttpError::status`] says what to answer.
#[derive(Debug)]
pub enum HttpError {
    /// The client closed the connection before sending anything: nobody to answer.
    Closed,
    /// The transport failed: nobody to answer either.
    Io(io::Error),
    /// The request did not arrive in time (408).
    Timeout,
    /// The request breaks the syntax (400).
    Malformed(String),
    /// The request line and headers are too long, or too many (431).
    HeadTooLarge,
    /// The body is longer than this request may have (413).
    BodyTooLarge { limit: usize },
    /// Something HTTP has that this server does not do (501).
    NotImplemented(String),
    /// An `Expect` other than `100-continue` (417).
    ExpectationFailed,
    /// An HTTP version other than 1.0 and 1.1 (505).
    Version(String),
}

impl HttpError {
    /// The status to answer with, or `None` when there is nobody to answer.
    pub fn status(&self) -> Option<u16> {
        Some(match self {
            HttpError::Closed | HttpError::Io(_) => return None,
            HttpError::Timeout => 408,
            HttpError::Malformed(_) => 400,
            HttpError::HeadTooLarge => 431,
            HttpError::BodyTooLarge { .. } => 413,
            HttpError::NotImplemented(_) => 501,
            HttpError::ExpectationFailed => 417,
            HttpError::Version(_) => 505,
        })
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HttpError::Closed => write!(f, "the connection closed before a request"),
            HttpError::Io(e) => write!(f, "{e}"),
            HttpError::Timeout => write!(f, "the request did not arrive in time"),
            HttpError::Malformed(why) => write!(f, "malformed request: {why}"),
            HttpError::HeadTooLarge => write!(f, "the request line and headers are too large"),
            HttpError::BodyTooLarge { limit } => {
                write!(f, "the request body is larger than {limit} bytes")
            }
            HttpError::NotImplemented(what) => write!(f, "{what} is not implemented"),
            HttpError::ExpectationFailed => write!(f, "only 'Expect: 100-continue' is understood"),
            HttpError::Version(v) => write!(f, "HTTP version '{v}' is not supported (1.1 and 1.0 are)"),
        }
    }
}

impl From<io::Error> for HttpError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HttpError::Timeout,
            _ => HttpError::Io(e),
        }
    }
}

fn malformed(why: impl Into<String>) -> HttpError {
    HttpError::Malformed(why.into())
}

/// A socket that gives up at a fixed instant: each read waits only for the time left
/// before the deadline, so a client sending one byte a second still runs out of time.
pub struct Deadline<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
}

impl<'a> Deadline<'a> {
    pub fn new(stream: &'a TcpStream, timeout: Duration) -> Self {
        Deadline {
            stream,
            deadline: Instant::now() + timeout,
        }
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        // A zero timeout means "wait forever" to set_read_timeout.
        self.stream
            .set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        (&mut &*self.stream).read(buf)
    }
}

impl Write for Deadline<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&mut &*self.stream).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&mut &*self.stream).flush()
    }
}

/// A request whose head has been read and accepted, its body still to come: what
/// [`read_request_head`] returns. The server judges the head (its `Host`, its token)
/// before [`Head::read_body`] reads or makes room for a byte of the body, so a request
/// it refuses costs no more than its head.
#[derive(Debug)]
pub struct Head {
    /// The request, with an empty body.
    pub request: Request,
    /// `Content-Length`, within the body limit.
    length: usize,
    /// What was read after the head: the start of the body.
    rest: Vec<u8>,
}

impl Head {
    /// The body's length, as `Content-Length` says.
    pub fn body_length(&self) -> usize {
        self.length
    }

    /// Reads the body. A request with `Expect: 100-continue` (curl sends it before
    /// large bodies) gets its `100 Continue` first, which is why `stream` is written to.
    pub fn read_body<S: Read + Write>(self, stream: &mut S) -> Result<Request, HttpError> {
        let Head {
            mut request,
            length,
            rest: mut body,
        } = self;
        // Only worth saying when the body is still to come (read_request_head refused
        // every other Expect).
        if body.len() < length && request.header("expect").is_some() {
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            stream.flush()?;
        }
        // Anything past the body (a pipelined request) is dropped with the connection.
        body.truncate(length);
        // The buffer grows as the bytes arrive rather than being made full size up
        // front: a client that announces 24 MiB and sends nothing holds next to nothing.
        let missing = (length - body.len()) as u64;
        body.reserve(missing.min(64 * 1024) as usize);
        (&mut *stream).take(missing).read_to_end(&mut body)?;
        if body.len() < length {
            return Err(malformed(format!(
                "the body ended after {} of {length} bytes",
                body.len()
            )));
        }
        request.body = body;
        Ok(request)
    }
}

/// Reads one request's head from `stream` within `limits`, and checks what it says
/// about the body: `body_limit(method, path)` is the most body bytes that request may
/// carry (the path as sent, before decoding). The body is left for
/// [`Head::read_body`].
pub fn read_request_head<S: Read>(
    stream: &mut S,
    limits: &Limits,
    body_limit: impl Fn(&str, &str) -> usize,
) -> Result<Head, HttpError> {
    let (head, rest) = read_head(stream, limits)?;
    let request = parse_head(&head, limits)?;

    if request.header("transfer-encoding").is_some() {
        return Err(HttpError::NotImplemented("Transfer-Encoding".into()));
    }
    let length = match request.header_count("content-length") {
        0 => 0,
        1 => parse_length(request.header("content-length").unwrap_or_default())?,
        _ => return Err(malformed("more than one Content-Length")),
    };
    let limit = body_limit(&request.method, &request.path);
    if length > limit as u64 {
        return Err(HttpError::BodyTooLarge { limit });
    }
    match request.header("expect").map(str::to_ascii_lowercase).as_deref() {
        None | Some("100-continue") => {}
        Some(_) => return Err(HttpError::ExpectationFailed),
    }
    Ok(Head {
        request,
        length: length as usize,
        rest,
    })
}

/// Reads one whole request: [`read_request_head`], then [`Head::read_body`] with
/// nothing judged in between.
pub fn read_request<S: Read + Write>(
    stream: &mut S,
    limits: &Limits,
    body_limit: impl Fn(&str, &str) -> usize,
) -> Result<Request, HttpError> {
    read_request_head(stream, limits, body_limit)?.read_body(stream)
}

/// Reads up to the blank line that ends the head. Returns the head (without that blank
/// line) and whatever was read after it.
fn read_head<S: Read>(stream: &mut S, limits: &Limits) -> Result<(Vec<u8>, Vec<u8>), HttpError> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    // Bytes before `scanned` hold no CRLFCRLF and no bare LF.
    let mut scanned: usize = 0;
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) if buf.is_empty() => return Err(HttpError::Closed),
            Ok(0) => return Err(malformed("the connection closed inside the request head")),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        buf.extend_from_slice(&chunk[..n]);
        // Look again from a little before the new bytes: the terminator may straddle.
        let from = scanned.saturating_sub(3);
        for i in from..buf.len() {
            if buf[i] == b'\n' && (i == 0 || buf[i - 1] != b'\r') {
                return Err(malformed("a line ends in a bare LF (lines end in CRLF)"));
            }
            // A head is ASCII text without control characters (the parser refuses any
            // other byte in every place): say so now rather than wait for the end of
            // what may be no request at all.
            if !(buf[i] == b'\r' || buf[i] == b'\n' || buf[i] == b'\t' || (0x20..0x7f).contains(&buf[i])) {
                return Err(malformed("the request head has control or non-ASCII bytes"));
            }
            if buf[i..].starts_with(b"\r\n\r\n") {
                // The limit counts the blank line that ends the head too.
                if i + 4 > limits.max_head {
                    return Err(HttpError::HeadTooLarge);
                }
                let rest = buf.split_off(i + 4);
                buf.truncate(i);
                return Ok((buf, rest));
            }
        }
        scanned = buf.len();
        // No CRLFCRLF starts before the last 3 bytes, so the head is longer than this.
        if buf.len() >= limits.max_head {
            return Err(HttpError::HeadTooLarge);
        }
    }
}

/// `tchar` of RFC 9110: what a method or a header name is made of.
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn parse_head(head: &[u8], limits: &Limits) -> Result<Request, HttpError> {
    // One empty line before the request line is ignored, as RFC 9112 section 2.2
    // asks (clients have sent a stray CRLF after a body).
    let head = head.strip_prefix(b"\r\n").unwrap_or(head);
    // Every line ends in CRLF (read_head refused bare LFs), so a CR anywhere else is
    // a stray one.
    let mut lines = head
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let request_line = lines.next().unwrap_or_default();
    let mut request = parse_request_line(request_line)?;
    for line in lines {
        if request.headers.len() == limits.max_headers {
            return Err(HttpError::HeadTooLarge);
        }
        request.headers.push(parse_header(line)?);
    }
    let http11 = request_line.ends_with(b"HTTP/1.1");
    if http11 && request.header_count("host") != 1 {
        return Err(malformed("an HTTP/1.1 request has exactly one Host header"));
    }
    Ok(request)
}

fn parse_request_line(line: &[u8]) -> Result<Request, HttpError> {
    if line.is_empty() {
        return Err(malformed("no request line"));
    }
    let text = std::str::from_utf8(line).map_err(|_| malformed("the request line is not ASCII"))?;
    let parts: Vec<&str> = text.split(' ').collect();
    let [method, target, version] = parts[..] else {
        return Err(malformed(
            "the request line is not 'METHOD target HTTP/version', single spaces apart",
        ));
    };
    if method.is_empty() || !method.bytes().all(is_tchar) {
        return Err(malformed("the method is not a token"));
    }
    match version {
        "HTTP/1.1" | "HTTP/1.0" => {}
        v if v.len() == 8
            && v.starts_with("HTTP/")
            && v.as_bytes()[5].is_ascii_digit()
            && v.as_bytes()[6] == b'.'
            && v.as_bytes()[7].is_ascii_digit() =>
        {
            return Err(HttpError::Version(v.to_string()))
        }
        _ => return Err(malformed("the request line does not end in an HTTP version")),
    }
    if !target.starts_with('/') {
        return Err(malformed("the target is not a path (it starts with '/')"));
    }
    if !target.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(malformed("the target has characters URLs do not"));
    }
    if target.contains('#') {
        return Err(malformed("the target has a fragment"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let segments = path[1..]
        .split('/')
        .map(|s| percent_decode(s, false))
        .collect::<Result<_, _>>()?;
    let query = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((percent_decode(name, true)?, percent_decode(value, true)?))
        })
        .collect::<Result<_, HttpError>>()?;
    Ok(Request {
        method: method.to_string(),
        version: version.to_string(),
        path: path.to_string(),
        segments,
        query,
        headers: Vec::new(),
        body: Vec::new(),
    })
}

/// Decodes `%XX` escapes (and `+` as a space in a query). The result must be UTF-8
/// without control characters.
pub fn percent_decode(s: &str, plus_is_space: bool) -> Result<String, HttpError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or_else(|| malformed("a '%' in the target is not followed by two hex digits"))?;
                out.push(u8::from_str_radix(hex, 16).expect("two hex digits"));
                i += 3;
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    let text =
        String::from_utf8(out).map_err(|_| malformed("the target decodes to something that is not UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(malformed("the target decodes to control characters"));
    }
    Ok(text)
}

fn parse_header(line: &[u8]) -> Result<(String, String), HttpError> {
    if line.first().is_some_and(|&b| b == b' ' || b == b'\t') {
        return Err(malformed("a header is folded over two lines"));
    }
    let colon = line
        .iter()
        .position(|&b| b == b':')
        .ok_or_else(|| malformed("a header line has no ':'"))?;
    let (name, value) = (&line[..colon], &line[colon + 1..]);
    if name.is_empty() || !name.iter().all(|&b| is_tchar(b)) {
        return Err(malformed(
            "a header name is not a token (no whitespace before the ':')",
        ));
    }
    let value = value.trim_ascii();
    if !value.iter().all(|&b| b == b'\t' || (0x20..0x7f).contains(&b)) {
        return Err(malformed("a header value has control or non-ASCII characters"));
    }
    let name = String::from_utf8(name.to_ascii_lowercase()).expect("ASCII");
    let value = String::from_utf8(value.to_vec()).expect("ASCII");
    Ok((name, value))
}

fn parse_length(s: &str) -> Result<u64, HttpError> {
    if s.is_empty() || s.len() > 19 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed("Content-Length is not a number"));
    }
    s.parse().map_err(|_| malformed("Content-Length is not a number"))
}

/// A response: status, headers, body. [`Response::write_to`] adds `Content-Length`
/// and `Connection: close`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
        }
    }

    /// A plain-text response.
    pub fn text(status: u16, text: &str) -> Response {
        Response::new(status, "text/plain; charset=utf-8", format!("{text}\n"))
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The value of the first header named `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Writes the response; for a `HEAD` request (`head_only`) without the body, but
    /// with the length the body would have.
    pub fn write_to<W: Write>(&self, w: &mut W, head_only: bool) -> io::Result<()> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        for (name, value) in &self.headers {
            out += &format!("{name}: {value}\r\n");
        }
        out += &format!("Content-Length: {}\r\nConnection: close\r\n\r\n", self.body.len());
        let mut bytes = out.into_bytes();
        if !head_only {
            bytes.extend_from_slice(&self.body);
        }
        w.write_all(&bytes)?;
        w.flush()
    }
}

/// The reason phrase of the statuses Pendragon sends.
pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        417 => "Expectation Failed",
        422 => "Unprocessable Content",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A stream that reads from `input` and records what is written.
    struct Pipe {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn pipe(input: &[u8]) -> Pipe {
        Pipe {
            input: Cursor::new(input.to_vec()),
            output: Vec::new(),
        }
    }

    const BODY_LIMIT: usize = 100;

    fn parse(input: &[u8]) -> Result<Request, HttpError> {
        read_request(&mut pipe(input), &Limits::default(), |_, _| BODY_LIMIT)
    }

    fn status(input: &[u8]) -> Option<u16> {
        match parse(input) {
            Ok(r) => panic!("{:?} parsed as {r:?}", String::from_utf8_lossy(input)),
            Err(e) => e.status(),
        }
    }

    #[test]
    fn a_get() {
        let r =
            parse(b"GET /api/vms/3?from=10&tap=2&x HTTP/1.1\r\nHost: localhost:7402\r\nX-A:  b c \r\n\r\n")
                .unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/api/vms/3");
        assert_eq!(r.segments, ["api", "vms", "3"]);
        assert_eq!(r.query("from"), Some("10"));
        assert_eq!(r.query("tap"), Some("2"));
        assert_eq!(r.query("x"), Some(""));
        assert_eq!(r.query("y"), None);
        assert_eq!(r.header("host"), Some("localhost:7402"));
        assert_eq!(r.header("x-a"), Some("b c"));
        assert!(r.body.is_empty());
        let r = parse(b"GET / HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
        assert_eq!(r.segments, [""]);
        // HTTP/1.0 needs no Host.
        let r = parse(b"GET /app.js HTTP/1.0\r\n\r\n").unwrap();
        assert_eq!(r.segments, ["app.js"]);
    }

    #[test]
    fn a_post_with_a_body() {
        let r = parse(b"POST /api/disks HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        assert_eq!(r.body, b"hello");
        // Whatever follows the body is not part of it.
        let r =
            parse(b"POST /x HTTP/1.1\r\nHost: h\r\nContent-Length: 2\r\n\r\nhiGET / HTTP/1.1\r\n").unwrap();
        assert_eq!(r.body, b"hi");
        let r = parse(b"DELETE /api/disks/d HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!(r.method, "DELETE");
        assert!(r.body.is_empty());
    }

    #[test]
    fn percent_encoding() {
        let r = parse(b"GET /a%20b/c%2Fd?q=1+2%26&n%3D=%C3%A9 HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
        assert_eq!(r.segments, ["a b", "c/d"]);
        assert_eq!(r.query, [("q".into(), "1 2&".into()), ("n=".into(), "é".into())]);
        // '+' is only a space in the query.
        let r = parse(b"GET /a+b HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
        assert_eq!(r.segments, ["a+b"]);
        for bad in ["%", "%4", "%zz", "%FF", "%00", "%0a", "%C3"] {
            let line = format!("GET /{bad} HTTP/1.1\r\nHost: h\r\n\r\n");
            assert_eq!(status(line.as_bytes()), Some(400), "{bad}");
            let line = format!("GET /?q={bad} HTTP/1.1\r\nHost: h\r\n\r\n");
            assert_eq!(status(line.as_bytes()), Some(400), "{bad}");
        }
    }

    #[test]
    fn malformed_requests() {
        for bad in [
            &b"\r\n\r\n"[..],
            b"GET\r\nHost: h\r\n\r\n",
            b"GET /\r\nHost: h\r\n\r\n",
            b"GET  / HTTP/1.1\r\nHost: h\r\n\r\n",
            b"GET / HTTP/1.1 \r\nHost: h\r\n\r\n",
            b"GET\t/ HTTP/1.1\r\nHost: h\r\n\r\n",
            b"G(T / HTTP/1.1\r\nHost: h\r\n\r\n",
            b"GET http://example.com/ HTTP/1.1\r\nHost: h\r\n\r\n",
            b"GET * HTTP/1.1\r\nHost: h\r\n\r\n",
            b"GET /a#b HTTP/1.1\r\nHost: h\r\n\r\n",
            b"GET /\x7f HTTP/1.1\r\nHost: h\r\n\r\n",
            b"GET /\xc3\xa9 HTTP/1.1\r\nHost: h\r\n\r\n",
            b"GET / HTTPS/1.1\r\nHost: h\r\n\r\n",
            b"GET / http/1.1\r\nHost: h\r\n\r\n",
            b"GET / HTTP/1.1\nHost: h\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\n\n",
            b"GET / HTTP/1.1\r\nHost: h\r\n\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nX: a\rb\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nX-A: 1\r\n folded\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nX-A : 1\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\n: 1\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nno colon\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nX: \x01\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nX: \xc3\xa9\r\n\r\n",
            // Host: none, or two.
            b"GET / HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
            // Content-Length: not a number, or twice.
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: -1\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 1 2\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: +1\r\n\r\nx",
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 0x1\r\n\r\nx",
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length:\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            // A body shorter than it says.
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 10\r\n\r\nshort",
            // Ends inside the head.
            b"GET / HTTP/1.1\r\nHost: h\r\n",
        ] {
            assert_eq!(status(bad), Some(400), "{:?}", String::from_utf8_lossy(bad));
        }
        assert!(matches!(parse(b""), Err(HttpError::Closed)));
        assert_eq!(status(b"GET / HTTP/2.0\r\nHost: h\r\n\r\n"), Some(505));
        assert_eq!(status(b"GET / HTTP/0.9\r\nHost: h\r\n\r\n"), Some(505));
        assert_eq!(
            status(b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"),
            Some(501)
        );
        assert_eq!(
            status(b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Some(501)
        );
        assert_eq!(
            status(b"POST / HTTP/1.1\r\nHost: h\r\nExpect: something\r\nContent-Length: 1\r\n\r\nx"),
            Some(417)
        );
    }

    #[test]
    fn oversize_requests() {
        // The body limit is checked before any of the body is read.
        let head = format!(
            "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: {}\r\n\r\n",
            BODY_LIMIT + 1
        );
        assert_eq!(status(head.as_bytes()), Some(413));
        let head = "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 99999999999999999999\r\n\r\n";
        assert_eq!(status(head.as_bytes()), Some(400));
        let head = "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 9999999999999999999\r\n\r\n";
        assert_eq!(status(head.as_bytes()), Some(413));
        let body = "x".repeat(BODY_LIMIT);
        let ok = format!("POST / HTTP/1.1\r\nHost: h\r\nContent-Length: {BODY_LIMIT}\r\n\r\n{body}");
        assert_eq!(parse(ok.as_bytes()).unwrap().body.len(), BODY_LIMIT);
        // The limit can depend on the method and path.
        let limit = |method: &str, path: &str| {
            if method == "POST" && path == "/big" {
                1000
            } else {
                10
            }
        };
        let req = |path: &str| {
            let text = format!(
                "POST {path} HTTP/1.1\r\nHost: h\r\nContent-Length: 500\r\n\r\n{}",
                "x".repeat(500)
            );
            read_request(&mut pipe(text.as_bytes()), &Limits::default(), limit)
        };
        assert_eq!(req("/big").unwrap().body.len(), 500);
        assert!(matches!(
            req("/small"),
            Err(HttpError::BodyTooLarge { limit: 10 })
        ));

        // A head over the limit, with or without its end in sight.
        let limits = Limits::default();
        let long = format!(
            "GET / HTTP/1.1\r\nHost: h\r\nX: {}\r\n\r\n",
            "a".repeat(limits.max_head)
        );
        assert_eq!(status(long.as_bytes()), Some(431));
        let endless = format!(
            "GET / HTTP/1.1\r\nHost: h\r\nX: {}",
            "a".repeat(limits.max_head * 4)
        );
        assert_eq!(status(endless.as_bytes()), Some(431));
        let mut many = "GET / HTTP/1.1\r\nHost: h\r\n".to_string();
        for i in 0..limits.max_headers {
            many += &format!("X-{i}: y\r\n");
        }
        assert_eq!(status(format!("{many}\r\n").as_bytes()), Some(431));
        // One fewer is fine (Host is one of them).
        let mut enough = "GET / HTTP/1.1\r\nHost: h\r\n".to_string();
        for i in 1..limits.max_headers {
            enough += &format!("X-{i}: y\r\n");
        }
        assert_eq!(
            parse(format!("{enough}\r\n").as_bytes()).unwrap().headers.len(),
            64
        );
    }

    #[test]
    fn the_head_limit_counts_every_byte() {
        let limits = Limits::default();
        // A head of `extra` bytes over the limit, its final blank line included.
        let head = |extra: usize| {
            let (start, end) = ("GET / HTTP/1.1\r\nHost: h\r\nX: ", "\r\n\r\n");
            let fill = "a".repeat(limits.max_head + extra - start.len() - end.len());
            format!("{start}{fill}{end}").into_bytes()
        };
        assert_eq!(head(0).len(), limits.max_head);
        assert!(parse(&head(0)).is_ok());
        assert_eq!(status(&head(1)), Some(431));
        assert_eq!(status(&head(4)), Some(431));
        // However the bytes arrive.
        let trickle = |bytes: Vec<u8>| read_request(&mut Trickle(bytes, 0), &limits, |_, _| 10);
        assert!(trickle(head(0)).is_ok());
        assert!(matches!(trickle(head(1)), Err(HttpError::HeadTooLarge)));
    }

    #[test]
    fn one_empty_line_before_the_request_is_ignored() {
        let r = parse(b"\r\nGET /x HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
        assert_eq!(r.segments, ["x"]);
        assert_eq!(status(b"\r\n\r\nGET / HTTP/1.1\r\nHost: h\r\n\r\n"), Some(400));
    }

    #[test]
    fn the_head_comes_before_the_body() {
        // The head is judged with nothing of the body read, nor a 100 Continue sent.
        let text = b"POST /p HTTP/1.1\r\nHost: h\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n";
        let mut p = pipe(text);
        let head = read_request_head(&mut p, &Limits::default(), |_, _| 10).unwrap();
        assert_eq!(head.body_length(), 5);
        assert!(head.request.body.is_empty());
        assert!(p.output.is_empty());
        // The body, once asked for: the 100 goes out, then the bytes are read.
        p.input.get_mut().extend_from_slice(b"hello");
        let r = head.read_body(&mut p).unwrap();
        assert_eq!(r.body, b"hello");
        assert_eq!(p.output, b"HTTP/1.1 100 Continue\r\n\r\n");
        // A body that says it is large and never comes is an error, not a hang.
        let mut p = pipe(b"POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 1000000\r\n\r\nabc");
        let head = read_request_head(&mut p, &Limits::default(), |_, _| 1 << 20).unwrap();
        assert!(matches!(head.read_body(&mut p), Err(HttpError::Malformed(_))));
    }

    #[test]
    fn expect_continue() {
        let mut p = pipe(b"POST / HTTP/1.1\r\nHost: h\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\n");
        // The body is not there: the client waits for the 100 (and here never sends).
        assert!(read_request(&mut p, &Limits::default(), |_, _| 10).is_err());
        assert_eq!(p.output, b"HTTP/1.1 100 Continue\r\n\r\n");
        let mut p =
            pipe(b"POST / HTTP/1.1\r\nHost: h\r\nExpect: 100-Continue\r\nContent-Length: 2\r\n\r\nok");
        assert_eq!(
            read_request(&mut p, &Limits::default(), |_, _| 10).unwrap().body,
            b"ok"
        );
        assert!(p.output.is_empty());
        // A body over the limit is refused before the 100.
        let mut p = pipe(b"POST / HTTP/1.1\r\nHost: h\r\nExpect: 100-continue\r\nContent-Length: 20\r\n\r\n");
        assert!(matches!(
            read_request(&mut p, &Limits::default(), |_, _| 10),
            Err(HttpError::BodyTooLarge { .. })
        ));
        assert!(p.output.is_empty());
    }

    /// A stream that hands out one byte per read, to check that the parser does not
    /// depend on how the bytes are split.
    struct Trickle(Vec<u8>, usize);

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.get(self.1) {
                Some(&b) if !buf.is_empty() => {
                    buf[0] = b;
                    self.1 += 1;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }

    impl Write for Trickle {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn byte_by_byte() {
        let text = b"POST /p?a=1 HTTP/1.1\r\nHost: h\r\nContent-Length: 3\r\n\r\nabc";
        let r = read_request(&mut Trickle(text.to_vec(), 0), &Limits::default(), |_, _| 10).unwrap();
        assert_eq!((r.path.as_str(), r.body.as_slice()), ("/p", &b"abc"[..]));
        let bad = b"GET / HTTP/1.1\r\nHost: h\n\r\n";
        assert!(matches!(
            read_request(&mut Trickle(bad.to_vec(), 0), &Limits::default(), |_, _| 10),
            Err(HttpError::Malformed(_))
        ));
    }

    #[test]
    fn slow_clients_time_out() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            // One byte at a time, never finishing: each read succeeds, but the
            // request as a whole runs out of time.
            for b in b"GET / HTTP/1.1\r\nHost: h\r\nX: ".iter().cycle().take(40) {
                if s.write_all(&[*b]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let limits = Limits {
            timeout: Duration::from_millis(300),
            ..Limits::default()
        };
        let start = Instant::now();
        let mut deadline = Deadline::new(&stream, limits.timeout);
        let err = read_request(&mut deadline, &limits, |_, _| 10).unwrap_err();
        assert!(matches!(err, HttpError::Timeout), "{err:?}");
        assert_eq!(err.status(), Some(408));
        assert!(start.elapsed() < Duration::from_secs(5));
        drop(stream);
        client.join().unwrap();
    }

    #[test]
    fn responses() {
        let mut out = Vec::new();
        Response::text(404, "no such page")
            .with_header("X-Test", "1")
            .write_to(&mut out, false)
            .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nX-Test: 1\r\n\
             Content-Length: 13\r\nConnection: close\r\n\r\nno such page\n"
        );
        let mut out = Vec::new();
        Response::new(200, "text/css", "body{}")
            .write_to(&mut out, true)
            .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.ends_with("Content-Length: 6\r\nConnection: close\r\n\r\n"),
            "{out}"
        );
        for status in [
            200, 201, 400, 403, 404, 405, 408, 409, 413, 415, 417, 422, 431, 500, 501, 502, 503, 505,
        ] {
            assert_ne!(reason(status), "Unknown", "{status}");
        }
    }
}
