//! What other web pages, and broken or hostile clients, get from Pendragon: wrong
//! `Host` (DNS rebinding), missing or wrong tokens, foreign origins, form bodies,
//! malformed and oversize requests, slow clients. Pendragon refuses each and carries
//! on serving.

mod common;

use common::*;
use pendragon::json::Json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn server() -> pendragon::ServerThread {
    pendragon_for(&hang_up_addr(), &hang_up_addr(), MIB)
}

/// A request that never reaches Avalon (both servers are away), so a 502 means the
/// guard let it through.
fn passes(r: &Reply) -> bool {
    r.status != 403
}

#[test]
fn the_page_is_static_and_strict() {
    let server = server();
    let addr = server.addr();
    let page = request(addr, "GET", "/", &[], b"");
    assert_eq!(page.status, 200);
    assert_eq!(page.header("content-type"), Some("text/html; charset=utf-8"));
    let csp = page.header("content-security-policy").unwrap();
    assert!(csp.contains("default-src 'none'"), "{csp}");
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert!(!csp.contains("unsafe"), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(page.header("x-frame-options"), Some("DENY"));
    assert_eq!(page.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(page.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(page.header("connection"), Some("close"));
    // No script in the page itself: only the script file, which builds every element
    // with the DOM and sets data as text.
    let html = page.text();
    assert_eq!(html.matches("<script").count(), 1);
    assert!(html.contains("<script src=\"/app.js\" defer></script>"));
    assert!(!html.contains(" on"), "no inline event handlers");
    assert!(!html.contains("style="), "no inline style");
    let js = request(addr, "GET", "/app.js", &[], b"");
    assert_eq!(js.header("content-type"), Some("text/javascript; charset=utf-8"));
    let js = js.text();
    for banned in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
        "eval(",
        "new Function",
    ] {
        assert!(!js.contains(banned), "app.js uses {banned}");
    }
    assert_eq!(
        request(addr, "GET", "/style.css", &[], b"").header("content-type"),
        Some("text/css; charset=utf-8")
    );
    // HEAD: the headers, with the length, and no body.
    let head = parse_reply(&raw(
        addr,
        format!("HEAD / HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes(),
    ));
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("content-length"),
        Some(html.len().to_string().as_str())
    );
    assert!(head.body.is_empty());
    server.stop().unwrap();
}

#[test]
fn hosts_are_checked() {
    let server = server();
    let addr = server.addr();
    let port = addr.port();
    let api = Api::new(&server);
    let get = |path: &str, host: &str| {
        let text = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Pendragon-Token: {}\r\n\r\n",
            api.token
        );
        parse_reply(&raw(addr, text.as_bytes()))
    };
    for host in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("LocalHost:{port}"),
    ] {
        assert_eq!(get("/", &host).status, 200, "{host}");
        assert!(passes(&get("/api/overview", &host)), "{host}");
    }
    // DNS rebinding: a page on another name, pointed at 127.0.0.1.
    for host in [
        format!("evil.example:{port}"),
        "evil.example".to_string(),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "127.0.0.1".to_string(),
        format!("127.0.0.1:{port}.evil.example"),
    ] {
        for path in ["/", "/app.js", "/api/overview", "/api/vms", "/api/switches"] {
            let r = get(path, &host);
            assert_eq!(r.status, 403, "{host} {path}");
            assert!(r.text().contains("is not this server"), "{}", r.text());
        }
    }
    // No Host at all: HTTP/1.1 requires one, HTTP/1.0 does not, but Pendragon does.
    assert_eq!(parse_reply(&raw(addr, b"GET / HTTP/1.1\r\n\r\n")).status, 400);
    assert_eq!(parse_reply(&raw(addr, b"GET / HTTP/1.0\r\n\r\n")).status, 403);
    server.stop().unwrap();

    // --allow-host adds names.
    let server = pendragon_with(pendragon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        avalon: hang_up_addr(),
        lake: hang_up_addr(),
        allow_hosts: vec!["camelot.lan".into()],
        log: false,
        ..pendragon::Config::default()
    });
    let addr = server.addr();
    let text = format!("GET / HTTP/1.1\r\nHost: camelot.lan:{}\r\n\r\n", addr.port());
    assert_eq!(parse_reply(&raw(addr, text.as_bytes())).status, 200);
    server.stop().unwrap();
}

