//! A VM's screen in the page, without a browser: the tests speak WebSocket to
//! Pendragon as a page does (masked frames, the token in a subprotocol), against
//! Avalon in process, with the repository's examples as guests: `colorbars` (the
//! framebuffer), `flip` (a mode change to 800x600), `kbd-echo` (the keyboard, raw mode
//! after F12), `pointer` (a cursor that follows the mouse, drawn into the frame) and
//! `cursor` (the display's cursor plane, which comes apart from the frames).

mod common;

use common::*;
use pendragon::json::Json;
use pendragon::websocket::{client_frame, close_payload, OP_BINARY, OP_CLOSE, OP_PING, OP_PONG, OP_TEXT};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

const MASK: [u8; 4] = [0xa1, 0x07, 0x5c, 0xe9];
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// `examples/pointer`'s background colour.
const BACKGROUND: u32 = 0x203048;

/// One message from Pendragon.
#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Text(Json),
    /// A rectangle: x, y, width, height, RGBA pixels.
    Rect(u32, u32, u32, u32, Vec<u8>),
    Ping(Vec<u8>),
    Close(Option<u16>, String),
}

/// A page's WebSocket to a VM's screen.
struct Page {
    stream: TcpStream,
    buf: Vec<u8>,
}

/// The handshake a page's `new WebSocket(url, ["pendragon.screen", "pendragon.token.T"])`
/// sends, with `extra` header lines.
fn handshake(addr: SocketAddr, vm: &str, protocols: &str, extra: &str) -> String {
    format!(
        "GET /api/vms/{vm}/screen HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\n\
         Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {KEY}\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: {protocols}\r\n{extra}\r\n"
    )
}

fn protocols(token: &str) -> String {
    format!("pendragon.screen, pendragon.token.{token}")
}

impl Page {
    /// Opens VM `vm`'s screen as the page does.
    fn open(api: &Api, vm: &str) -> Page {
        let (status, head, page) =
            Page::try_open(api.addr, &handshake(api.addr, vm, &protocols(&api.token), ""));
        assert_eq!(status, 101, "{head}");
        assert!(
            head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"),
            "{head}"
        );
        assert!(
            head.contains("Sec-WebSocket-Protocol: pendragon.screen\r\n"),
            "{head}"
        );
        assert!(!head.contains(&api.token), "the token is not repeated: {head}");
        page
    }

    /// Sends `request` and reads the answer's head: its status, the head, and the
    /// connection with what came after the head.
    fn try_open(addr: SocketAddr, request: &str) -> (u16, String, Page) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(SLOW)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            let n = stream.read(&mut chunk).unwrap();
            assert!(
                n > 0,
                "closed before the head: {:?}",
                String::from_utf8_lossy(&buf)
            );
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8(buf[..end].to_vec()).unwrap();
        let status = head[9..12].parse().unwrap();
        (
            status,
            head,
            Page {
                stream,
                buf: buf[end..].to_vec(),
            },
        )
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) {
        self.stream
            .write_all(&client_frame(true, opcode, MASK, payload))
            .unwrap();
    }

    fn send(&mut self, json: &str) {
        self.send_frame(OP_TEXT, json.as_bytes());
    }

    fn key(&mut self, down: bool, code: &str, key: &str) {
        self.send(&format!(
            r#"{{"type":"key","down":{down},"code":"{code}","key":"{key}","shift":false,"ctrl":false,"alt":false}}"#
        ));
    }

    /// A press and a release.
    fn tap(&mut self, code: &str, key: &str) {
        self.key(true, code, key);
        self.key(false, code, key);
    }

    fn pointer(&mut self, x: i64, y: i64, buttons: u8) {
        self.send(&format!(
            r#"{{"type":"pointer","x":{x},"y":{y},"buttons":{buttons}}}"#
        ));
    }

    /// The next message, `None` once the connection is closed.
    fn recv(&mut self) -> Option<Msg> {
        loop {
            if let Some((len, msg)) = parse(&self.buf) {
                self.buf.drain(..len);
                return Some(msg);
            }
            let mut chunk = vec![0u8; 1 << 16];
            match self.stream.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return None,
                Err(e) => panic!("reading the screen: {e}"),
            }
        }
    }

    /// Messages until `stop` says so (true: that one was the last wanted).
    fn until(&mut self, what: &str, mut stop: impl FnMut(&Msg) -> bool) -> Vec<Msg> {
        let mut out = Vec::new();
        loop {
            let msg = self
                .recv()
                .unwrap_or_else(|| panic!("closed while waiting for {what}: {out:?}"));
            let done = stop(&msg);
            out.push(msg);
            if done {
                return out;
            }
        }
    }

    /// The text message of type `kind` next (rectangles before it are drawn on `canvas`).
    fn text_of(&mut self, kind: &str, canvas: &mut Canvas) -> Json {
        loop {
            match self.recv() {
                Some(Msg::Text(json)) if s(&json, "type") == kind => return json,
                Some(Msg::Text(json)) if s(&json, "type") == "screen" => canvas.resize(&json),
                Some(Msg::Text(_)) => {}
                Some(Msg::Rect(x, y, w, h, px)) => canvas.draw(x, y, w, h, &px),
                other => panic!("waiting for '{kind}': {other:?}"),
            }
        }
    }
}

