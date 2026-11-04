//! VM screens for the browser: Pendragon is the Avalon viewer of a VM's screen
//! (protocol doc, section 6.4) on behalf of a page, and relays it over a WebSocket
//! ([`crate::websocket`]), as Herald's `--screen` window shows it.
//!
//! Each page's WebSocket is one Avalon session of its own, attached to one VM's
//! screen (`ScreenAttach`), so Avalon keeps the keys and buttons of each page apart
//! and releases them when the page goes, as it does for any viewer. Two threads serve
//! it: the connection's own thread reads the page's messages (keys, pointer changes)
//! and sends them to Avalon, and a pump thread reads Avalon's frames and writes them
//! to the page.
//!
//! What goes to the page:
//!
//! - text messages, JSON: `{"type": "screen", "vm", "width", "height"}` first and on
//!   every mode change (`ScreenResized`: the page starts a black canvas of that size),
//!   `{"type": "state", "state", "state_text", "exit_code"}` with the VM's state, at
//!   the start and on every change, and `{"type": "end", "reason"}` before Pendragon
//!   closes the WebSocket (the VM stopped, exited or was destroyed, Avalon went away);
//! - binary messages, one per `ScreenFrame`: the rectangle's `x`, `y`, `width` and
//!   `height` (u32, little-endian), then its pixels as a canvas takes them
//!   (`ImageData`: R, G, B, A bytes, A 255), rows top to bottom.
//!
//! What comes from the page, text messages in JSON ([`PageInput`]): keys as the
//! browser reports them (`KeyboardEvent.code` and `.key`), pointer changes in screen
//! pixels with the buttons held, wheel turns, and a release of everything held when
//! the page loses the focus. [`Input`] turns them into `KeyInput` and `PointerInput`,
//! with the rules of Grail's window: a key's physical form is the HID usage of its
//! `code` (`grail::keyboard::usage::from_dom_code`), so the page has no key table; its
//! release carries what its press did; a key down again is the browser's auto-repeat.

use crate::api::state_word;
use crate::json::Json;
use crate::websocket::{
    close_payload, frame_header, Incoming, Reader, CLOSE_GOING_AWAY, CLOSE_INTERNAL_ERROR, CLOSE_NORMAL,
    CLOSE_POLICY, CLOSE_UNSUPPORTED, OP_BINARY, OP_CLOSE, OP_PING, OP_PONG, OP_TEXT,
};
use avalon_client::{Client, Timeouts};
use avalon_proto::{
    kind_of, AttachKind, Connection, Error, KeyEvent, Message, MessageKind, PointerChange, PointerMotion,
    Screen, VmRef, VmState, MAX_FRAME_LEN, PIXEL_FORMAT_XRGB8888, POINTER_BUTTONS, SCREEN_FRAME_OVERHEAD,
};
use grail::keyboard::{code, usage};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The subprotocol the page asks for (`Sec-WebSocket-Protocol`), and Pendragon
/// answers with: the messages above.
pub const SUBPROTOCOL: &str = "pendragon.screen";

/// Most screens open at once by default: each is a connection (of the
/// [`crate::MAX_CONNECTIONS`]), a session with Avalon and two threads.
pub const MAX_SCREENS: usize = 16;

/// By default, a page that has sent nothing for this long is pinged, and one that
/// has not answered for this long again is gone (its machine went to sleep, its
/// network went away): the screen is closed.
pub const PING: Duration = Duration::from_secs(20);

/// Most bytes of a message from the page: its messages are a few dozen bytes.
pub const MAX_MESSAGE: usize = 4096;

/// How long the page has to answer Pendragon's close frame before the connection is
/// closed anyway.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// How often the connection's thread looks up from reading the page: to close a
/// screen whose VM ended, ping, or shut down.
const POLL: Duration = Duration::from_millis(100);

/// How long writing one message to the page may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a detach waits for Avalon's answer before the session is closed anyway.
const DETACH_WAIT: Duration = Duration::from_secs(2);

/// Wheel pixels that make one notch, and the pixels a line counts for when the
/// browser reports lines (or pages): noVNC's `WHEEL_STEP` and `WHEEL_LINE_HEIGHT`,
/// which make one notch of every browser's wheel one notch.
pub const WHEEL_STEP: f64 = 50.0;
pub const WHEEL_LINE: f64 = 19.0;

/// How many screens Pendragon keeps open, and when it gives up on a silent page
/// ([`crate::Config::screens`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenLimits {
    /// Most screens open at once; more are refused (409, over budget).
    pub max_screens: usize,
    /// A page silent this long is pinged, and closed after as long again.
    pub ping: Duration,
}

impl Default for ScreenLimits {
    fn default() -> Self {
        ScreenLimits {
            max_screens: MAX_SCREENS,
            ping: PING,
        }
    }
}

// ---- What the page sends ----

/// One message from the page.
#[derive(Debug, Clone, PartialEq)]
pub enum PageInput {
    /// A key went down or up: `KeyboardEvent.code` (the physical key), `.key` (what
    /// it types in the person's layout) and the modifiers held.
    Key {
        down: bool,
        code: String,
        key: String,
        shift: bool,
        ctrl: bool,
        alt: bool,
    },
    /// The pointer moved to (`x`, `y`) in screen pixels (outside the screen while a
    /// drag has left it), or (`at` none) only the buttons changed; `buttons` are those
    /// held now, `PointerEvent.buttons` (bit 0 left, bit 1 right, bit 2 middle, the
    /// protocol's own bits).
    Pointer { at: Option<(i64, i64)>, buttons: u8 },
    /// The wheel turned: `WheelEvent.deltaY` (positive towards the person) in units
    /// of `deltaMode` (0 pixels, 1 lines, 2 pages).
    Wheel { dy: f64, mode: u8 },
    /// The page lost the focus: let go of every key and button held.
    Release,
}

fn only(msg: &Json, keys: &[&str]) -> Result<(), String> {
    match msg {
        Json::Object(members) => match members.iter().find(|(k, _)| !keys.contains(&k.as_str())) {
            Some((key, _)) => Err(format!("unknown field '{key}'")),
            None => Ok(()),
        },
        _ => Err("not a JSON object".into()),
    }
}

fn text<'a>(msg: &'a Json, key: &str) -> Result<&'a str, String> {
    msg.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{key}: not a string"))
}