#[test]
fn every_api_request_needs_the_token() {
    let server = server();
    let api = Api::new(&server);
    assert_eq!(api.token.len(), 64);
    assert!(server.url().ends_with(&format!("/?token={}", api.token)));
    assert!(server.url().starts_with(&format!("http://{}/", server.addr())));
    let json = [("Content-Type", "application/json")];
    for (method, path, body) in [
        ("GET", "/api/overview", ""),
        ("GET", "/api/vms", ""),
        ("POST", "/api/vms", "{}"),
        ("POST", "/api/vms/1/stop", ""),
        ("POST", "/api/disks", "{\"name\": \"d\", \"size\": \"1M\"}"),
        ("DELETE", "/api/disks/d", ""),
        ("POST", "/api/disks/d/detach", ""),
        ("GET", "/api/vms/1/console", ""),
        ("POST", "/api/vms/1/console", "{\"data\": \"rm -rf /\\n\"}"),
        ("GET", "/api/switches", ""),
        ("POST", "/api/switches", "{\"name\": \"s\", \"nat\": true}"),
        ("DELETE", "/api/switches/s", ""),
        (
            "POST",
            "/api/switches/s/nat",
            "{\"hostfwd\": [\"tcp::8080-:80\"]}",
        ),
        ("DELETE", "/api/switches/s/nat", ""),
        ("POST", "/api/switches/s/attach", "{\"vm\": 1}"),
        ("POST", "/api/switches/s/detach", "{\"vm\": 1}"),
        ("GET", "/api/nope", ""),
    ] {
        let missing = request(api.addr, method, path, &json, body.as_bytes());
        assert_eq!(missing.status, 403, "{method} {path}");
        assert!(missing.error().starts_with("no token"), "{}", missing.error());
        for wrong in [
            "",
            "x",
            &api.token[1..],
            &format!("{}0", api.token),
            &api.token.to_uppercase(),
        ] {
            let headers = [json[0], ("X-Pendragon-Token", wrong)];
            let r = request(api.addr, method, path, &headers, body.as_bytes());
            assert_eq!(r.status, 403, "{method} {path} {wrong}");
            assert!(r.error().starts_with("wrong token"), "{}", r.error());
        }
        let headers = [json[0], ("X-Pendragon-Token", api.token.as_str())];
        let r = request(api.addr, method, path, &headers, body.as_bytes());
        assert!(passes(&r), "{method} {path}: {}", r.text());
    }
    // The pages themselves need no token: they are the same for everyone.
    assert_eq!(request(api.addr, "GET", "/", &[], b"").status, 200);

    // Each launch has its own token.
    let other = self::server();
    assert_ne!(Api::new(&other).token, api.token);
    let r = request(
        other.addr(),
        "GET",
        "/api/overview",
        &[("X-Pendragon-Token", api.token.as_str())],
        b"",
    );
    assert_eq!(r.status, 403);
    other.stop().unwrap();
    server.stop().unwrap();
}

#[test]
fn changes_come_from_pendragons_own_page() {
    let server = server();
    let api = Api::new(&server);
    let token = ("X-Pendragon-Token", api.token.as_str());
    let json = ("Content-Type", "application/json");
    let body = b"{\"name\": \"d\", \"size\": \"1M\"}";
    let own = format!("http://{}", api.addr);
    // Another site's page (which could only get here with the token anyway).
    for origin in [
        "http://evil.example",
        "null",
        "https://evil.example",
        own.replace("http", "https").as_str(),
    ] {
        let r = request(
            api.addr,
            "POST",
            "/api/disks",
            &[token, json, ("Origin", origin)],
            body,
        );
        assert_eq!(r.status, 403, "{origin}");
        assert!(r.error().contains("not accepted"), "{}", r.error());
        let r = request(
            api.addr,
            "POST",
            "/api/vms/1/destroy",
            &[token, ("Origin", origin)],
            b"",
        );
        assert_eq!(r.status, 403, "{origin}");
        // A gateway forwards host ports: another site must not be able to open one.
        let r = request(
            api.addr,
            "POST",
            "/api/switches",
            &[token, json, ("Origin", origin)],
            b"{\"name\": \"s\", \"nat\": true, \"hostfwd\": [\"tcp:0.0.0.0:8080-:80\"]}",
        );
        assert_eq!(r.status, 403, "{origin}");
        let r = request(
            api.addr,
            "DELETE",
            "/api/switches/s",
            &[token, ("Origin", origin)],
            b"",
        );
        assert_eq!(r.status, 403, "{origin}");
    }
    assert!(passes(&request(
        api.addr,
        "POST",
        "/api/disks",
        &[token, json, ("Origin", &own)],
        body
    )));
    // Reads may come without an Origin check (browsers do not always send one).
    assert!(passes(&request(
        api.addr,
        "GET",
        "/api/overview",
        &[token, ("Origin", "http://evil.example")],
        b""
    )));
    // Bodies a plain HTML form can send are refused.
    for media in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data; boundary=b",
    ] {
        let r = request(
            api.addr,
            "POST",
            "/api/disks",
            &[token, ("Content-Type", media)],
            body,
        );
        assert_eq!(r.status, 403, "{media}");
        assert!(r.error().contains("application/json"));
    }
    assert_eq!(
        request(api.addr, "POST", "/api/disks", &[token], body).status,
        403
    );
    server.stop().unwrap();
}

