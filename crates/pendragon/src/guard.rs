//! What keeps other web pages from using Pendragon through the browser of the person
//! running it.
//!
//! Pendragon listens on loopback, but every web page the browser opens can send
//! requests to loopback too. Three checks stand in the way:
//!
//! 1. **Host** ([`Guard::check_host`], every request). A page on `evil.example` can
//!    point that name at 127.0.0.1 (DNS rebinding) and then read Pendragon's
//!    responses as its own origin. Its requests still say `Host: evil.example`, so
//!    only the names Pendragon is reached by are accepted: the listening address,
//!    `localhost` for a loopback one, and whatever `--allow-host` adds, with the port.
//! 2. **Token** ([`Guard::check_token`], every API request). A random 256-bit token
//!    made at each launch ([`crate::random`]) goes in the `X-Pendragon-Token` header.
//!    Only someone who saw the URL Pendragon printed knows it, and a page on another
//!    origin cannot set a custom header without a CORS preflight, which Pendragon
//!    never grants. Compared in constant time.
//! 3. **Origin and content type** ([`Guard::check_origin`], requests that change
//!    something). A browser says where a request comes from in `Origin`; one that is
//!    not Pendragon's own is refused. Bodies must be `application/json`, which a plain
//!    HTML form cannot send.
//!
//! The pages themselves are static, build every element from data with the DOM
//! (never markup), and are served with a Content Security Policy that allows no
//! inline script ([`SECURITY_HEADERS`]).

use crate::http::Request;
use std::net::{IpAddr, SocketAddr};

/// The header that carries the token.
pub const TOKEN_HEADER: &str = "x-pendragon-token";