fn boolean(msg: &Json, key: &str) -> Result<bool, String> {
    msg.get(key)
        .and_then(Json::as_bool)
        .ok_or_else(|| format!("{key}: not true or false"))
}

impl PageInput {
    /// Parses a message from the page. Every field is required, and no other may be
    /// there: the page and Pendragon are built together, so anything else is a bug.
    pub fn parse(message: &str) -> Result<PageInput, String> {
        let msg = Json::parse(message)?;
        let kind = msg.get("type").and_then(Json::as_str).unwrap_or_default();
        match kind {
            "key" => {
                only(&msg, &["type", "down", "code", "key", "shift", "ctrl", "alt"])?;
                Ok(PageInput::Key {
                    down: boolean(&msg, "down")?,
                    code: text(&msg, "code")?.to_string(),
                    key: text(&msg, "key")?.to_string(),
                    shift: boolean(&msg, "shift")?,
                    ctrl: boolean(&msg, "ctrl")?,
                    alt: boolean(&msg, "alt")?,
                })
            }
            "pointer" => {
                only(&msg, &["type", "x", "y", "buttons"])?;
                let coord = |key| {
                    msg.get(key)
                        .map(|v| v.as_i64().ok_or(format!("{key}: not a whole number")))
                };
                let at = match (coord("x"), coord("y")) {
                    (Some(x), Some(y)) => Some((x?, y?)),
                    (None, None) => None,
                    _ => return Err("x and y come together".into()),
                };
                let buttons = msg
                    .get("buttons")
                    .and_then(Json::as_u64)
                    .filter(|b| *b <= 0xff)
                    .ok_or("buttons: not a number from 0 to 255")?;
                Ok(PageInput::Pointer {
                    at,
                    buttons: buttons as u8,
                })
            }
            "wheel" => {
                only(&msg, &["type", "dy", "mode"])?;
                let dy = msg.get("dy").and_then(Json::as_f64).ok_or("dy: not a number")?;
                let mode = msg
                    .get("mode")
                    .and_then(Json::as_u64)
                    .filter(|m| *m <= 2)
                    .ok_or("mode: not 0, 1 or 2")?;
                Ok(PageInput::Wheel { dy, mode: mode as u8 })
            }
            "release" => {
                only(&msg, &["type"])?;
                Ok(PageInput::Release)
            }
            other => Err(format!("unknown message type '{other}'")),
        }
    }
}

/// ASCII mode's key code for a key the browser reports as `KeyboardEvent.key` `key`
/// (the spec's table, section 9.4): a printable ASCII character is itself, the keys of
/// the table their codes, anything else 0 (a modifier alone, Home, a dead key, a
/// character outside ASCII). Browsers report the character without Ctrl's effect
/// (Ctrl+C is "c"), as Grail's window does, and the one AltGr types.
pub fn ascii_code(key: &str) -> u8 {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return if (' '..='~').contains(&c) { c as u8 } else { 0 };
    }
    match key {
        "Enter" => code::ENTER,
        "Tab" => code::TAB,
        "Backspace" => code::BACKSPACE,
        "Escape" => code::ESC,
        "ArrowUp" => code::UP,
        "ArrowDown" => code::DOWN,
        "ArrowLeft" => code::LEFT,
        "ArrowRight" => code::RIGHT,
        _ => match key.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
            // "F1" to "F12" (not "F01").
            Some(n @ 1..=12) if !key[1..].starts_with('0') => code::F1 + n - 1,
            _ => 0,
        },
    }
}

/// Whether the key of usage `u` is one a guest sees pressed and released at once, as
/// in Grail's window: Lang1 and Lang2 (Hangul and Hanja) are make-only keys.
fn taps(u: u16) -> bool {
    matches!(u, usage::LANG1 | usage::LANG2)
}

/// A key the page has reported down.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    /// Which key: its `KeyboardEvent.code`, or its `.key` for one with no code.
    id: String,
    /// The two forms its press had, which its release carries too.
    code: u8,
    usage: u16,
    /// Reported as a press and a release at once: its key-up reports nothing.
    tapped: bool,
}

/// The page's keys and pointer, turned into the protocol's messages. Keeps what the
/// page holds down, so a release carries what its press did and a lost focus lets go
/// of everything, as Grail's window does.
#[derive(Debug, Default)]
pub struct Input {
    held: Vec<Held>,
    /// The buttons of the last pointer change sent.
    buttons: u8,
    /// Wheel pixels turned and not yet a notch: negative away from the person.
    wheel: f64,
}

impl Input {
    pub fn new() -> Input {
        Input::default()
    }

    /// The messages for `input`, to VM `vm` whose screen is `width` × `height`.
    pub fn messages(&mut self, vm: u32, input: PageInput, width: u32, height: u32) -> Vec<Message> {
        let keys = |keys: Vec<KeyEvent>| {
            keys.into_iter()
                .map(|key| Message::KeyInput { vm, key })
                .collect()
        };
        let pointer = |p: Option<PointerChange>| {
            p.into_iter()
                .map(|pointer| Message::PointerInput { vm, pointer })
                .collect()
        };
        match input {
            PageInput::Key {
                down,
                code,
                key,
                shift,
                ctrl,
                alt,
            } => {
                // Ctrl+Alt is AltGr on Windows: the character it types is the key, not
                // a Ctrl chord (Grail's window says the same).
                keys(self.key(down, &code, &key, shift, ctrl && !alt))
            }
            PageInput::Pointer { at, buttons } => pointer(self.pointer(at, buttons, width, height)),
            PageInput::Wheel { dy, mode } => pointer(self.wheel(dy, mode)),
            PageInput::Release => {
                let mut out: Vec<Message> = keys(self.release_keys());
                out.extend(pointer(self.release_buttons()));
                out
            }
        }
    }