#[test]
fn output_is_escaped() {
    // What a request puts in an error message comes back as JSON text that no HTML
    // parser would take for markup.
    let server = server();
    let api = Api::new(&server);
    let evil = "<script>alert(1)</script>&";
    let r = api.post("disks", &obj([("name", evil.into()), ("size", "1M".into())]));
    assert_eq!(r.status, 400);
    let text = r.text();
    assert!(
        !text.contains('<') && !text.contains('>') && !text.contains('&'),
        "{text}"
    );
    assert!(r.error().contains(evil), "{}", r.error());
    assert_eq!(r.header("content-type"), Some("application/json"));
    server.stop().unwrap();
}

#[test]
fn malformed_requests_are_refused() {
    let server = server();
    let addr = server.addr();
    let host = format!("Host: {addr}\r\n");
    let token = Api::new(&server).token;
    let status = |bytes: &[u8]| parse_reply(&raw(addr, bytes)).status;
    for (bytes, want) in [
        (b"GET / HTTP/1.1\nHost: x\n\n".to_vec(), 400),
        (b"GARBAGE\r\n\r\n".to_vec(), 400),
        (format!("GET / HTTP/2.0\r\n{host}\r\n").into_bytes(), 505),
        (format!("GET /a#b HTTP/1.1\r\n{host}\r\n").into_bytes(), 400),
        (
            format!("GET http://evil/ HTTP/1.1\r\n{host}\r\n").into_bytes(),
            400,
        ),
        (
            format!("GET / HTTP/1.1\r\n{host}X: 1\r\n folded\r\n\r\n").into_bytes(),
            400,
        ),
        (format!("GET / HTTP/1.1\r\n{host}X : 1\r\n\r\n").into_bytes(), 400),
        (format!("GET /%zz HTTP/1.1\r\n{host}\r\n").into_bytes(), 400),
        (
            format!("POST /api/disks HTTP/1.1\r\n{host}Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n")
                .into_bytes(),
            501,
        ),
        (
            format!("POST /api/disks HTTP/1.1\r\n{host}Content-Length: 2\r\nContent-Length: 2\r\n\r\n{{}}")
                .into_bytes(),
            400,
        ),
        (
            format!("POST /api/disks HTTP/1.1\r\n{host}Content-Length: x\r\n\r\n").into_bytes(),
            400,
        ),
        (
            // A body shorter than it says (the guard passes it: it judges the head).
            format!(
                "POST /api/disks HTTP/1.1\r\n{host}X-Pendragon-Token: {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: 10\r\n\r\n{{}}"
            )
            .into_bytes(),
            400,
        ),
        (
            format!("GET / HTTP/1.1\r\n{host}X: {}\r\n\r\n", "a".repeat(20_000)).into_bytes(),
            431,
        ),
        (
            format!("GET / HTTP/1.1\r\n{host}{}\r\n", "X-A: b\r\n".repeat(100)).into_bytes(),
            431,
        ),
    ] {
        assert_eq!(
            status(&bytes),
            want,
            "{:?}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
    }
    // Garbage, then an empty connection: no answer, no harm.
    let r = raw(addr, &[0xff; 64]);
    assert_eq!(parse_reply(&r).status, 400);
    drop(TcpStream::connect(addr).unwrap());
    assert_eq!(request(addr, "GET", "/", &[], b"").status, 200);
    server.stop().unwrap();
}

#[test]
fn oversize_bodies_are_refused_unread() {
    let server = server();
    let api = Api::new(&server);
    let token = ("X-Pendragon-Token", api.token.as_str());
    // The limit is checked against Content-Length before the body is read: a client
    // that says it will send a gigabyte is answered at once.
    for (path, limit) in [
        ("/api/disks", pendragon::MAX_BODY),
        ("/api/vms", pendragon::MAX_UPLOAD),
    ] {
        let mut s = TcpStream::connect(api.addr).unwrap();
        s.set_read_timeout(Some(SLOW)).unwrap();
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\n{}: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            api.addr,
            token.0,
            token.1,
            limit + 1
        );
        s.write_all(head.as_bytes()).unwrap();
        let mut reply = Vec::new();
        let _ = s.read_to_end(&mut reply);
        assert_eq!(parse_reply(&reply).status, 413, "{path}");
        // Just under the limit is read (and then found not to be JSON).
        let body = vec![b' '; limit.min(1 << 20)];
        let r = request(
            api.addr,
            "POST",
            path,
            &[token, ("Content-Type", "application/json")],
            &body,
        );
        assert_eq!(r.status, 400, "{path}");
    }
    // A full-size body for a small route: 413 while it is still being sent.
    let big = vec![b'x'; pendragon::MAX_BODY + 1];
    let r = request(
        api.addr,
        "POST",
        "/api/disks",
        &[token, ("Content-Type", "application/json")],
        &big,
    );
    assert_eq!(r.status, 413);
    server.stop().unwrap();
}