/// Sent with every response: no framing, no MIME sniffing, no referrer (the page's
/// URL carries the token when it is first opened), and a policy that allows only
/// Pendragon's own script, style and requests.
pub const SECURITY_HEADERS: [(&str, &str); 5] = [
    (
        "Content-Security-Policy",
        "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
    ("Cache-Control", "no-store"),
];

/// A refused request: 403 and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

/// The checks, set up for one listening address and token.
#[derive(Debug, Clone)]
pub struct Guard {
    /// Acceptable `Host` values, lowercase.
    hosts: Vec<String>,
    token: String,
}

impl Guard {
    /// `listen` is the address Pendragon is bound to (its real port); `extra` the
    /// `--allow-host` values (`name` or `name:port`; without a port, the listening
    /// port).
    pub fn new(listen: SocketAddr, extra: &[String], token: String) -> Guard {
        let port = listen.port();
        let mut names: Vec<String> = Vec::new();
        let ip = listen.ip();
        if ip.is_loopback() || ip.is_unspecified() {
            names.push("localhost".into());
            names.push("127.0.0.1".into());
            names.push("[::1]".into());
        }
        if !ip.is_unspecified() && !names.contains(&host_text(ip)) {
            names.push(host_text(ip));
        }
        let mut hosts = Vec::new();
        for name in names {
            hosts.push(format!("{name}:{port}"));
            if port == 80 {
                // Browsers leave the default port out.
                hosts.push(name);
            }
        }
        for host in extra {
            let host = host.to_ascii_lowercase();
            if has_port(&host) {
                hosts.push(host);
            } else {
                hosts.push(format!("{host}:{port}"));
                if port == 80 {
                    hosts.push(host);
                }
            }
        }
        let mut unique: Vec<String> = Vec::new();
        for host in hosts {
            if !unique.contains(&host) {
                unique.push(host);
            }
        }
        Guard { hosts: unique, token }
    }

    /// The `Host` values accepted, lowercase.
    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// The request names Pendragon in `Host`.
    pub fn check_host(&self, req: &Request) -> Result<(), Refusal> {
        let host = req.header("host").unwrap_or_default().to_ascii_lowercase();
        if self.hosts.contains(&host) {
            Ok(())
        } else {
            Err(Refusal(format!(
                "Host '{host}' is not this server (DNS rebinding?). Use one of: {}{}",
                self.hosts.join(", "),
                ", or start pendragon with --allow-host NAME"
            )))
        }
    }

    /// The request carries the token.
    pub fn check_token(&self, req: &Request) -> Result<(), Refusal> {
        match req.header(TOKEN_HEADER) {
            None => Err(Refusal(
                "no token: open the URL pendragon printed when it started (it ends in ?token=...)".into(),
            )),
            Some(t) if constant_time_eq(t.as_bytes(), self.token.as_bytes()) => Ok(()),
            Some(_) => Err(Refusal(
                "wrong token: pendragon was restarted? Open the URL it printed when it started".into(),
            )),
        }
    }

    /// A request that changes something comes from Pendragon's own page: its
    /// `Origin`, when the browser sent one, is ours, and its body, if any, is JSON.
    /// Judged on the head alone: the server runs it before reading the body.
    pub fn check_origin(&self, req: &Request) -> Result<(), Refusal> {
        if let Some(origin) = req.header("origin") {
            let origin = origin.to_ascii_lowercase();
            let ours = origin
                .strip_prefix("http://")
                .is_some_and(|host| self.hosts.iter().any(|h| h == host));
            if !ours {
                return Err(Refusal(format!("requests from '{origin}' are not accepted")));
            }
        }
        if req.has_body() {
            let media = req.header("content-type").unwrap_or_default();
            let media = media.split(';').next().unwrap_or_default().trim();
            if !media.eq_ignore_ascii_case("application/json") {
                return Err(Refusal("the body must be application/json".into()));
            }
        }
        Ok(())
    }
}

/// An IP address as a `Host` header writes it (IPv6 in brackets).
fn host_text(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

fn has_port(host: &str) -> bool {
    match host.strip_prefix('[') {
        Some(v6) => v6.contains("]:"),
        None => host.contains(':'),
    }
}

/// Checks an `--allow-host` value: a host name, an IPv4 address or a bracketed IPv6
/// address, optionally with `:port`.
pub fn parse_allow_host(s: &str) -> Result<String, String> {
    let bad =
        || format!("'{s}' is not a host name (examples: camelot.lan, 192.168.1.5, [fd00::1], camelot:8080)");
    let (host, port) = if let Some(v6) = s.strip_prefix('[') {
        let (ip, rest) = v6.split_once(']').ok_or_else(bad)?;
        ip.parse::<std::net::Ipv6Addr>().map_err(|_| bad())?;
        (&s[..ip.len() + 2], rest.strip_prefix(':'))
    } else {
        match s.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (s, None),
        }
    };
    let name_ok = !host.is_empty()
        && (host.starts_with('[')
            || host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'));
    let port_ok = port.is_none_or(|p| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|p| p > 0)
    });
    if !name_ok || !port_ok || (s.starts_with('[') && !s.ends_with(']') && port.is_none()) {
        return Err(bad());
    }
    Ok(s.to_ascii_lowercase())
}