    /// A key went down or up: `dom` is its `KeyboardEvent.code`, `key` its `.key`.
    pub fn key(&mut self, down: bool, dom: &str, key: &str, shift: bool, ctrl: bool) -> Vec<KeyEvent> {
        let usage = usage::from_dom_code(dom).unwrap_or(0);
        let id = if dom.is_empty() || dom == "Unidentified" {
            format!("key {key}")
        } else {
            dom.to_string()
        };
        let event = |held: &Held, release: bool, repeat: bool| KeyEvent {
            code: held.code,
            usage: held.usage,
            release,
            repeat,
            shift,
            ctrl,
        };
        let tap = |code: u8, usage: u16| {
            let press = KeyEvent {
                code,
                usage,
                release: false,
                repeat: false,
                shift,
                ctrl,
            };
            vec![
                press,
                KeyEvent {
                    release: true,
                    ..press
                },
            ]
        };
        match (down, self.held.iter().position(|h| h.id == id)) {
            // Nothing more for a tapped key until it is up again.
            (true, Some(i)) if self.held[i].tapped => vec![],
            (false, Some(i)) if self.held[i].tapped => {
                self.held.remove(i);
                vec![]
            }
            // Down again before it was up: the browser's auto-repeat, another press of
            // what the key got when it first went down.
            (true, Some(i)) => vec![event(&self.held[i], false, true)],
            (true, None) => {
                let held = Held {
                    id,
                    code: ascii_code(key),
                    usage,
                    tapped: taps(usage),
                };
                if held.code == 0 && held.usage == 0 {
                    return vec![]; // nothing to report in either mode
                }
                let out = if held.tapped {
                    tap(held.code, held.usage)
                } else {
                    vec![event(&held, false, false)]
                };
                self.held.push(held);
                out
            }
            (false, Some(i)) => {
                let held = self.held.remove(i);
                vec![event(&held, true, false)]
            }
            (false, None) => {
                // Windows keeps Print Screen's key-down for its screenshot hotkey, so a
                // browser there sees its key-up alone: a press and a release, as in
                // Grail's window. So is a make-only key whose event is a key-up.
                if usage == usage::PRINT_SCREEN || taps(usage) {
                    tap(0, usage)
                } else {
                    // Held when the page got the focus: the guest never saw it go down.
                    vec![]
                }
            }
        }
    }

    /// Releases every key the page reported down, modifiers included.
    pub fn release_keys(&mut self) -> Vec<KeyEvent> {
        self.held
            .drain(..)
            .filter(|h| !h.tapped)
            .map(|h| KeyEvent {
                code: h.code,
                usage: h.usage,
                release: true,
                repeat: false,
                shift: false,
                ctrl: false,
            })
            .collect()
    }

    /// The pointer moved to `at` (none: it did not), with `buttons` held. Positions
    /// past the screen's edges, which a drag that left the canvas reports, go out
    /// clamped to them.
    pub fn pointer(
        &mut self,
        at: Option<(i64, i64)>,
        buttons: u8,
        width: u32,
        height: u32,
    ) -> Option<PointerChange> {
        let buttons = buttons & POINTER_BUTTONS;
        if at.is_none() && buttons == self.buttons {
            return None;
        }
        let clamp = |v: i64, size: u32| v.clamp(0, size.saturating_sub(1) as i64) as u32;
        self.buttons = buttons;
        Some(PointerChange {
            motion: match at {
                Some((x, y)) => PointerMotion::To {
                    x: clamp(x, width),
                    y: clamp(y, height),
                },
                None => PointerMotion::None,
            },
            buttons,
            wheel: 0,
        })
    }

    /// The wheel turned `dy` in `mode`'s units. As noVNC does it: lines and pages
    /// count [`WHEEL_LINE`] pixels each, the pixels add up until they make
    /// [`WHEEL_STEP`], and then go out as one notch, the rest dropped. A turn the
    /// other way starts afresh, as in Grail's window.
    pub fn wheel(&mut self, dy: f64, mode: u8) -> Option<PointerChange> {
        // The protocol's wheel is positive away from the person, the browser's
        // deltaY towards.
        let turn = -if mode == 0 { dy } else { dy * WHEEL_LINE };
        if turn != 0.0 && (self.wheel < 0.0) != (turn < 0.0) {
            self.wheel = 0.0;
        }
        self.wheel += turn;
        if self.wheel.abs() < WHEEL_STEP {
            return None;
        }
        let notch = if self.wheel > 0.0 { 1 } else { -1 };
        self.wheel = 0.0;
        Some(PointerChange {
            motion: PointerMotion::None,
            buttons: self.buttons,
            wheel: notch,
        })
    }

    /// Releases the buttons held, if any.
    pub fn release_buttons(&mut self) -> Option<PointerChange> {
        self.pointer(None, 0, 1, 1)
    }
}

// ---- What goes to the page ----

/// `{"type": "screen", ...}`: the screen is `width` × `height`, black.
fn screen_json(vm: u32, width: u32, height: u32) -> String {
    Json::object([
        ("type", "screen".into()),
        ("vm", vm.into()),
        ("width", width.into()),
        ("height", height.into()),
    ])
    .encode()
}

/// `{"type": "state", ...}`: the VM's state, in the API's words.
fn state_json(state: VmState) -> String {
    Json::object([
        ("type", "state".into()),
        ("state", state_word(state).into()),
        ("state_text", state.to_string().into()),
        (
            "exit_code",
            match state {
                VmState::Exited(code) => Some(code),
                _ => None,
            }
            .into(),
        ),
    ])
    .encode()
}

fn end_json(reason: &str) -> String {
    Json::object([("type", "end".into()), ("reason", reason.into())]).encode()
}

/// The WebSocket frame that carries a `ScreenFrame`'s rectangle (`x`, `y`, `width`,
/// `height`, its XRGB8888 `pixels`) to the page: the rectangle, then the pixels in
/// the canvas's RGBA order.
pub fn frame_message(x: u32, y: u32, width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let len = 16 + pixels.len();
    let mut out = frame_header(OP_BINARY, len);
    out.reserve(len);
    for v in [x, y, width, height] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // XRGB8888 is the word 0x00RRGGBB, little-endian: B, G, R, X in memory.
    for px in pixels.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 0xff]);
    }
    out
}

/// Whether Pendragon can relay a `width` × `height` screen in `format`: XRGB8888, not
/// empty, and a full frame of it fits in a protocol frame (as Herald's `check_screen`).
fn check_screen(width: u32, height: u32, format: u32) -> Result<(), String> {
    let room = MAX_FRAME_LEN as usize - SCREEN_FRAME_OVERHEAD;
    if format != PIXEL_FORMAT_XRGB8888 || !Screen::byte_len(width, height).is_some_and(|n| n <= room) {
        return Err(format!("a {width}x{height} screen in pixel format {format}"));
    }
    Ok(())
}

/// The page's end of the WebSocket, for both threads: messages go out whole, one at a
/// time, and nothing after the close frame (RFC 6455 section 5.5.1). A write that
/// fails may have left part of a frame on the wire, after which no byte means what
/// it says: the page counts as closed then too, and gets nothing more, not even the
/// close frame (the connection is failed, section 7.1.7).
struct Page {
    stream: Mutex<(TcpStream, bool)>,
}