#[test]
fn refusals_come_before_the_body() {
    // A request the guard refuses is answered from its head: Pendragon neither waits
    // for nor makes room for the body it announces (up to MAX_UPLOAD, 24 MiB).
    let server = server();
    let addr = server.addr();
    let token = Api::new(&server).token;
    let token = token.as_str();
    let host = addr.to_string();
    let host = host.as_str();
    let big = pendragon::MAX_UPLOAD.to_string();
    for (why, headers) in [
        (
            "wrong host",
            vec![("Host", "evil.example"), ("Content-Type", "text/plain")],
        ),
        (
            "no token",
            vec![("Host", host), ("Content-Type", "application/json")],
        ),
        (
            "foreign origin",
            vec![
                ("Host", host),
                ("X-Pendragon-Token", token),
                ("Origin", "http://evil.example"),
                ("Content-Type", "application/json"),
            ],
        ),
        (
            "form body",
            vec![
                ("Host", host),
                ("X-Pendragon-Token", token),
                ("Content-Type", "text/plain"),
            ],
        ),
        (
            // The 100 Continue goes only to requests that pass.
            "no token, expecting a 100",
            vec![
                ("Host", host),
                ("Expect", "100-continue"),
                ("Content-Type", "application/json"),
            ],
        ),
    ] {
        let mut text = "POST /api/vms HTTP/1.1\r\n".to_string();
        for (name, value) in headers {
            text += &format!("{name}: {value}\r\n");
        }
        text += &format!("Content-Length: {big}\r\n\r\n");
        let mut s = TcpStream::connect(addr).unwrap();
        // Waiting for a body that never comes would run into this.
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(text.as_bytes()).unwrap();
        let mut reply = Vec::new();
        let read = s.read_to_end(&mut reply);
        assert!(read.is_ok(), "{why}: {read:?}");
        let reply = parse_reply(&reply);
        assert_eq!(reply.status, 403, "{why}: {}", reply.text());
    }
    server.stop().unwrap();
}

