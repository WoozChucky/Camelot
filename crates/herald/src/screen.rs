//! Herald's screen: a VM's framebuffer shown in a view (a window), and the keys and
//! mouse of that view sent to the VM's keyboard and pointer devices (protocol doc,
//! section 6.4).
//!
//! [`ScreenSession`] is the streaming layer, and knows nothing of windows: it attaches
//! over an Avalon [`Connection`], rebuilds the VM's screen from the frames the server
//! sends ([`Screen::apply_frame`]), and runs one loop until the view is closed or the
//! VM ends. Everything it needs from the window is the [`View`] trait, so the tests
//! drive a session with a view that is a few vectors, on any platform. The Windows
//! view is `crate::window`, Grail's own framebuffer window.

use crate::{ended, out_of_step, End};
use avalon_proto::{
    kind_of, AttachKind, Connection, Error, Frame, KeyEvent, Message, MessageKind, PointerChange,
    PointerMotion, Screen, VmRef, VmState, VmSummary, PIXEL_FORMAT_XRGB8888, POINTER_BUTTONS,
};
use grail::pointer::Motion;
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Longest a session waits for the server before it looks at the view again: the
/// latency of a key or a click when the screen is not changing.
pub const POLL: Duration = Duration::from_millis(10);

/// Something that happened in the view, for the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A key, for the keyboard device.
    Key(KeyEvent),
    /// A change of the pointer, in screen pixels, for the pointer device.
    Pointer(PointerChange),
}

/// Where a [`ScreenSession`] shows the screen and gets its input from: a window.
pub trait View {
    /// Handles what happened in the view since the last call. Returns false once the
    /// person has closed it, which detaches.
    fn pump(&mut self) -> bool;

    /// The keys typed and the pointer changes since the last call, oldest first.
    fn take_input(&mut self) -> Vec<Input>;

    /// Whether the view must be drawn again although the screen did not change (it
    /// was uncovered, say).
    fn needs_redraw(&mut self) -> bool;

    /// Shows `screen`.
    fn draw(&mut self, screen: &Screen);

    /// The VM is now in `state`, for the view to show (in its title, say).
    fn vm_state(&mut self, _state: VmState) {}
}

/// A VM's screen attached over an Avalon connection.
///
/// [`ScreenSession::attach`] attaches and learns the VM's name and state;
/// [`ScreenSession::run`] streams until the session ends. Several viewers (in one
/// process or many) may attach to one VM: all of them see its screen, and input from
/// any of them reaches it.
pub struct ScreenSession {
    conn: Connection<TcpStream>,
    /// The VM, as the server described it right after the attach (its state is kept up
    /// to date while the session runs).
    pub vm: VmSummary,
    /// The VM's screen, as the frames received so far draw it.
    pub screen: Screen,
    /// Frames received so far, the full one after the attach included.
    pub frames: u64,
    /// How long to wait for the server to confirm a detach (default
    /// [`crate::DETACH_TIMEOUT`]).
    pub detach_timeout: Duration,
    /// Set once a detach is under way.
    detaching: bool,
    /// The buttons of the last pointer change sent, for [`input_messages`].
    held: Option<u8>,
    /// The screen changed since it was last drawn.
    dirty: bool,
}

impl ScreenSession {
    /// Attaches to `vm`'s screen on `conn` (a connection after its handshake), then asks
    /// for the VM's name and state. Fails with the server's error if there is no such
    /// VM, and with [`Error::Invalid`] for a screen this version cannot show (a format
    /// other than XRGB8888, or an empty screen).
    pub fn attach(mut conn: Connection<TcpStream>, vm: &VmRef) -> Result<ScreenSession, Error> {
        let (id, width, height) = match conn.request(&Message::ScreenAttach { vm: vm.clone() })? {
            Message::ScreenAttached {
                vm,
                width,
                height,
                format,
            } => {
                if format != PIXEL_FORMAT_XRGB8888 || Screen::byte_len(width, height).is_none() {
                    return Err(Error::Invalid(format!(
                        "a {width}x{height} screen in pixel format {format}, which Herald cannot show"
                    )));
                }
                (vm, width, height)
            }
            other => return Err(Error::Unexpected(other.type_code())),
        };
        // The full frame that follows the attach is queued by `request`; `run` takes it.
        let vm = match conn.request(&Message::GetVmInfo { vm: VmRef::Id(id) })? {
            Message::VmInfo(details) => details.vm,
            other => return Err(Error::Unexpected(other.type_code())),
        };
        Ok(ScreenSession {
            conn,
            vm,
            screen: Screen::new(width, height),
            frames: 0,
            detach_timeout: crate::DETACH_TIMEOUT,
            detaching: false,
            held: None,
            dirty: true,
        })
    }