impl Page {
    fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut guard = crate::console::lock(&self.stream);
        let (stream, closed) = &mut *guard;
        if *closed {
            return Ok(());
        }
        let sent = stream.write_all(frame).and_then(|()| stream.flush());
        if sent.is_err() {
            *closed = true;
        }
        sent
    }

    fn message(&self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let mut frame = frame_header(opcode, payload.len());
        frame.extend_from_slice(payload);
        self.send(&frame)
    }

    fn text(&self, text: &str) -> io::Result<()> {
        self.message(OP_TEXT, text.as_bytes())
    }

    /// Sends the close frame, once. Returns whether this call sent it.
    fn close(&self, code: Option<u16>, reason: &str) -> bool {
        let mut guard = crate::console::lock(&self.stream);
        let (stream, closed) = &mut *guard;
        if std::mem::replace(closed, true) {
            return false;
        }
        let payload = code.map(|c| close_payload(c, reason)).unwrap_or_default();
        let mut frame = frame_header(OP_CLOSE, payload.len());
        frame.extend_from_slice(&payload);
        let _ = stream.write_all(&frame).and_then(|()| stream.flush());
        true
    }
}

/// What the two threads of a screen share.
struct Shared {
    vm: u32,
    page: Page,
    /// The screen's size now (it changes with the guest's mode), for pointer input.
    size: Mutex<(u32, u32)>,
    /// Set by the pump when the session with Avalon has ended.
    ended: AtomicBool,
    /// Set by the connection's thread once it has asked Avalon to detach.
    detaching: AtomicBool,
}

/// Every open screen.
pub struct Screens {
    avalon: String,
    timeouts: Timeouts,
    limits: ScreenLimits,
    /// How many screens are open.
    open: Arc<Mutex<usize>>,
    /// Set when Pendragon shuts down: open screens close, new ones are refused.
    closed: Arc<AtomicBool>,
}

/// One of the [`ScreenLimits::max_screens`], given back when dropped.
struct Slot(Arc<Mutex<usize>>);

impl Drop for Slot {
    fn drop(&mut self) {
        *crate::console::lock(&self.0) -= 1;
    }
}

/// Why a screen could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// [`ScreenLimits::max_screens`] are open, or Pendragon is shutting down.
    Full(String),
    /// Avalon refused, or could not be reached.
    Avalon(avalon_client::Error),
    /// Avalon's screen is one Pendragon cannot relay.
    Screen(String),
}

/// A screen attached on Avalon, its WebSocket not yet answered: what
/// [`Screens::open`] returns, for [`Viewer::run`].
pub struct Viewer {
    _slot: Slot,
    conn: Connection<TcpStream>,
    vm: u32,
    width: u32,
    height: u32,
    state: VmState,
    ping: Duration,
    closed: Arc<AtomicBool>,
}