/// A frame from the server (never masked), and its length; `None` until it has all
/// arrived.
fn parse(b: &[u8]) -> Option<(usize, Msg)> {
    if b.len() < 2 {
        return None;
    }
    assert_eq!(b[0] & 0xf0, 0x80, "a whole frame, no reserved bits");
    assert_eq!(b[1] & 0x80, 0, "the server does not mask");
    let (len, at) = match b[1] & 0x7f {
        126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as usize, 4),
        127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()) as usize, 10),
        126 | 127 => return None,
        n => (n as usize, 2),
    };
    if b.len() < at + len {
        return None;
    }
    let p = &b[at..at + len];
    let msg = match b[0] & 0x0f {
        OP_TEXT => Msg::Text(Json::parse(std::str::from_utf8(p).unwrap()).unwrap()),
        OP_BINARY => {
            let word = |i: usize| u32::from_le_bytes(p[i..i + 4].try_into().unwrap());
            Msg::Rect(word(0), word(4), word(8), word(12), p[16..].to_vec())
        }
        OP_PING => Msg::Ping(p.to_vec()),
        OP_CLOSE if p.is_empty() => Msg::Close(None, String::new()),
        OP_CLOSE => Msg::Close(
            Some(u16::from_be_bytes([p[0], p[1]])),
            String::from_utf8(p[2..].to_vec()).unwrap(),
        ),
        op => panic!("opcode {op:#x}"),
    };
    Some((at + len, msg))
}

/// The page's canvas: RGBA, as `putImageData` leaves it.
struct Canvas {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    resizes: usize,
}

impl Canvas {
    fn new() -> Canvas {
        Canvas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            resizes: 0,
        }
    }

    /// A `screen` message: a new canvas, black.
    fn resize(&mut self, msg: &Json) {
        self.width = n(msg, "width") as u32;
        self.height = n(msg, "height") as u32;
        self.rgba = [0, 0, 0, 0xff].repeat((self.width * self.height) as usize);
        self.resizes += 1;
    }

    fn draw(&mut self, x: u32, y: u32, w: u32, h: u32, px: &[u8]) {
        assert!(x + w <= self.width && y + h <= self.height, "inside the screen");
        assert_eq!(px.len(), (w * h * 4) as usize);
        for row in 0..h {
            let from = (row * w * 4) as usize;
            let to = (((y + row) * self.width + x) * 4) as usize;
            self.rgba[to..to + (w * 4) as usize].copy_from_slice(&px[from..from + (w * 4) as usize]);
        }
    }

    /// The pixel at (`x`, `y`) as XRGB8888, `0x00RRGGBB`.
    fn pixel(&self, x: u32, y: u32) -> u32 {
        let i = ((y * self.width + x) * 4) as usize;
        u32::from_be_bytes([0, self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]])
    }

    /// Takes the page's messages until the pixel at (`x`, `y`) is `want`.
    fn wait_for_pixel(&mut self, page: &mut Page, x: u32, y: u32, want: u32) {
        let start = Instant::now();
        while self.width == 0 || self.pixel(x, y) != want {
            assert!(
                start.elapsed() < SLOW,
                "pixel ({x}, {y}) is {:06x}",
                self.pixel(x, y)
            );
            match page.recv() {
                Some(Msg::Text(json)) if s(&json, "type") == "screen" => self.resize(&json),
                Some(Msg::Rect(x, y, w, h, px)) => self.draw(x, y, w, h, &px),
                Some(Msg::Text(_)) => {}
                other => panic!("{other:?}"),
            }
        }
    }
}