    /// Shows the screen in `view` and streams: frames from the server into the view,
    /// the view's keys and pointer changes to the VM. Returns when the view is closed
    /// (which detaches: [`End::Detached`]), or when the VM stops, exits or is destroyed;
    /// a VM that had already stopped or exited ends the session right after its last
    /// frame is shown. `self.screen` keeps the last frame, for the caller to go on
    /// showing.
    ///
    /// Losing the connection, or a frame that does not fit the screen, is an error.
    /// Events this session does not need (other VMs', console output, ones that fail
    /// to decode) are passed over, as the protocol says a client should.
    pub fn run(&mut self, view: &mut dyn View) -> Result<End, Error> {
        view.vm_state(self.vm.state);
        // Events read while attaching come first: the full frame, and maybe more.
        while self.conn.queued_events() > 0 {
            if let Ok(event) = self.conn.recv() {
                if let Some(end) = self.on_server(event.type_code(), Ok(event), view)? {
                    view.draw(&self.screen);
                    return Ok(end);
                }
            }
        }
        view.draw(&self.screen);
        self.dirty = false;
        if ended(self.vm.state) {
            return Ok(End::VmEnded(self.vm.state));
        }

        let (tx, events) = mpsc::channel();
        let mut reader = self.conn.try_clone()?;
        reader.get_ref().set_read_timeout(None)?;
        let reading = std::thread::spawn(move || loop {
            let frame = reader.recv_frame();
            let fatal = matches!(&frame, Err(e) if e.is_fatal());
            if tx.send(frame).is_err() || fatal {
                break;
            }
        });
        let result = self.stream(view, events);
        // Unblocks the reading thread (and tells the server we are gone).
        let _ = self.conn.get_ref().shutdown(Shutdown::Both);
        let _ = reading.join();
        result
    }