impl Screens {
    pub fn new(avalon: &str, timeouts: Timeouts, limits: ScreenLimits) -> Screens {
        Screens {
            avalon: avalon.to_string(),
            timeouts,
            limits,
            open: Arc::new(Mutex::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// How many screens are open.
    pub fn open_count(&self) -> usize {
        *crate::console::lock(&self.open)
    }

    /// Attaches to `vm`'s screen on Avalon, for a page whose WebSocket handshake has
    /// passed: the screen's size and the VM's state, the full frame queued after them.
    pub fn open(&self, vm: &VmRef) -> Result<Viewer, OpenError> {
        let slot = {
            let mut open = crate::console::lock(&self.open);
            if self.closed.load(Ordering::SeqCst) {
                return Err(OpenError::Full("Pendragon is shutting down".into()));
            }
            if *open >= self.limits.max_screens {
                return Err(OpenError::Full(format!(
                    "{} screens are open already, the most Pendragon keeps",
                    self.limits.max_screens
                )));
            }
            *open += 1;
            Slot(Arc::clone(&self.open))
        };
        // The slot is given back if attaching fails, as it is when the screen closes.
        let (conn, vm, width, height, state) = self.attach(vm)?;
        Ok(Viewer {
            _slot: slot,
            conn,
            vm,
            width,
            height,
            state,
            ping: self.limits.ping,
            closed: Arc::clone(&self.closed),
        })
    }

    #[allow(clippy::type_complexity)]
    fn attach(&self, vm: &VmRef) -> Result<(Connection<TcpStream>, u32, u32, u32, VmState), OpenError> {
        let mut client =
            Client::connect_with(&self.avalon, crate::AGENT, self.timeouts).map_err(OpenError::Avalon)?;
        let request = |client: &mut Client, msg: &Message| {
            client
                .connection()
                .request(msg)
                .map_err(|e| OpenError::Avalon(avalon_client::Error::Request(e)))
        };
        let (id, width, height) = match request(&mut client, &Message::ScreenAttach { vm: vm.clone() })? {
            Message::ScreenAttached {
                vm,
                width,
                height,
                format,
            } => {
                check_screen(width, height, format).map_err(OpenError::Screen)?;
                (vm, width, height)
            }
            other => {
                return Err(OpenError::Avalon(avalon_client::Error::Request(
                    Error::Unexpected(other.type_code()),
                )))
            }
        };
        let state = match request(&mut client, &Message::GetVmInfo { vm: VmRef::Id(id) })? {
            Message::VmInfo(details) => details.vm.state,
            other => {
                return Err(OpenError::Avalon(avalon_client::Error::Request(
                    Error::Unexpected(other.type_code()),
                )))
            }
        };
        Ok((client.into_connection(), id, width, height, state))
    }

    /// Closes every open screen (each at its next look, within [`POLL`]) and refuses
    /// new ones: Pendragon is shutting down.
    pub fn close_all(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

impl Viewer {
    /// Serves the page on `ws`, the connection whose handshake was just answered;
    /// `rest` is what the page sent after its handshake. Returns when the screen is
    /// closed, by the page, by the VM's end or by Pendragon's.
    pub fn run(self, ws: &TcpStream, rest: Vec<u8>) {
        let Viewer {
            _slot,
            conn,
            vm,
            width,
            height,
            state,
            ping,
            closed,
        } = self;
        let _ = serve(conn, ws, rest, vm, (width, height), state, ping, &closed);
        // The slot goes back as `_slot` drops here.
    }
}

/// What the connection's thread does; errors writing to the page end it.
#[allow(clippy::too_many_arguments)]
fn serve(
    conn: Connection<TcpStream>,
    ws: &TcpStream,
    rest: Vec<u8>,
    vm: u32,
    (width, height): (u32, u32),
    state: VmState,
    ping: Duration,
    closing_down: &AtomicBool,
) -> io::Result<()> {
    let page = ws.try_clone()?;
    // Set on this handle too: on Windows a cloned socket handle keeps timeouts of its
    // own. A page that takes no frame for this long is given up on.
    page.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let shared = Arc::new(Shared {
        vm,
        page: Page {
            stream: Mutex::new((page, false)),
        },
        size: Mutex::new((width, height)),
        ended: AtomicBool::new(false),
        detaching: AtomicBool::new(false),
    });
    let mut to_avalon = conn.try_clone().map_err(io::Error::other)?;
    let avalon_socket = conn.get_ref().try_clone()?;
    ws.set_read_timeout(Some(POLL))?;
    shared.page.text(&screen_json(vm, width, height))?;
    shared.page.text(&state_json(state))?;
    let pump = {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name(format!("screen-{vm}"))
            .spawn(move || pump(conn, &shared, state))?
    };

    // From here on nothing returns early: the pump must be stopped and joined.
    let mut reader = Reader::new(MAX_MESSAGE);
    reader.push(&rest);
    let mut input = Input::new();
    let mut heard = Instant::now();
    let mut pinged: Option<Instant> = None;
    // When Pendragon sent its close frame: the page has CLOSE_WAIT to answer.
    let mut closing: Option<Instant> = None;
    let mut buf = vec![0u8; 4096];
    loop {
        match reader.message() {
            Ok(Some(message)) => {
                heard = Instant::now();
                pinged = None;
                match message {
                    Incoming::Text(text) if closing.is_none() => match PageInput::parse(&text) {
                        Ok(page_input) => {
                            let (w, h) = *crate::console::lock(&shared.size);
                            for msg in input.messages(vm, page_input, w, h) {
                                // Avalon gone: the pump sees it and ends the screen.
                                let _ = to_avalon.send(&msg);
                            }
                        }
                        Err(why) => {
                            shared
                                .page
                                .close(Some(CLOSE_POLICY), &format!("bad message: {why}"));
                            closing = Some(Instant::now());
                        }
                    },
                    Incoming::Binary(_) if closing.is_none() => {
                        shared
                            .page
                            .close(Some(CLOSE_UNSUPPORTED), "Pendragon takes text messages");
                        closing = Some(Instant::now());
                    }
                    Incoming::Text(_) | Incoming::Binary(_) => {}
                    Incoming::Ping(payload) => {
                        if shared.page.message(OP_PONG, &payload).is_err() {
                            break;
                        }
                    }
                    Incoming::Pong(_) => {}
                    // The page closes: answer with its code (section 5.5.1), and the
                    // connection ends. A close answering ours ends it too.
                    Incoming::Close(code, _) => {
                        shared.page.close(code, "");
                        break;
                    }
                }
                continue;
            }
            Ok(None) => {}
            Err(violation) => {
                shared.page.close(Some(violation.code), &violation.reason);
                break;
            }
        }
        if closing.is_none() {
            if shared.ended.load(Ordering::SeqCst) {
                // The pump has sent the end and the close frame.
                closing = Some(Instant::now());
            } else if closing_down.load(Ordering::SeqCst) {
                let _ = shared.page.text(&end_json("Pendragon is shutting down"));
                shared
                    .page
                    .close(Some(CLOSE_GOING_AWAY), "Pendragon is shutting down");
                closing = Some(Instant::now());
            } else if let Some(at) = pinged {
                if at.elapsed() >= ping {
                    shared.page.close(Some(CLOSE_GOING_AWAY), "no answer to a ping");
                    break;
                }
            } else if heard.elapsed() >= ping {
                if shared.page.message(OP_PING, b"").is_err() {
                    break;
                }
                pinged = Some(Instant::now());
            }
        }
        if closing.is_some_and(|at| at.elapsed() >= CLOSE_WAIT) {
            break;
        }
        match (&mut &*ws).read(&mut buf) {
            Ok(0) => break,
            Ok(n) => reader.push(&buf[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }

    // The page is gone: let go of the VM's screen. Avalon releases what this page
    // held (protocol doc, section 6.4, Input), whether it answers the detach or only
    // sees the connection close.
    if !shared.ended.load(Ordering::SeqCst) {
        shared.detaching.store(true, Ordering::SeqCst);
        let _ = to_avalon.send(&Message::ScreenDetach { vm });
        let deadline = Instant::now() + DETACH_WAIT;
        while !pump.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let _ = avalon_socket.shutdown(Shutdown::Both);
    if !pump.is_finished() {
        // Stuck writing a frame to a page that reads no more.
        let _ = ws.shutdown(Shutdown::Both);
    }
    let _ = pump.join();
    Ok(())
}

/// The pump thread: Avalon's frames to the page, until the session ends.
fn pump(mut conn: Connection<TcpStream>, shared: &Shared, state: VmState) {
    if let Some((code, reason)) = pump_until_end(&mut conn, shared, state) {
        let _ = shared.page.text(&end_json(&reason));
        shared.page.close(Some(code), &reason);
    }
    shared.ended.store(true, Ordering::SeqCst);
}

/// Relays frames until the session ends: the close code and reason to give the page,
/// or none when the page went first (the connection's thread detached).
fn pump_until_end(
    conn: &mut Connection<TcpStream>,
    shared: &Shared,
    state: VmState,
) -> Option<(u16, String)> {
    // The pump waits for frames as long as there are none: the VM may draw nothing
    // for hours. Set on the very handle it reads (on Windows a cloned handle keeps
    // timeouts of its own).
    if conn.get_ref().set_read_timeout(None).is_err() {
        return Some((CLOSE_INTERNAL_ERROR, "cannot read from Avalon".into()));
    }
    // Events read while attaching first (the full frame), then the stream.
    while conn.queued_events() > 0 {
        if let Ok(event) = conn.recv() {
            match on_server(shared, event.type_code(), Ok(event)) {
                Step::Go => {}
                Step::End(end) => return end,
            }
        }
    }
    if ended(state) {
        return Some((CLOSE_NORMAL, format!("the VM has {}", ended_text(state))));
    }
    loop {
        let step = match conn.recv_frame() {
            Ok(frame) => on_server(shared, frame.ty, frame.message()),
            Err(_) if shared.detaching.load(Ordering::SeqCst) => return None,
            Err(e) if e.is_fatal() => {
                return Some((
                    CLOSE_INTERNAL_ERROR,
                    format!("the connection to Avalon ended ({e})"),
                ))
            }
            Err(_) => Step::Go,
        };
        if let Step::End(end) = step {
            return end;
        }
    }
}

enum Step {
    Go,
    End(Option<(u16, String)>),
}

fn ended(state: VmState) -> bool {
    matches!(state, VmState::Stopped | VmState::Exited(_))
}

fn ended_text(state: VmState) -> String {
    match state {
        VmState::Exited(code) => format!("exited (code {code})"),
        _ => "stopped".into(),
    }
}

/// Handles a frame of type `ty` from Avalon, decoded as `msg`, as Herald's screen
/// session does (`ScreenSession::on_server`): the only request this session sends
/// is `ScreenDetach`, so a reply is its answer.
fn on_server(shared: &Shared, ty: u16, msg: Result<Message, Error>) -> Step {
    let vm = shared.vm;
    let broken = |why: String| Step::End(Some((CLOSE_INTERNAL_ERROR, format!("Avalon sent {why}"))));
    let to_page = |result: io::Result<()>| match result {
        Ok(()) => Step::Go,
        // The page reads no more: the connection's thread sees it go.
        Err(_) => Step::End(None),
    };
    match (kind_of(ty), msg) {
        (
            Some(MessageKind::Event),
            Ok(Message::ScreenFrame {
                vm: v,
                x,
                y,
                width,
                height,
                pixels,
            }),
        ) if v == vm => {
            let (w, h) = *crate::console::lock(&shared.size);
            let inside =
                x.checked_add(width).is_some_and(|r| r <= w) && y.checked_add(height).is_some_and(|b| b <= h);
            if !inside || Screen::byte_len(width, height) != Some(pixels.len()) {
                return broken(format!("a frame outside the {w}x{h} screen"));
            }
            to_page(shared.page.send(&frame_message(x, y, width, height, &pixels)))
        }
        (
            Some(MessageKind::Event),
            Ok(Message::ScreenResized {
                vm: v,
                width,
                height,
                format,
            }),
        ) if v == vm => {
            if let Err(why) = check_screen(width, height, format) {
                return broken(why);
            }
            *crate::console::lock(&shared.size) = (width, height);
            to_page(shared.page.text(&screen_json(vm, width, height)))
        }
        (Some(MessageKind::Event), Ok(Message::VmStateChanged { vm: v, state })) if v == vm => {
            if let Step::End(end) = to_page(shared.page.text(&state_json(state))) {
                return Step::End(end);
            }
            if ended(state) {
                return Step::End(Some((CLOSE_NORMAL, format!("the VM has {}", ended_text(state)))));
            }
            Step::Go
        }
        (
            Some(MessageKind::Event),
            Ok(Message::Detached {
                vm: v,
                kind: AttachKind::Screen,
            }),
        ) if v == vm => Step::End(Some((CLOSE_NORMAL, "the VM was destroyed".into()))),
        // Other events, and events that do not decode: passed over.
        (Some(MessageKind::Event), _) => Step::Go,
        // The answer to the detach the connection's thread sent.
        (Some(MessageKind::Reply), _) if shared.detaching.load(Ordering::SeqCst) => Step::End(None),
        (Some(MessageKind::Reply), _) => broken(format!("a reply ({ty:#06x}) nobody asked for")),
        (Some(MessageKind::Request) | None, _) => broken(format!("a frame of type {ty:#06x}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avalon_proto::{POINTER_LEFT, POINTER_RIGHT};

    fn key(down: bool, code: &str, key: &str) -> PageInput {
        PageInput::Key {
            down,
            code: code.into(),
            key: key.into(),
            shift: false,
            ctrl: false,
            alt: false,
        }
    }

    #[test]
    fn page_messages() {
        assert_eq!(
            PageInput::parse(
                r#"{"type":"key","down":true,"code":"KeyA","key":"a","shift":true,"ctrl":false,"alt":false}"#
            ),
            Ok(PageInput::Key {
                down: true,
                code: "KeyA".into(),
                key: "a".into(),
                shift: true,
                ctrl: false,
                alt: false
            })
        );
        assert_eq!(
            PageInput::parse(r#"{"type":"pointer","x":-3,"y":700,"buttons":5}"#),
            Ok(PageInput::Pointer {
                at: Some((-3, 700)),
                buttons: 5
            })
        );
        assert_eq!(
            PageInput::parse(r#"{"type":"pointer","buttons":0}"#),
            Ok(PageInput::Pointer { at: None, buttons: 0 })
        );
        assert_eq!(
            PageInput::parse(r#"{"type":"wheel","dy":-33.5,"mode":0}"#),
            Ok(PageInput::Wheel { dy: -33.5, mode: 0 })
        );
        assert_eq!(PageInput::parse(r#"{"type":"release"}"#), Ok(PageInput::Release));
        for bad in [
            "",
            "[]",
            r#"{"type":"jump"}"#,
            r#"{"type":"release","now":true}"#,
            r#"{"type":"key","down":true,"code":"KeyA","key":"a","shift":false,"ctrl":false}"#,
            r#"{"type":"key","down":1,"code":"KeyA","key":"a","shift":false,"ctrl":false,"alt":false}"#,
            r#"{"type":"pointer","x":1,"buttons":0}"#,
            r#"{"type":"pointer","x":1.5,"y":2,"buttons":0}"#,
            r#"{"type":"pointer","x":1,"y":2,"buttons":256}"#,
            r#"{"type":"pointer","x":1,"y":2}"#,
            r#"{"type":"wheel","dy":1e999,"mode":0}"#,
            r#"{"type":"wheel","dy":1,"mode":3}"#,
        ] {
            assert!(PageInput::parse(bad).is_err(), "{bad}");
        }
    }

    /// `KeyboardEvent.key` values to ASCII mode's codes (UI Events, "KeyboardEvent key
    /// Values"): printable characters, the named keys of the spec's table, nothing
    /// for the rest.
    #[test]
    fn ascii_codes() {
        for (key, want) in [
            ("a", b'a'),
            ("A", b'A'),
            (" ", b' '),
            ("~", b'~'),
            ("Enter", code::ENTER),
            ("Tab", code::TAB),
            ("Backspace", code::BACKSPACE),
            ("Escape", code::ESC),
            ("ArrowUp", code::UP),
            ("ArrowDown", code::DOWN),
            ("ArrowLeft", code::LEFT),
            ("ArrowRight", code::RIGHT),
            ("F1", code::F1),
            ("F12", code::F12),
            ("F13", 0),
            ("F0", 0),
            ("F01", 0),
            ("Shift", 0),
            ("Home", 0),
            ("Dead", 0),
            ("é", 0),
            ("\u{7f}", 0),
            ("Unidentified", 0),
        ] {
            assert_eq!(ascii_code(key), want, "{key}");
        }
    }

    fn press(code: u8, usage: u16) -> KeyEvent {
        KeyEvent {
            code,
            usage,
            release: false,
            repeat: false,
            shift: false,
            ctrl: false,
        }
    }

    fn release(code: u8, usage: u16) -> KeyEvent {
        KeyEvent {
            release: true,
            ..press(code, usage)
        }
    }

    fn keys(input: &mut Input, page: PageInput) -> Vec<KeyEvent> {
        input
            .messages(7, page, 640, 480)
            .into_iter()
            .map(|m| match m {
                Message::KeyInput { vm: 7, key } => key,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// Both forms of each key, the release with what the press had, auto-repeat, and
    /// keys that are only one of the forms.
    #[test]
    fn keys_carry_both_forms() {
        let mut input = Input::new();
        assert_eq!(keys(&mut input, key(true, "KeyQ", "q")), [press(b'q', 0x14)]);
        // Shift went down meanwhile: the release is still the 'q' of the press.
        assert_eq!(keys(&mut input, key(false, "KeyQ", "Q")), [release(b'q', 0x14)]);
        assert_eq!(
            keys(&mut input, key(true, "ShiftLeft", "Shift")),
            [press(0, usage::LEFT_SHIFT)]
        );
        let repeat = KeyEvent {
            repeat: true,
            ..press(0, usage::LEFT_SHIFT)
        };
        assert_eq!(keys(&mut input, key(true, "ShiftLeft", "Shift")), [repeat]);
        assert_eq!(
            keys(&mut input, key(false, "ShiftLeft", "Shift")),
            [release(0, usage::LEFT_SHIFT)]
        );
        // The keypad's Enter and the main one are two keys, with one code.
        assert_eq!(
            keys(&mut input, key(true, "NumpadEnter", "Enter")),
            [press(code::ENTER, usage::KP_ENTER)]
        );
        assert_eq!(
            keys(&mut input, key(true, "Enter", "Enter")),
            [press(code::ENTER, usage::ENTER)]
        );
        assert_eq!(
            keys(&mut input, key(false, "Enter", "Enter")),
            [release(code::ENTER, usage::ENTER)]
        );
        assert_eq!(
            keys(&mut input, key(false, "NumpadEnter", "Enter")),
            [release(code::ENTER, usage::KP_ENTER)]
        );
        // A key the browser cannot place: its character alone, matched by it.
        assert_eq!(keys(&mut input, key(true, "", "x")), [press(b'x', 0)]);
        assert_eq!(keys(&mut input, key(false, "", "x")), [release(b'x', 0)]);
        // A key with neither form, and a release of a key never seen down.
        assert_eq!(keys(&mut input, key(true, "Unidentified", "Unidentified")), []);
        assert_eq!(keys(&mut input, key(false, "KeyZ", "z")), []);
        // Old names for the Windows key.
        assert_eq!(
            keys(&mut input, key(true, "OSLeft", "Meta")),
            [press(0, usage::LEFT_GUI)]
        );
        assert_eq!(
            keys(&mut input, key(false, "OSLeft", "Meta")),
            [release(0, usage::LEFT_GUI)]
        );
    }

    /// Shift and Ctrl go with each key for ASCII mode; Ctrl with Alt is AltGr, a
    /// character, not a Ctrl chord.
    #[test]
    fn modifiers() {
        let mut input = Input::new();
        let chord = |down, code: &str, k: &str, shift, ctrl, alt| PageInput::Key {
            down,
            code: code.into(),
            key: k.into(),
            shift,
            ctrl,
            alt,
        };
        let c = KeyEvent {
            ctrl: true,
            ..press(b'c', 0x06)
        };
        assert_eq!(
            keys(&mut input, chord(true, "KeyC", "c", false, true, false)),
            [c]
        );
        let at = KeyEvent {
            shift: false,
            ctrl: false,
            ..press(b'@', 0x14)
        };
        assert_eq!(
            keys(&mut input, chord(true, "KeyQ", "@", false, true, true)),
            [at]
        );
        let bang = KeyEvent {
            shift: true,
            ..press(b'!', 0x1e)
        };
        assert_eq!(
            keys(&mut input, chord(true, "Digit1", "!", true, false, false)),
            [bang]
        );
    }

    /// Losing the focus releases every key held, modifiers included, each with the
    /// code of its press; then a key-up from the browser for one of them reports
    /// nothing more.
    #[test]
    fn a_lost_focus_releases_every_key() {
        let mut input = Input::new();
        keys(&mut input, key(true, "AltLeft", "Alt"));
        keys(&mut input, key(true, "KeyQ", "q"));
        keys(&mut input, key(true, "ControlRight", "Control"));
        assert_eq!(
            keys(&mut input, PageInput::Release),
            [
                release(0, usage::LEFT_ALT),
                release(b'q', 0x14),
                release(0, usage::RIGHT_CTRL)
            ]
        );
        assert_eq!(keys(&mut input, key(false, "KeyQ", "q")), []);
        assert_eq!(keys(&mut input, PageInput::Release), []);
    }

    /// Print Screen comes as its key-up alone on Windows; the Korean Lang1 and Lang2
    /// keys are make-only. Each is a press and a release at once.
    #[test]
    fn taps() {
        let mut input = Input::new();
        let tap = |u| vec![press(0, u), release(0, u)];
        assert_eq!(
            keys(&mut input, key(false, "PrintScreen", "PrintScreen")),
            tap(usage::PRINT_SCREEN)
        );
        assert_eq!(
            keys(&mut input, key(true, "Lang1", "HangulMode")),
            tap(usage::LANG1)
        );
        assert_eq!(keys(&mut input, key(true, "Lang1", "HangulMode")), []);
        assert_eq!(keys(&mut input, key(false, "Lang1", "HangulMode")), []);
        assert_eq!(
            keys(&mut input, key(false, "Lang2", "HanjaMode")),
            tap(usage::LANG2)
        );
        // On Linux Print Screen has its key-down too: a key like the others.
        assert_eq!(
            keys(&mut input, key(true, "PrintScreen", "PrintScreen")),
            [press(0, usage::PRINT_SCREEN)]
        );
        assert_eq!(
            keys(&mut input, key(false, "PrintScreen", "PrintScreen")),
            [release(0, usage::PRINT_SCREEN)]
        );
    }

    fn pointers(input: &mut Input, page: PageInput) -> Vec<PointerChange> {
        input
            .messages(7, page, 800, 600)
            .into_iter()
            .map(|m| match m {
                Message::PointerInput { vm: 7, pointer } => pointer,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn at(x: u32, y: u32, buttons: u8) -> PointerChange {
        PointerChange {
            motion: PointerMotion::To { x, y },
            buttons,
            wheel: 0,
        }
    }

    fn page_at(x: i64, y: i64, buttons: u8) -> PageInput {
        PageInput::Pointer {
            at: Some((x, y)),
            buttons,
        }
    }

    /// Moves and buttons in screen pixels, clamped to the screen; a button change with
    /// no move; a lost focus releases the buttons.
    #[test]
    fn the_pointer() {
        let mut input = Input::new();
        assert_eq!(pointers(&mut input, page_at(10, 20, 0)), [at(10, 20, 0)]);
        assert_eq!(
            pointers(&mut input, page_at(10, 20, POINTER_LEFT)),
            [at(10, 20, POINTER_LEFT)]
        );
        // A drag that left the canvas, the fourth and fifth buttons (back, forward)
        // left out: the protocol has three.
        assert_eq!(
            pointers(&mut input, page_at(-5, 900, 0x19)),
            [at(0, 599, POINTER_LEFT)]
        );
        assert_eq!(
            pointers(&mut input, page_at(5000, -1, 0x19)),
            [at(799, 0, POINTER_LEFT)]
        );
        // No move and the same buttons: nothing to say.
        assert_eq!(
            pointers(&mut input, PageInput::Pointer { at: None, buttons: 1 }),
            []
        );
        let right = PointerChange {
            motion: PointerMotion::None,
            buttons: POINTER_LEFT | POINTER_RIGHT,
            wheel: 0,
        };
        assert_eq!(
            pointers(&mut input, PageInput::Pointer { at: None, buttons: 3 }),
            [right]
        );
        let none = PointerChange {
            motion: PointerMotion::None,
            buttons: 0,
            wheel: 0,
        };
        assert_eq!(
            input.messages(7, PageInput::Release, 800, 600),
            [Message::PointerInput { vm: 7, pointer: none }]
        );
        assert_eq!(pointers(&mut input, PageInput::Release), []);
    }

    /// The wheel as noVNC turns it into notches: 50 pixels make one, a line counts
    /// 19, an event gives at most one, a turn the other way starts again.
    #[test]
    fn the_wheel() {
        let mut input = Input::new();
        let wheel = |input: &mut Input, dy: f64, mode: u8| {
            pointers(input, PageInput::Wheel { dy, mode })
                .iter()
                .map(|p| {
                    assert_eq!(p.motion, PointerMotion::None);
                    p.wheel
                })
                .collect::<Vec<i8>>()
        };
        // Chrome on Windows: 100 pixels a notch, towards the person.
        assert_eq!(wheel(&mut input, 100.0, 0), [-1]);
        assert_eq!(wheel(&mut input, -100.0, 0), [1]);
        // Firefox: 3 lines a notch.
        assert_eq!(wheel(&mut input, -3.0, 1), [1]);
        assert_eq!(wheel(&mut input, -1.0, 2), []);
        // A touchpad's little steps add up.
        assert_eq!(wheel(&mut input, 20.0, 0), []);
        assert_eq!(wheel(&mut input, 20.0, 0), []);
        assert_eq!(wheel(&mut input, 20.0, 0), [-1]);
        // ... but not across a change of direction.
        assert_eq!(wheel(&mut input, 40.0, 0), []);
        assert_eq!(wheel(&mut input, -40.0, 0), []);
        assert_eq!(wheel(&mut input, -10.0, 0), [1]);
        // The buttons held go with it.
        pointers(&mut input, page_at(1, 1, POINTER_RIGHT));
        let p = pointers(&mut input, PageInput::Wheel { dy: 60.0, mode: 0 });
        assert_eq!((p[0].buttons, p[0].wheel), (POINTER_RIGHT, -1));
    }

    /// A rectangle goes to the page with its place, its pixels in canvas order.
    #[test]
    fn frames_for_the_canvas() {
        // Two pixels: 0x00112233 and 0x00ffeedd, little-endian XRGB.
        let msg = frame_message(3, 4, 2, 1, &[0x33, 0x22, 0x11, 0x00, 0xdd, 0xee, 0xff, 0x00]);
        assert_eq!(msg[..2], [0x82, 24]);
        assert_eq!(msg[2..18], [3, 0, 0, 0, 4, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(msg[18..], [0x11, 0x22, 0x33, 0xff, 0xff, 0xee, 0xdd, 0xff]);
    }

    /// After a write that failed, which may have left part of a frame on the wire,
    /// the page counts as closed: neither a message nor the close frame is written
    /// after it, so the page never reads them as the rest of the torn frame.
    #[test]
    fn nothing_follows_a_failed_write() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let _peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        // Every write fails from here, the same way on every system.
        stream.shutdown(Shutdown::Write).unwrap();
        let page = Page {
            stream: Mutex::new((stream, false)),
        };
        assert!(page.send(&frame_message(0, 0, 1, 1, &[0; 4])).is_err());
        assert!(crate::console::lock(&page.stream).1, "closed");
        assert!(page.text("after").is_ok(), "dropped, as after a close frame");
        assert!(!page.close(Some(CLOSE_NORMAL), "bye"), "no close frame either");
    }

    #[test]
    fn screens_pendragon_can_relay() {
        assert!(check_screen(640, 480, 0).is_ok());
        assert!(check_screen(1920, 1080, 0).is_ok());
        assert!(check_screen(640, 480, 1).is_err());
        assert!(check_screen(0, 480, 0).is_err());
        assert!(check_screen(4096, 4096, 0).is_err());
    }
}