#[test]
fn clients_cannot_linger_after_the_response() {
    // Every client on the connection limit has its answer, then keeps sending a byte
    // now and then: Pendragon closes each connection after a moment all the same,
    // and serves others.
    let server = server();
    let addr = server.addr();
    let mut lingering: Vec<TcpStream> = (0..pendragon::MAX_CONNECTIONS)
        .map(|_| {
            let mut s = TcpStream::connect(addr).unwrap();
            s.set_read_timeout(Some(SLOW)).unwrap();
            s.write_all(format!("GET / HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())
                .unwrap();
            s
        })
        .collect();
    for s in &mut lingering {
        // The response, up to where the server says it sends no more.
        let mut reply = Vec::new();
        let _ = s.read_to_end(&mut reply);
        assert_eq!(parse_reply(&reply).status, 200);
    }
    let trickler = std::thread::spawn(move || {
        let start = Instant::now();
        let mut open = lingering.len();
        while open > 0 && start.elapsed() < Duration::from_secs(8) {
            open = 0;
            for s in &mut lingering {
                if s.write_all(b"x").is_ok() {
                    open += 1;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        open
    });
    let start = Instant::now();
    loop {
        if request(addr, "GET", "/", &[], b"").status == 200 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the lingering clients still hold every connection"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // The server closed them all: their writes fail.
    assert_eq!(trickler.join().unwrap(), 0);
    server.stop().unwrap();
}

#[test]
fn a_client_turned_away_cannot_hold_up_the_others() {
    let server = server();
    let addr = server.addr();
    let idle: Vec<TcpStream> = (0..pendragon::MAX_CONNECTIONS)
        .map(|_| TcpStream::connect(addr).unwrap())
        .collect();
    // One over the limit: its 503, then a byte now and then, for a long while.
    let mut dawdler = wait_for("a connection over the limit", || {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(SLOW)).unwrap();
        let _ = s.write_all(format!("GET / HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes());
        let mut reply = Vec::new();
        let _ = s.read_to_end(&mut reply);
        (parse_reply(&reply).status == 503).then_some(s)
    });
    let dawdling = std::thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(8) && dawdler.write_all(b"x").is_ok() {
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    // Room again: served at once, not once the dawdler stops.
    drop(idle);
    let start = Instant::now();
    wait_for("connections to be served again", || {
        (request(addr, "GET", "/", &[], b"").status == 200).then_some(())
    });
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "served after {:?}",
        start.elapsed()
    );
    dawdling.join().unwrap();
    server.stop().unwrap();
}

#[test]
fn slow_clients_time_out() {
    let server = pendragon_with(pendragon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        avalon: hang_up_addr(),
        lake: hang_up_addr(),
        limits: pendragon::http::Limits {
            timeout: Duration::from_millis(500),
            ..pendragon::http::Limits::default()
        },
        log: false,
        ..pendragon::Config::default()
    });
    let addr = server.addr();
    // Half a request, then nothing: 408 once the time is up.
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(SLOW)).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: ").unwrap();
    let start = Instant::now();
    let mut reply = Vec::new();
    let _ = s.read_to_end(&mut reply);
    assert_eq!(parse_reply(&reply).status, 408);
    assert!(start.elapsed() < Duration::from_secs(5));
    // A byte at a time is no better.
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(SLOW)).unwrap();
    let trickler = std::thread::spawn(move || {
        for b in b"GET / HTTP/1.1\r\nX-Slow: "
            .iter()
            .chain(b"a".repeat(100).iter())
        {
            if s.write_all(&[*b]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            // Stop once the answer is in, as a browser would: Pendragon lingers only a
            // moment after answering, then closes, and a reset can lose the answer.
            s.set_nonblocking(true).unwrap();
            let answered = s.peek(&mut [0]).is_ok_and(|n| n > 0);
            s.set_nonblocking(false).unwrap();
            if answered {
                break;
            }
        }
        let mut reply = Vec::new();
        let _ = s.read_to_end(&mut reply);
        reply
    });
    assert_eq!(parse_reply(&trickler.join().unwrap()).status, 408);
    // Meanwhile, and after, others are served.
    assert_eq!(request(addr, "GET", "/", &[], b"").status, 200);
    server.stop().unwrap();
}

#[test]
fn too_many_connections() {
    let server = server();
    let addr = server.addr();
    // Idle connections, each holding a thread until it times out.
    let idle: Vec<TcpStream> = (0..pendragon::MAX_CONNECTIONS)
        .map(|_| TcpStream::connect(addr).unwrap())
        .collect();
    let busy = wait_for("a connection over the limit to be refused", || {
        let r = parse_reply(&raw(
            addr,
            format!("GET / HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes(),
        ));
        (r.status == 503).then_some(r)
    });
    assert!(busy.text().contains("too many connections"));
    drop(idle);
    wait_for("connections to be served again", || {
        (request(addr, "GET", "/", &[], b"").status == 200).then_some(())
    });
    server.stop().unwrap();
}

#[test]
fn api_replies_are_json() {
    let server = server();
    let api = Api::new(&server);
    let r = api.get("overview");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(matches!(r.json(), Json::Object(_)));
    server.stop().unwrap();
}