/// Compares two byte strings in time that depends only on their lengths, so a
/// network observer cannot learn the token one byte at a time.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(headers: &[(&str, &str)], body: &[u8]) -> Request {
        Request {
            method: "POST".into(),
            version: "HTTP/1.1".into(),
            path: "/api/vms".into(),
            segments: vec!["api".into(), "vms".into()],
            query: vec![],
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_ascii_lowercase(), v.to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }

    fn guard(listen: &str, extra: &[&str]) -> Guard {
        let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        Guard::new(listen.parse().unwrap(), &extra, "secret".into())
    }

    #[test]
    fn hosts() {
        let g = guard("127.0.0.1:7402", &[]);
        assert_eq!(g.hosts(), ["localhost:7402", "127.0.0.1:7402", "[::1]:7402"]);
        let ok = |g: &Guard, host: &str| g.check_host(&request(&[("Host", host)], b"")).is_ok();
        assert!(ok(&g, "127.0.0.1:7402"));
        assert!(ok(&g, "LOCALHOST:7402"));
        assert!(ok(&g, "[::1]:7402"));
        for bad in [
            "evil.example",
            "evil.example:7402",
            "127.0.0.1",
            "127.0.0.1:7403",
            "localhost",
            "127.0.0.1:7402.evil.example",
            "",
        ] {
            assert!(!ok(&g, bad), "{bad}");
        }
        assert!(g.check_host(&request(&[], b"")).is_err());

        // Any interface: loopback names, plus what --allow-host adds.
        let g = guard("0.0.0.0:7402", &["camelot.lan", "Box:9000", "[fd00::1]"]);
        assert!(ok(&g, "localhost:7402"));
        assert!(ok(&g, "camelot.lan:7402"));
        assert!(ok(&g, "box:9000"));
        assert!(ok(&g, "[fd00::1]:7402"));
        assert!(!ok(&g, "0.0.0.0:7402"));
        assert!(!ok(&g, "camelot.lan"));
        // A specific address is accepted by itself only.
        let g = guard("192.168.1.5:7402", &[]);
        assert_eq!(g.hosts(), ["192.168.1.5:7402"]);
        let g = guard("[::1]:7402", &[]);
        assert!(ok(&g, "[::1]:7402"));
        // Port 80 is left out by browsers.
        let g = guard("127.0.0.1:80", &["web"]);
        assert!(ok(&g, "localhost"));
        assert!(ok(&g, "localhost:80"));
        assert!(ok(&g, "web"));
    }

    #[test]
    fn tokens() {
        let g = guard("127.0.0.1:7402", &[]);
        assert!(g
            .check_token(&request(&[("X-Pendragon-Token", "secret")], b""))
            .is_ok());
        let missing = g.check_token(&request(&[], b"")).unwrap_err();
        assert!(missing.0.starts_with("no token"), "{missing:?}");
        for wrong in ["", "secreT", "secret ", "secre", "secrets", "x"] {
            let r = request(&[("X-Pendragon-Token", wrong)], b"");
            assert!(
                g.check_token(&r).unwrap_err().0.starts_with("wrong token"),
                "{wrong}"
            );
        }
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"a", b"b"));
    }

    #[test]
    fn origins() {
        let g = guard("127.0.0.1:7402", &[]);
        let json = [("Content-Type", "application/json")];
        let check = |headers: &[(&str, &str)], body: &[u8]| g.check_origin(&request(headers, body));
        assert!(check(&[], b"").is_ok());
        assert!(check(&json, b"{}").is_ok());
        assert!(check(&[("Content-Type", "Application/JSON; charset=utf-8")], b"{}").is_ok());
        assert!(check(&[("Origin", "http://127.0.0.1:7402"), json[0]], b"{}").is_ok());
        assert!(check(&[("Origin", "http://localhost:7402")], b"").is_ok());
        for origin in [
            "http://evil.example",
            "http://127.0.0.1:7403",
            "https://127.0.0.1:7402",
            "null",
            "http://127.0.0.1:7402.evil.example",
        ] {
            assert!(check(&[("Origin", origin), json[0]], b"{}").is_err(), "{origin}");
        }
        // What a plain HTML form can send.
        for media in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
        ] {
            assert!(check(&[("Content-Type", media)], b"{}").is_err(), "{media}");
        }
        assert!(check(&[], b"{}").is_err());
        // Judged from the head, before the body is read: Content-Length says one comes.
        assert!(check(&[("Content-Type", "text/plain"), ("Content-Length", "5")], b"").is_err());
        assert!(check(&[("Content-Length", "5"), json[0]], b"").is_ok());
        assert!(check(&[("Content-Type", "text/plain"), ("Content-Length", "0")], b"").is_ok());
    }

    #[test]
    fn allow_host_values() {
        for (s, want) in [
            ("camelot.lan", "camelot.lan"),
            ("Camelot", "camelot"),
            ("box:9000", "box:9000"),
            ("192.168.1.5", "192.168.1.5"),
            ("[fd00::1]", "[fd00::1]"),
            ("[fd00::1]:8080", "[fd00::1]:8080"),
        ] {
            assert_eq!(parse_allow_host(s).as_deref(), Ok(want), "{s}");
        }
        for bad in [
            "", ":80", "a b", "a/b", "a:", "a:0", "a:x", "a:99999", "[zz]", "[::1", "[::1]x", "a:1:2", "a_b",
        ] {
            assert!(parse_allow_host(bad).is_err(), "{bad}");
        }
    }
}