    /// The session loop. The view is pumped at least every [`POLL`], and drawn once
    /// after each batch of frames rather than once per frame.
    fn stream(&mut self, view: &mut dyn View, events: Receiver<Result<Frame, Error>>) -> Result<End, Error> {
        let mut deadline: Option<Instant> = None;
        loop {
            if deadline.is_none() {
                if view.pump() {
                    for msg in input_messages(self.vm.id, &mut self.held, view.take_input()) {
                        self.conn.send(&msg)?;
                    }
                } else {
                    // Closed: detach, and end once the server says so.
                    self.conn.send(&Message::ScreenDetach { vm: self.vm.id })?;
                    self.detaching = true;
                    deadline = Some(Instant::now() + self.detach_timeout);
                }
            }
            let wait = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => POLL,
            };
            let first = match events.recv_timeout(wait) {
                Ok(frame) => Some(frame),
                // No answer in time: the detach is done as far as we are concerned,
                // closing the connection ends the attachment anyway.
                Err(RecvTimeoutError::Timeout) if deadline.is_some() => return Ok(End::Detached),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) if self.detaching => return Ok(End::Detached),
                Err(RecvTimeoutError::Disconnected) => return Err(Error::Closed),
            };
            // Everything that has arrived, then one drawing.
            for frame in first
                .into_iter()
                .chain(std::iter::from_fn(|| events.try_recv().ok()))
            {
                if let Some(end) = self.on_frame(frame, view)? {
                    if self.dirty {
                        view.draw(&self.screen);
                    }
                    return Ok(end);
                }
            }
            if std::mem::take(&mut self.dirty) | view.needs_redraw() {
                view.draw(&self.screen);
            }
        }
    }

    /// Handles a frame the reading thread got, or the error that ended its reading.
    fn on_frame(&mut self, frame: Result<Frame, Error>, view: &mut dyn View) -> Result<Option<End>, Error> {
        match frame {
            Ok(frame) => self.on_server(frame.ty, frame.message(), view),
            // The server hanging up on a detach: the detach is done all the same.
            Err(Error::Closed) if self.detaching => Ok(Some(End::Detached)),
            Err(e) if e.is_fatal() => Err(e),
            // A frame too short to hold a type: it cannot be placed while an answer is
            // due; otherwise it is set aside.
            Err(e) if self.detaching => Err(out_of_step(e)),
            Err(_) => Ok(None),
        }
    }

    /// Handles a frame of type `ty` from the server, decoded as `msg`. Returns the end
    /// of the session, if it is one. Frames are placed as in the console session
    /// (`Session::on_server`): the only request a running session sends is
    /// `ScreenDetach`, so a reply is its answer.
    fn on_server(
        &mut self,
        ty: u16,
        msg: Result<Message, Error>,
        view: &mut dyn View,
    ) -> Result<Option<End>, Error> {
        let id = self.vm.id;
        match (kind_of(ty), msg) {
            (Some(MessageKind::Event), Ok(frame @ Message::ScreenFrame { vm, .. })) if vm == id => {
                self.screen
                    .apply_frame(&frame)
                    .map_err(|e| Error::Invalid(format!("screen frame: {e}")))?;
                self.frames += 1;
                self.dirty = true;
            }
            (Some(MessageKind::Event), Ok(Message::VmStateChanged { vm, state })) if vm == id => {
                self.vm.state = state;
                view.vm_state(state);
                if ended(state) {
                    return Ok(Some(End::VmEnded(state)));
                }
            }
            (
                Some(MessageKind::Event),
                Ok(Message::Detached {
                    vm,
                    kind: AttachKind::Screen,
                }),
            ) if vm == id => return Ok(Some(End::Destroyed)),
            // Other events, and events that do not decode: set aside, never answered.
            (Some(MessageKind::Event), _) => {}
            (Some(MessageKind::Reply), _) if self.detaching => return Ok(Some(End::Detached)),
            (Some(MessageKind::Reply), _) => return Err(Error::Unexpected(ty)),
            // A request, or a type this client does not know.
            (Some(MessageKind::Request) | None, msg) => {
                if self.detaching {
                    return Err(out_of_step(msg.err().unwrap_or(Error::Unexpected(ty))));
                }
            }
        }
        Ok(None)
    }
}

/// The messages that carry `input` to VM `vm`: one `KeyInput` per key, one
/// `PointerInput` per pointer change, except that a run of moves with the same
/// buttons held and no wheel goes as its last move (a mouse reports far more moves
/// than a guest's pointer FIFO holds; only where it ends up matters). Keys with code
/// 0 and usage 0, which no key has, are dropped.
///
/// A button change is not part of the run around it, though Grail's window reports a
/// press or a release as a move to where it happened, with the new buttons: the moves
/// after it start a run of their own, so the guest sees the press (or release) where
/// it was, and a drag starts and ends where it did. `held` is the buttons in effect
/// before `input` (the last pointer change sent), `None` while unknown; it is updated,
/// for the next batch.
pub fn input_messages(vm: u32, held: &mut Option<u8>, input: Vec<Input>) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    // Whether the last message in `out` is a move that changed no button, so a later
    // move with the same buttons may take its place.
    let mut last_is_run = false;
    for item in input {
        match item {
            Input::Key(key) if key.code != 0 || key.usage != 0 => {
                out.push(Message::KeyInput { vm, key });
                last_is_run = false;
            }
            Input::Key(_) => {}
            Input::Pointer(mut pointer) => {
                pointer.buttons &= POINTER_BUTTONS;
                let is_move = matches!(pointer.motion, PointerMotion::To { .. }) && pointer.wheel == 0;
                let same_buttons = *held == Some(pointer.buttons);
                if let Some(Message::PointerInput { pointer: last, .. }) = out.last_mut() {
                    if last_is_run && is_move && same_buttons {
                        *last = pointer;
                        continue;
                    }
                }
                out.push(Message::PointerInput { vm, pointer });
                last_is_run = is_move && same_buttons;
                *held = Some(pointer.buttons);
            }
        }
    }
    out
}