/// What Grail's display shows after running `program` to its end, in the canvas's
/// RGBA order.
fn grail_screen(program: &Path) -> Vec<u8> {
    let mut m = grail::Machine::with_ram(16 * 1024 * 1024);
    m.load_elf(&std::fs::read(program).unwrap()).unwrap();
    assert_eq!(m.run(Some(100_000_000), |_| {}), grail::RunResult::Exited(0));
    let xrgb = m.cpu.bus.frame().pixels.expect("the display is on").to_vec();
    xrgb.chunks(4).flat_map(|p| [p[2], p[1], p[0], 0xff]).collect()
}

fn create(api: &Api, name: &str, program: &Path, start: bool) -> u64 {
    let program = pendragon::base64::encode(&std::fs::read(program).unwrap());
    let r = api.expect(
        201,
        api.post(
            "vms",
            &obj([
                ("name", name.into()),
                ("ram", "16".into()),
                ("program", program.into()),
                ("start", start.into()),
            ]),
        ),
    );
    n(&r, "id")
}

fn rv64(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../examples/{name}/{name}-rv64.elf"))
}

/// The VM's console from the start, until it ends with `want`.
fn console_until(api: &Api, id: u64, want: &str) -> String {
    let start = Instant::now();
    let mut text = String::new();
    let mut next = 0;
    while !text.ends_with(want) {
        assert!(
            start.elapsed() < SLOW,
            "the console has {text:?}, waiting for {want:?}"
        );
        let r = api.expect(200, api.get(&format!("vms/{id}/console?from={next}&wait=500")));
        next = n(&r, "next");
        text += &String::from_utf8(pendragon::base64::decode(s(&r, "data")).unwrap()).unwrap();
    }
    text
}

