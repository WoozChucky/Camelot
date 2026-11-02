//! Helpers for Pendragon's tests: Avalon, Lake and Pendragon in process on ephemeral
//! ports, and a small HTTP client that speaks to Pendragon as a browser would (or as
//! a hostile page, or a broken client, would).

#![allow(dead_code)]

use pendragon::json::Json;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const MIB: u64 = 1024 * 1024;

/// Generous ceiling for anything that should take well under a second.
pub const SLOW: Duration = Duration::from_secs(10);

/// A fresh directory under the system's temporary directory, removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("pendragon-test-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The whole platform in process: Avalon (with a RAM budget), Lake (serving a
/// temporary directory) and Pendragon in front of both.
pub struct Platform {
    pub avalon: avalon::ServerThread,
    pub lake: lake::ServerThread,
    pub pendragon: pendragon::ServerThread,
    pub dir: TempDir,
}

impl Platform {
    pub fn start(tag: &str, max_ram_mib: u64) -> Platform {
        let dir = TempDir::new(tag);
        let avalon = avalon::Server::bind(&avalon::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            log: false,
            max_ram: Some(max_ram_mib * MIB),
            ..avalon::Config::default()
        })
        .unwrap()
        .spawn()
        .unwrap();
        let lake = lake::Server::bind(&lake::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            log: false,
            ..lake::Config::new(dir.path())
        })
        .unwrap()
        .spawn()
        .unwrap();
        let pendragon = pendragon_for(
            &avalon.addr().to_string(),
            &lake.addr().to_string(),
            max_ram_mib * MIB,
        );
        Platform {
            avalon,
            lake,
            pendragon,
            dir,
        }
    }

    pub fn api(&self) -> Api {
        Api::new(&self.pendragon)
    }

    pub fn lake_addr(&self) -> String {
        self.lake.addr().to_string()
    }

    pub fn stop(self) {
        self.pendragon.stop().unwrap();
        self.avalon.stop().unwrap();
        self.lake.stop().unwrap();
    }
}

/// A quiet Pendragon on 127.0.0.1 and an ephemeral port.
pub fn pendragon_for(avalon: &str, lake: &str, max_ram: u64) -> pendragon::ServerThread {
    pendragon_with(pendragon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        avalon: avalon.into(),
        lake: lake.into(),
        avalon_max_ram: max_ram,
        log: false,
        ..pendragon::Config::default()
    })
}

pub fn pendragon_with(config: pendragon::Config) -> pendragon::ServerThread {
    pendragon::Server::bind(&config).unwrap().spawn().unwrap()
}

/// An address nothing listens on.
pub fn dead_addr() -> String {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string()
}

/// An address where a server hangs up on every connection at once: a server that is
/// away, without the second or so Windows takes to give up on a port nobody listens on.
pub fn hang_up_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    addr
}

/// An HTTP response.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.body.clone()).expect("the body is UTF-8")
    }

    pub fn json(&self) -> Json {
        Json::parse(&self.text()).unwrap_or_else(|e| panic!("{e}: {}", self.text()))
    }

    /// The `error` of a JSON error reply.
    pub fn error(&self) -> String {
        self.json()
            .get("error")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Sends `bytes` as they are and reads everything until the server closes.
pub fn raw(addr: SocketAddr, bytes: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(SLOW * 3)).unwrap();
    // The server may answer (and close) before taking all of a request it refuses.
    let _ = s.write_all(bytes);
    // Nothing more is coming: a request cut short is seen as such at once.
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    out
}

/// Parses a response as a browser would read it.
pub fn parse_reply(bytes: &[u8]) -> Reply {
    let split = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no head in {:?}", String::from_utf8_lossy(bytes)));
    let head = std::str::from_utf8(&bytes[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let status = status_line[9..12].parse().unwrap();
    let headers: Vec<(String, String)> = lines
        .map(|l| {
            let (n, v) = l.split_once(':').unwrap();
            (n.to_string(), v.trim().to_string())
        })
        .collect();
    let body = bytes[split + 4..].to_vec();
    let reply = Reply {
        status,
        headers,
        body,
    };
    if let Some(len) = reply.header("content-length") {
        let len: usize = len.parse().unwrap();
        assert!(
            reply.body.len() == len || reply.body.is_empty(),
            "length {len}, body {}",
            reply.body.len()
        );
    }
    reply
}

/// One request; `headers` come after `Host`.
pub fn request(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
    for (name, value) in headers {
        text += &format!("{name}: {value}\r\n");
    }
    if !body.is_empty() || method == "POST" {
        text += &format!("Content-Length: {}\r\n", body.len());
    }
    text += "\r\n";
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(body);
    parse_reply(&raw(addr, &bytes))
}

/// Pendragon's API, used as its own page uses it: the token, the right `Host` and
/// `Origin`, JSON bodies.
pub struct Api {
    pub addr: SocketAddr,
    pub token: String,
}

impl Api {
    pub fn new(server: &pendragon::ServerThread) -> Api {
        Api {
            addr: server.addr(),
            token: server.token().to_string(),
        }
    }

    pub fn call(&self, method: &str, path: &str, body: Option<&Json>) -> Reply {
        let origin = format!("http://{}", self.addr);
        let mut headers = vec![
            ("X-Pendragon-Token", self.token.as_str()),
            ("Origin", origin.as_str()),
        ];
        let body = body.map(Json::encode).unwrap_or_default();
        if !body.is_empty() {
            headers.push(("Content-Type", "application/json"));
        }
        request(
            self.addr,
            method,
            &format!("/api/{path}"),
            &headers,
            body.as_bytes(),
        )
    }

    pub fn get(&self, path: &str) -> Reply {
        self.call("GET", path, None)
    }

    pub fn post(&self, path: &str, body: &Json) -> Reply {
        self.call("POST", path, Some(body))
    }

    /// A POST without a body: a VM action, a detach.
    pub fn action(&self, path: &str) -> Reply {
        self.call("POST", path, None)
    }

    pub fn delete(&self, path: &str) -> Reply {
        self.call("DELETE", path, None)
    }

    /// A call that must answer `status`; returns the reply's JSON.
    pub fn expect(&self, status: u16, reply: Reply) -> Json {
        assert_eq!(reply.status, status, "{}", String::from_utf8_lossy(&reply.body));
        reply.json()
    }
}

/// `{"key": value, ...}` from `(key, value)` pairs.
pub fn obj<'a>(members: impl IntoIterator<Item = (&'a str, Json)>) -> Json {
    Json::object(members)
}

pub fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../examples/{name}/{name}.elf"))
}

/// An example program, as the page uploads it.
pub fn program(name: &str) -> Json {
    pendragon::base64::encode(&std::fs::read(example(name)).unwrap()).into()
}

/// Waits until `done` returns something.
pub fn wait_for<T>(what: &str, mut done: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = done() {
            return value;
        }
        assert!(start.elapsed() < SLOW, "still waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A member as a string, a number, a list.
pub fn s<'a>(v: &'a Json, key: &str) -> &'a str {
    v.get(key)
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("no string '{key}' in {v}"))
}

pub fn n(v: &Json, key: &str) -> u64 {
    v.get(key)
        .and_then(Json::as_u64)
        .unwrap_or_else(|| panic!("no number '{key}' in {v}"))
}

pub fn list<'a>(v: &'a Json, key: &str) -> &'a [Json] {
    v.get(key)
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("no list '{key}' in {v}"))
}