/// A key from Grail's window, for the wire: the character typed and the physical key
/// (its HID usage), so the guest's keyboard mode picks the one it sees.
pub fn key_from_grail(key: grail::keyboard::KeyEvent) -> KeyEvent {
    KeyEvent {
        code: key.code,
        release: key.release,
        shift: key.shift,
        ctrl: key.ctrl,
        repeat: key.repeat,
        usage: key.usage,
    }
}

/// A pointer change from Grail's window, for the wire, on a screen `width` × `height`
/// pixels. The window reports positions past the screen's edges while a drag has left
/// it; they go out clamped to the edges, as the guest's device would clamp them. The
/// wheel is clamped to what one change carries.
pub fn pointer_from_grail(input: grail::pointer::PointerInput, width: u32, height: u32) -> PointerChange {
    let clamp = |v: i32, size: u32| v.clamp(0, size.saturating_sub(1) as i32) as u32;
    PointerChange {
        motion: match input.motion {
            Motion::None => PointerMotion::None,
            Motion::To { x, y } => PointerMotion::To {
                x: clamp(x, width),
                y: clamp(y, height),
            },
            Motion::By { dx, dy } => PointerMotion::By { dx, dy },
        },
        buttons: input.buttons & POINTER_BUTTONS,
        wheel: input.wheel.clamp(i8::MIN as i32, i8::MAX as i32) as i8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avalon_proto::{POINTER_LEFT, POINTER_RIGHT};

    fn to(x: u32, y: u32, buttons: u8) -> Input {
        Input::Pointer(PointerChange {
            motion: PointerMotion::To { x, y },
            buttons,
            wheel: 0,
        })
    }

    fn key(code: u8) -> Input {
        Input::Key(KeyEvent {
            code,
            release: false,
            shift: false,
            ctrl: false,
            repeat: false,
            usage: 0,
        })
    }

    #[test]
    fn keys_that_type_nothing_go_by_their_usage() {
        // Shift alone has no code but a usage: the guest sees it in raw mode.
        let shift = KeyEvent {
            usage: 0xe1,
            ..match key(0) {
                Input::Key(k) => k,
                _ => unreachable!(),
            }
        };
        let msgs = input_messages(7, &mut None, vec![Input::Key(shift), key(0)]);
        assert_eq!(msgs, [Message::KeyInput { vm: 7, key: shift }]);
        msgs[0].check().unwrap();
    }

    fn pointer_of(msg: &Message) -> PointerChange {
        match msg {
            Message::PointerInput { vm: 7, pointer } => *pointer,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn runs_of_moves_go_as_their_last() {
        let wheel = Input::Pointer(PointerChange {
            motion: PointerMotion::None,
            buttons: POINTER_LEFT,
            wheel: -1,
        });
        let mut held = Some(0);
        let msgs = input_messages(
            7,
            &mut held,
            vec![
                to(1, 1, 0),
                to(2, 2, 0),
                to(3, 3, 0),
                to(3, 3, POINTER_LEFT),
                to(4, 4, POINTER_LEFT),
                to(5, 5, POINTER_LEFT),
                wheel,
                to(6, 6, POINTER_LEFT),
                key(b'a'),
                to(7, 7, POINTER_LEFT),
                key(0),
                to(8, 8, POINTER_LEFT | 0xf8),
            ],
        );
        let pointer = |i: usize| pointer_of(&msgs[i]);
        assert_eq!(msgs.len(), 7, "{msgs:#?}");
        assert_eq!(
            pointer(0),
            PointerChange {
                motion: PointerMotion::To { x: 3, y: 3 },
                ..Default::default()
            }
        );
        // A press is kept where it happened: the moves after it are a run of their
        // own, not merged into it; nor is a wheel turn.
        assert_eq!(pointer(1).motion, PointerMotion::To { x: 3, y: 3 });
        assert_eq!(pointer(1).buttons, POINTER_LEFT);
        assert_eq!(pointer(2).motion, PointerMotion::To { x: 5, y: 5 });
        assert_eq!(pointer(2).buttons, POINTER_LEFT);
        assert_eq!(pointer(3).wheel, -1);
        assert_eq!(pointer(4).motion, PointerMotion::To { x: 6, y: 6 });
        // Keys keep their place among the moves; a key with code 0 is dropped.
        assert!(matches!(msgs[5], Message::KeyInput { vm: 7, key } if key.code == b'a'));
        // Reserved button bits never reach the wire.
        assert_eq!(pointer(6).motion, PointerMotion::To { x: 8, y: 8 });
        assert_eq!(pointer(6).buttons, POINTER_LEFT);
        assert_eq!(held, Some(POINTER_LEFT));
        for msg in &msgs {
            msg.check().unwrap();
        }
    }

    /// A press and a release, each followed by moves in the same batch, reach the guest
    /// where they happened, also when the batch before is not known.
    #[test]
    fn a_drag_starts_and_ends_where_it_did() {
        let mut held = None;
        let msgs = input_messages(
            7,
            &mut held,
            vec![
                to(10, 10, 0),
                to(10, 10, POINTER_LEFT),
                to(100, 80, POINTER_LEFT),
                to(200, 150, POINTER_LEFT),
                to(200, 150, 0),
                to(250, 160, 0),
                to(300, 300, 0),
            ],
        );
        let got: Vec<(PointerMotion, u8)> = msgs
            .iter()
            .map(|m| (pointer_of(m).motion, pointer_of(m).buttons))
            .collect();
        let at = |x, y, b| (PointerMotion::To { x, y }, b);
        assert_eq!(
            got,
            [
                at(10, 10, 0),
                at(10, 10, POINTER_LEFT),
                at(200, 150, POINTER_LEFT),
                at(200, 150, 0),
                at(300, 300, 0)
            ]
        );

        // `held` carries over: a press that opens the next batch is kept too.
        let msgs = input_messages(7, &mut held, vec![to(5, 5, POINTER_LEFT), to(6, 6, POINTER_LEFT)]);
        let got: Vec<(PointerMotion, u8)> = msgs
            .iter()
            .map(|m| (pointer_of(m).motion, pointer_of(m).buttons))
            .collect();
        assert_eq!(got, [at(5, 5, POINTER_LEFT), at(6, 6, POINTER_LEFT)]);
        // Moves with the button held, from the start of a batch, are one run.
        let msgs = input_messages(7, &mut held, vec![to(7, 7, POINTER_LEFT), to(8, 8, POINTER_LEFT)]);
        let got: Vec<(PointerMotion, u8)> = msgs
            .iter()
            .map(|m| (pointer_of(m).motion, pointer_of(m).buttons))
            .collect();
        assert_eq!(got, [at(8, 8, POINTER_LEFT)]);
    }

    #[test]
    fn grail_input_for_the_wire() {
        use grail::pointer::PointerInput;
        assert_eq!(
            pointer_from_grail(PointerInput::move_to(-5, 900).with_buttons(0xff), 640, 480),
            PointerChange {
                motion: PointerMotion::To { x: 0, y: 479 },
                buttons: POINTER_BUTTONS,
                wheel: 0,
            }
        );
        assert_eq!(
            pointer_from_grail(PointerInput::move_to(639, 0), 640, 480).motion,
            PointerMotion::To { x: 639, y: 0 }
        );
        assert_eq!(
            pointer_from_grail(PointerInput::buttons(POINTER_RIGHT).with_wheel(1000), 640, 480),
            PointerChange {
                motion: PointerMotion::None,
                buttons: POINTER_RIGHT,
                wheel: 127,
            }
        );
        assert_eq!(
            pointer_from_grail(PointerInput::move_by(-3, 4).with_wheel(-1000), 640, 480),
            PointerChange {
                motion: PointerMotion::By { dx: -3, dy: 4 },
                buttons: 0,
                wheel: -128,
            }
        );
        // Both forms of the key go out: the code and the physical key.
        let k = grail::keyboard::KeyEvent::release(b'q')
            .with_usage(0x14)
            .with_ctrl(true);
        assert_eq!(
            key_from_grail(k),
            KeyEvent {
                code: b'q',
                release: true,
                shift: false,
                ctrl: true,
                repeat: false,
                usage: 0x14,
            }
        );
        let k = grail::keyboard::KeyEvent::key_press(0xe3).with_repeat(true);
        assert_eq!(
            key_from_grail(k),
            KeyEvent {
                code: 0,
                release: false,
                shift: false,
                ctrl: false,
                repeat: true,
                usage: 0xe3,
            }
        );
    }
}