/// The framebuffer example reaches the page's canvas pixel for pixel as Grail draws
/// it, with the VM's states, and the screen closes when the program exits: an `end`,
/// then a close frame (1000) answered by the page's own.
#[test]
fn a_vms_screen_reaches_the_canvas() {
    let platform = Platform::start("screen", 64);
    let api = platform.api();
    let id = create(&api, "bars", &example("colorbars"), false);
    let mut page = Page::open(&api, "bars");
    let mut canvas = Canvas::new();
    match page.recv() {
        Some(Msg::Text(json)) => {
            assert_eq!(
                json.encode(),
                format!(r#"{{"type":"screen","vm":{id},"width":640,"height":480}}"#)
            );
            canvas.resize(&json);
        }
        other => panic!("{other:?}"),
    }
    let state = page.text_of("state", &mut canvas);
    assert_eq!(
        state.encode(),
        r#"{"type":"state","state":"created","state_text":"created","exit_code":null}"#
    );
    // The full frame that follows the attach: black.
    match page.recv() {
        Some(Msg::Rect(0, 0, 640, 480, px)) => assert!(px.chunks(4).all(|p| p == [0, 0, 0, 0xff])),
        other => panic!("{other:?}"),
    }
    assert_eq!(n(&api.expect(200, api.get("overview")), "screens"), 1);

    api.expect(200, api.action(&format!("vms/{id}/start")));
    assert_eq!(s(&page.text_of("state", &mut canvas), "state"), "running");
    let exited = page.text_of("state", &mut canvas);
    assert_eq!((s(&exited, "state"), n(&exited, "exit_code")), ("exited", 0));
    let end = page.text_of("end", &mut canvas);
    assert_eq!(s(&end, "reason"), "the VM has exited (code 0)");
    assert_eq!(
        page.recv(),
        Some(Msg::Close(Some(1000), "the VM has exited (code 0)".into()))
    );
    page.send_frame(OP_CLOSE, &close_payload(1000, ""));
    assert_eq!(page.recv(), None, "Pendragon closes the connection");
    assert!(
        canvas.rgba == grail_screen(&example("colorbars")),
        "the canvas shows what Grail does"
    );
    wait_for("the screen to close", || {
        (n(&api.expect(200, api.get("overview")), "screens") == 0).then_some(())
    });

    // Opened after the end: the last frame, then the end at once.
    let mut late = Page::open(&api, &id.to_string());
    let mut canvas = Canvas::new();
    let state = page_end(&mut late, &mut canvas);
    assert_eq!(state, "the VM has exited (code 0)");
    assert!(canvas.rgba == grail_screen(&example("colorbars")));
    platform.stop();
}

/// Takes the page's messages to its end; returns the end's reason.
fn page_end(page: &mut Page, canvas: &mut Canvas) -> String {
    let reason = s(&page.text_of("end", canvas), "reason").to_string();
    assert!(matches!(page.recv(), Some(Msg::Close(Some(1000), _))));
    reason
}

/// A guest that changes modes (`flip`: 800x600, page flipped from RAM): a `screen`
/// message gives the canvas its new size, and the canvas ends with the last frame
/// Grail shows.
#[test]
fn a_mode_change_resizes_the_canvas() {
    let platform = Platform::start("flip", 64);
    let api = platform.api();
    let id = create(&api, "flip", &example("flip"), false);
    let mut page = Page::open(&api, &id.to_string());
    let mut canvas = Canvas::new();
    page.text_of("state", &mut canvas);
    api.expect(200, api.action(&format!("vms/{id}/start")));
    assert_eq!(page_end(&mut page, &mut canvas), "the VM has exited (code 0)");
    assert_eq!((canvas.width, canvas.height, canvas.resizes), (800, 600, 2));
    assert!(canvas.rgba == grail_screen(&example("flip")));
    platform.stop();
}

/// Keys reach a raw-mode guest as physical keys: kbd-echo switches to raw mode on
/// F12 and prints each event's HID usage. Shift alone shows, a key's release carries
/// its press's usage, the browser's auto-repeat is dropped in raw mode, and losing
/// the focus releases every key held. RV32 and RV64 guests alike.
#[test]
fn keys_reach_a_raw_mode_guest_as_physical_keys() {
    let platform = Platform::start("keys", 64);
    let api = platform.api();
    for (name, program) in [("kbd", example("kbd-echo")), ("kbd64", rv64("kbd-echo"))] {
        let id = create(&api, name, &program, true);
        let mut page = Page::open(&api, name);
        // ASCII mode first: 'q' types itself; F12 switches.
        page.tap("KeyQ", "q");
        page.key(true, "F12", "F12");
        assert_eq!(console_until(&api, id, "q\n"), "q\n");
        page.key(false, "F12", "F12");
        page.key(true, "ShiftLeft", "Shift");
        page.key(true, "KeyQ", "Q");
        page.key(true, "KeyQ", "Q"); // auto-repeat
        page.key(false, "KeyQ", "Q");
        page.key(false, "ShiftLeft", "Shift");
        assert_eq!(console_until(&api, id, "-e1 "), "q\n-45 +e1 +14 -14 -e1 ");
        // The page loses the focus with Left Alt and 'q' down.
        page.key(true, "AltLeft", "Alt");
        page.key(true, "KeyQ", "q");
        page.send(r#"{"type":"release"}"#);
        // The key-ups the browser sends later belong to another window: nothing.
        page.key(false, "KeyQ", "q");
        page.key(false, "AltLeft", "Alt");
        page.tap("Backquote", "`");
        assert_eq!(
            console_until(&api, id, "-35 "),
            "q\n-45 +e1 +14 -14 -e1 +e2 +14 -e2 -14 +35 -35 "
        );
        // The page closes with Right Ctrl held: Avalon lets go of it.
        page.key(true, "ControlRight", "Control");
        assert!(console_until(&api, id, "+e4 ").ends_with("+35 -35 +e4 "));
        page.send_frame(OP_CLOSE, &close_payload(1001, "the page left"));
        let msgs = page.until("the close", |m| matches!(m, Msg::Close(..)));
        assert_eq!(
            msgs.last(),
            Some(&Msg::Close(Some(1001), String::new())),
            "its code back"
        );
        assert!(console_until(&api, id, "-e4 ").ends_with("+e4 -e4 "));

        let mut again = Page::open(&api, name);
        again.key(true, "Escape", "Escape");
        let mut canvas = Canvas::new();
        assert_eq!(page_end(&mut again, &mut canvas), "the VM has exited (code 0)");
    }
    platform.stop();
}

/// The mouse moved and clicked over the canvas reaches the guest's pointer device:
/// its cursor goes there and turns red with the left button. A position past the
/// screen is clamped to its edge.
#[test]
fn the_pointer_reaches_the_guest() {
    let platform = Platform::start("pointer", 64);
    let api = platform.api();
    for (name, program) in [("ptr", example("pointer")), ("ptr64", rv64("pointer"))] {
        create(&api, name, &program, true);
        let mut page = Page::open(&api, name);
        let mut canvas = Canvas::new();
        canvas.wait_for_pixel(&mut page, 320, 240, BACKGROUND);
        for x in 0..=100 {
            page.pointer(x, x / 2, 0);
        }
        page.pointer(100, 50, 1);
        canvas.wait_for_pixel(&mut page, 100, 50, 0xff0000);
        // A drag off the bottom right: the cursor at the corner's pixel, gone from
        // where it was.
        page.pointer(5000, 5000, 1);
        canvas.wait_for_pixel(&mut page, 639, 479, 0xff0000);
        assert_eq!(canvas.pixel(100, 50), BACKGROUND);
        // Released: white.
        page.pointer(10, 10, 0);
        canvas.wait_for_pixel(&mut page, 10, 10, 0xffffff);
        page.tap("Escape", "Escape");
        assert_eq!(page_end(&mut page, &mut canvas), "the VM has exited (code 0)");
    }
    platform.stop();
}

/// What Grail's display does running `examples/cursor` to its end: where its cursor
/// is shown, from the cursor registers at each vertical blank that changed it, and the
/// picture at the end, the cursor blended over the frame (`0x00RRGGBB`, row by row).
fn grail_cursor(program: &Path) -> (Vec<(i32, i32)>, Vec<u32>) {
    let mut m = grail::Machine::with_ram(16 * 1024 * 1024);
    m.load_elf(&std::fs::read(program).unwrap()).unwrap();
    let mut places = Vec::new();
    let mut slices = 0;
    while m.exit_code().is_none() {
        m.run_for(1_000_000, &mut |_| {});
        slices += 1;
        assert!(slices < 10_000, "the program does not end");
        let fb = &mut m.cpu.bus.fb;
        if fb.take_cursor_update() {
            let c = fb.cursor();
            assert!(c.on);
            places.push((c.x, c.y));
        }
    }
    assert_eq!(m.exit_code(), Some(0));
    let frame = m.cpu.bus.frame();
    assert!(frame.cursor.is_some(), "the cursor shows at the end");
    let picture = (0..frame.height)
        .flat_map(|y| (0..frame.width).map(move |x| (x, y)))
        .map(|(x, y)| frame.pixel(x, y))
        .collect();
    (places, picture)
}

/// The page's cursor, as the page keeps it from the `cursor_shape` and
/// `cursor_position` messages: the image (width, height, straight RGBA), and whether
/// and where it shows.
#[derive(Debug, Default)]
struct PageCursor {
    image: Option<(u32, u32, Vec<u8>)>,
    visible: bool,
    x: i64,
    y: i64,
}

impl PageCursor {
    /// Applies a cursor message; whether `msg` was one.
    fn apply(&mut self, msg: &Json) -> bool {
        let int = |key: &str| msg.get(key).and_then(Json::as_i64).unwrap();
        match s(msg, "type") {
            "cursor_shape" => {
                let (w, h) = (n(msg, "width") as u32, n(msg, "height") as u32);
                let px = pendragon::base64::decode(s(msg, "pixels")).unwrap();
                assert_eq!(px.len(), (w * h * 4) as usize);
                self.image = Some((w, h, px));
                true
            }
            "cursor_position" => {
                self.visible = msg.get("visible").and_then(Json::as_bool).unwrap();
                (self.x, self.y) = (int("x"), int("y"));
                true
            }
            _ => false,
        }
    }

    /// What the browser shows: the cursor's canvas over the screen's, blended as a
    /// browser blends canvases, on premultiplied pixels: the straight image is
    /// multiplied by its alpha into the canvas (`putImageData`), each to the nearest,
    /// and composited source-over, `cursor + screen × (255 − A) / 255`, to the nearest.
    fn over(&self, canvas: &Canvas) -> Vec<u32> {
        let mut out: Vec<u32> = (0..canvas.height)
            .flat_map(|y| (0..canvas.width).map(move |x| (x, y)))
            .map(|(x, y)| canvas.pixel(x, y))
            .collect();
        let Some((w, h, px)) = self.image.as_ref().filter(|_| self.visible) else {
            return out;
        };
        let (w, h) = (*w as i64, *h as i64);
        for cy in 0..h {
            for cx in 0..w {
                let (x, y) = (self.x + cx, self.y + cy);
                if x < 0 || y < 0 || x >= canvas.width as i64 || y >= canvas.height as i64 {
                    continue;
                }
                let i = ((cy * w + cx) * 4) as usize;
                let a = px[i + 3] as u32;
                let screen = &mut out[(y * canvas.width as i64 + x) as usize];
                let mut blended = 0;
                for (k, shift) in [16, 8, 0].into_iter().enumerate() {
                    let cursor = (px[i + k] as u32 * a + 127) / 255;
                    let under = (*screen >> shift) & 0xff;
                    blended |= (cursor + (under * (255 - a) + 127) / 255).min(255) << shift;
                }
                *screen = blended;
            }
        }
        out
    }
}

/// The guest's cursor reaches the page apart from the frames (`examples/cursor`, RV32
/// and RV64): its image once, in a `cursor_shape` before the first `cursor_position`,
/// and no frame once it shows. Its places are Grail's cursor registers at the vertical
/// blanks that moved it (Avalon looks at the screen at its own pace, so the page may
/// miss a place, never get one out of turn) and it ends at the last; the canvas with
/// the cursor over it shows, pixel for pixel, what Grail's display does. A page that
/// opens later gets the cursor as it is, after the full frame.
#[test]
fn the_cursor_reaches_the_page_apart_from_the_frames() {
    let platform = Platform::start("cursor", 64);
    let api = platform.api();
    for (name, program) in [("cursor", example("cursor")), ("cursor64", rv64("cursor"))] {
        let (places, picture) = grail_cursor(&program);
        let id = create(&api, name, &program, false);
        let mut page = Page::open(&api, name);
        let mut canvas = Canvas::new();
        page.text_of("state", &mut canvas);
        api.expect(200, api.action(&format!("vms/{id}/start")));
        let mut cursor = PageCursor::default();
        let mut shapes = 0;
        let mut shown = Vec::new();
        let mut frames_after = 0;
        loop {
            match page.recv() {
                Some(Msg::Rect(x, y, w, h, px)) => {
                    canvas.draw(x, y, w, h, &px);
                    if shapes > 0 {
                        frames_after += 1;
                    }
                }
                Some(Msg::Text(json)) if cursor.apply(&json) => {
                    if s(&json, "type") == "cursor_shape" {
                        shapes += 1;
                    } else {
                        assert_eq!(shapes, 1, "the image before the place: {json}");
                        assert!(cursor.visible, "never hidden: {json}");
                        shown.push((cursor.x as i32, cursor.y as i32));
                    }
                }
                Some(Msg::Text(json)) if s(&json, "type") == "screen" => canvas.resize(&json),
                Some(Msg::Text(json)) if s(&json, "type") == "end" => break,
                Some(Msg::Text(_)) => {}
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(shapes, 1, "{name}: the image once");
        assert_eq!(frames_after, 0, "{name}: no frame once the cursor shows");
        let mut path = places.iter();
        assert!(
            shown.iter().all(|place| path.any(|p| p == place)),
            "{name}: {shown:?} in the order of Grail's {places:?}"
        );
        assert_eq!(shown.last(), places.last(), "{name}: the last place");
        let (w, h, px) = cursor.image.clone().unwrap();
        assert_eq!((w, h), (64, 64));
        let pixel = |x: usize, y: usize| &px[(y * 64 + x) * 4..][..4];
        assert_eq!(pixel(0, 0), [0, 0, 0, 0xff], "the tip, opaque black");
        assert_eq!(pixel(1, 2), [0xff; 4], "inside, opaque white");
        assert_eq!(
            pixel(0, 16),
            [0, 0, 0xff, 0x80],
            "the tail, straight blue half covered"
        );
        assert_eq!(pixel(40, 40), [0; 4], "clear");
        assert!(
            cursor.over(&canvas) == picture,
            "{name}: the page shows what Grail does"
        );

        // Opened after the end: the full frame, the cursor's image and place, the end.
        let mut late = Page::open(&api, name);
        let mut canvas = Canvas::new();
        let mut cursor = PageCursor::default();
        let mut order = Vec::new();
        loop {
            match late.recv() {
                Some(Msg::Rect(x, y, w, h, px)) => {
                    canvas.draw(x, y, w, h, &px);
                    order.push("frame".to_string());
                }
                Some(Msg::Text(json)) => {
                    let kind = s(&json, "type").to_string();
                    match kind.as_str() {
                        "screen" => canvas.resize(&json),
                        "end" => break,
                        _ => {
                            cursor.apply(&json);
                        }
                    }
                    order.push(kind);
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            order,
            ["screen", "state", "frame", "cursor_shape", "cursor_position"],
            "{name}"
        );
        assert_eq!((cursor.x as i32, cursor.y as i32), *places.last().unwrap());
        assert!(cursor.over(&canvas) == picture, "{name}: the late page too");
    }
    platform.stop();
}

/// The checks every API request has, on the WebSocket's handshake: the Host, the
/// token (in the subprotocols the page offers, or the header), the Origin (a GET, but
/// any page may open a WebSocket to any server); then the handshake's own rules.
#[test]
fn refused_handshakes() {
    let platform = Platform::start("refusals", 64);
    let api = platform.api();
    create(&api, "spin", &example("pointer"), false);
    let addr = api.addr;
    let good = handshake(addr, "spin", &protocols(&api.token), "");
    let other = "f".repeat(64);
    for (request, status, says) in [
        (
            good.replace(&format!("Host: {addr}"), "Host: evil.example"),
            403,
            "is not this server",
        ),
        (handshake(addr, "spin", "pendragon.screen", ""), 403, "no token"),
        (
            handshake(addr, "spin", &protocols(&other), ""),
            403,
            "wrong token",
        ),
        // The token's subprotocol counts on the screen's route only: an upgrade to
        // another one is a request without the token.
        (good.replace("/screen HTTP", " HTTP"), 403, "no token"),
        (
            good.replace(&format!("Origin: http://{addr}"), "Origin: http://evil.example"),
            403,
            "not accepted",
        ),
        // A plain GET (not a WebSocket, so the token goes in its header).
        (
            good.replace(
                "Upgrade: websocket\r\n",
                &format!("X-Pendragon-Token: {}\r\n", api.token),
            ),
            426,
            "Upgrade: websocket",
        ),
        (good.replace("Version: 13", "Version: 8"), 426, "version 13"),
        (good.replace("GET ", "POST "), 405, "takes GET"),
        (
            handshake(addr, "spin", &format!("chat, pendragon.token.{}", api.token), ""),
            400,
            "pendragon.screen",
        ),
        (
            handshake(addr, "nosuch", &protocols(&api.token), ""),
            404,
            "no such VM",
        ),
    ] {
        let (got, head, mut page) = Page::try_open(addr, &request);
        assert_eq!(got, status, "{request}\n{head}");
        let mut body = std::mem::take(&mut page.buf);
        let _ = page.stream.read_to_end(&mut body);
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(says), "{status}: {body}");
        if status == 426 && says.contains("version") {
            assert!(head.contains("Sec-WebSocket-Version: 13\r\n"), "{head}");
        }
        if status == 426 && says.contains("Upgrade") {
            assert!(head.contains("Upgrade: websocket\r\n"), "{head}");
        }
    }
    // A client that can set headers may send the token in X-Pendragon-Token, and
    // offer no subprotocol; it then gets none.
    let request = handshake(addr, "spin", "", &format!("X-Pendragon-Token: {}\r\n", api.token))
        .replace("Sec-WebSocket-Protocol: \r\n", "");
    let (got, head, _) = Page::try_open(addr, &request);
    assert_eq!(got, 101, "{head}");
    assert!(!head.contains("Sec-WebSocket-Protocol"), "{head}");
    platform.stop();
}

/// A page that breaks the rules is closed with the code that says how: an unmasked
/// frame (1002), a message Pendragon does not know (1008), a binary one (1003), one
/// too large (1009).
#[test]
fn a_page_that_breaks_the_rules_is_closed() {
    let platform = Platform::start("rules", 64);
    let api = platform.api();
    create(&api, "ptr", &example("pointer"), false);
    let unmasked = [0x81, 0x02, b'{', b'}'].to_vec();
    let bad_json = client_frame(true, OP_TEXT, MASK, br#"{"type":"jump"}"#);
    let binary = client_frame(true, OP_BINARY, MASK, b"x");
    let large = client_frame(true, OP_TEXT, MASK, &[b' '; 5000]);
    for (frame, code) in [(unmasked, 1002), (bad_json, 1008), (binary, 1003), (large, 1009)] {
        let mut page = Page::open(&api, "ptr");
        page.stream.write_all(&frame).unwrap();
        let msgs = page.until("the close", |m| matches!(m, Msg::Close(..)));
        match msgs.last() {
            Some(Msg::Close(Some(got), why)) => assert_eq!(*got, code, "{why}"),
            other => panic!("{other:?}"),
        }
        page.send_frame(OP_CLOSE, &close_payload(code, ""));
        assert_eq!(page.recv(), None);
    }
    platform.stop();
}

/// A page that falls silent is pinged, and closed (1001) when it does not answer; one
/// that answers stays. Shutting Pendragon down closes the screens (1001 too). At most
/// `max_screens` are open (409).
#[test]
fn pings_limits_and_shutdown() {
    let mut platform = Platform::start("pings", 64);
    let config = pendragon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        avalon: platform.avalon.addr().to_string(),
        lake: platform.lake_addr(),
        screens: pendragon::ScreenLimits {
            max_screens: 2,
            ping: Duration::from_millis(300),
        },
        log: false,
        ..pendragon::Config::default()
    };
    let old = std::mem::replace(&mut platform.pendragon, pendragon_with(config));
    old.stop().unwrap();
    let api = platform.api();
    create(&api, "ptr", &example("pointer"), false);

    let mut silent = Page::open(&api, "ptr");
    // The answering page answers from the moment it opens, as a browser does, on a
    // thread of its own: its pings come while the test waits for the silent page's
    // close. Every message it reads comes to the test, the pings among them.
    let answering = Page::open(&api, "ptr");
    let (to_test, answered) = std::sync::mpsc::channel();
    let answerer = std::thread::spawn(move || {
        let mut page = answering;
        while let Some(msg) = page.recv() {
            match &msg {
                Msg::Ping(payload) => page.send_frame(OP_PONG, payload),
                Msg::Close(..) => {
                    page.send_frame(OP_CLOSE, &close_payload(1000, ""));
                    let _ = to_test.send(msg);
                    return;
                }
                _ => {}
            }
            let _ = to_test.send(msg);
        }
    });
    let next = || {
        answered
            .recv_timeout(SLOW)
            .expect("a message to the answering page")
    };
    let (status, _, mut third) =
        Page::try_open(api.addr, &handshake(api.addr, "ptr", &protocols(&api.token), ""));
    assert_eq!(status, 409);
    let mut body = std::mem::take(&mut third.buf);
    let _ = third.stream.read_to_end(&mut body);
    assert!(
        String::from_utf8_lossy(&body).contains(r#""code":"over budget""#),
        "{}",
        String::from_utf8_lossy(&body)
    );

    let msgs = silent.until("the close", |m| matches!(m, Msg::Close(..)));
    assert!(msgs.contains(&Msg::Ping(vec![])), "{msgs:?}");
    assert_eq!(
        msgs.last(),
        Some(&Msg::Close(Some(1001), "no answer to a ping".into()))
    );

    // The answering page outlives a few pings (it has had one at least by the time
    // the silent page was closed for not answering its own).
    let mut pings = 0;
    while pings < 3 {
        match next() {
            Msg::Ping(_) => pings += 1,
            other @ Msg::Close(..) => panic!("the answering page was closed: {other:?}"),
            _ => {}
        }
    }
    // A slot is free again: the silent page's.
    wait_for("the silent page's slot", || {
        (n(&api.expect(200, api.get("overview")), "screens") == 1).then_some(())
    });

    let pendragon = std::mem::replace(
        &mut platform.pendragon,
        pendragon_for(&dead_addr(), &dead_addr(), 64 * MIB),
    );
    pendragon.stop().unwrap();
    let mut msgs = Vec::new();
    while !matches!(msgs.last(), Some(Msg::Close(..))) {
        msgs.push(next());
    }
    answerer.join().unwrap();
    let end = msgs.iter().find_map(|m| match m {
        Msg::Text(json) if s(json, "type") == "end" => Some(s(json, "reason").to_string()),
        _ => None,
    });
    assert_eq!(end.as_deref(), Some("Pendragon is shutting down"));
    assert_eq!(
        msgs.last(),
        Some(&Msg::Close(Some(1001), "Pendragon is shutting down".into()))
    );
    platform.stop();
}

/// An Avalon that cannot be reached is a 502 before the upgrade.
#[test]
fn avalon_away() {
    let pendragon = pendragon_for(&hang_up_addr(), &dead_addr(), 64 * MIB);
    let api = Api::new(&pendragon);
    let (status, _, _) = Page::try_open(api.addr, &handshake(api.addr, "1", &protocols(&api.token), ""));
    assert_eq!(status, 502);
    pendragon.stop().unwrap();
}
