//! The screen session against an in-process Avalon (and a scripted fake server), with
//! no window: the view is a few vectors behind a lock, so the test can type and move
//! the mouse into it while the session runs, and look at what it was asked to draw.
//! The guests are the repository's examples: `colorbars` (the framebuffer example),
//! `pointer` (a cursor that follows the mouse; Esc ends it), `cursor` (the cursor plane
//! moved over a still frame).

mod common;

use avalon_proto::{
    Connection, Cursor, KeyEvent, Message, PointerChange, PointerMotion, Screen, VmRef, VmState, POINTER_LEFT,
};
use common::*;
use grail::framebuffer::Composite;
use herald::screen::{Input, ScreenSession, View};
use herald::End;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the fake view holds.
#[derive(Default)]
struct ViewState {
    /// False once the "window" is closed.
    closed: bool,
    /// Input waiting for the session to take it.
    input: Vec<Input>,
    /// The screen and the cursor as last drawn, and how many times they were drawn.
    drawn: Option<Screen>,
    cursor: Cursor,
    draws: usize,
    /// How many of those were for a change of the cursor alone (`View::draw_cursor`).
    cursor_draws: usize,
    /// What the last drawing showed: the screen with the cursor blended over it, as the
    /// window shows it (`herald::screen::frame`, kept from one drawing to the next as
    /// the window keeps it).
    shown: Vec<u8>,
    composite: Composite,
    /// Ask for a redraw, as an uncovered window does.
    redraw: bool,
    /// Every state the session reported.
    states: Vec<VmState>,
    pumps: usize,
}

#[derive(Clone, Default)]
struct FakeView(Arc<Mutex<ViewState>>);

impl View for FakeView {
    fn pump(&mut self) -> bool {
        let mut st = self.0.lock().unwrap();
        st.pumps += 1;
        !st.closed
    }

    fn take_input(&mut self) -> Vec<Input> {
        std::mem::take(&mut self.0.lock().unwrap().input)
    }

    fn needs_redraw(&mut self) -> bool {
        std::mem::take(&mut self.0.lock().unwrap().redraw)
    }

    fn draw(&mut self, screen: &Screen, cursor: &Cursor) {
        self.show(screen, cursor, false);
    }

    fn draw_cursor(&mut self, screen: &Screen, cursor: &Cursor) {
        self.show(screen, cursor, true);
    }

    fn vm_state(&mut self, state: VmState) {
        self.0.lock().unwrap().states.push(state);
    }
}

impl FakeView {
    /// Draws as the window does: the whole picture, or only the cursor's part of it.
    fn show(&self, screen: &Screen, cursor: &Cursor, cursor_only: bool) {
        let mut st = self.0.lock().unwrap();
        let st = &mut *st;
        st.drawn = Some(screen.clone());
        st.cursor = cursor.clone();
        let frame = herald::screen::frame(screen, cursor);
        let shown = if cursor_only {
            st.composite.draw_cursor(&frame)
        } else {
            st.composite.draw(&frame)
        };
        st.shown = shown.unwrap().to_vec();
        st.draws += 1;
        st.cursor_draws += usize::from(cursor_only);
    }

    fn send(&self, input: Input) {
        self.0.lock().unwrap().input.push(input);
    }

    fn close(&self) {
        self.0.lock().unwrap().closed = true;
    }

    fn draws(&self) -> usize {
        self.0.lock().unwrap().draws
    }

    /// The pixel at (`x`, `y`) of the screen as last drawn.
    fn pixel(&self, x: u32, y: u32) -> Option<u32> {
        self.0.lock().unwrap().drawn.as_ref().map(|s| s.pixel(x, y))
    }

    fn wait_for_pixel(&self, x: u32, y: u32, want: u32) {
        wait_until(&format!("pixel ({x}, {y}) to be {want:06x}"), || {
            self.pixel(x, y) == Some(want)
        });
    }
}

/// A screen session running on a thread of its own with a fake view.
struct Running {
    view: FakeView,
    thread: JoinHandle<(Result<End, avalon_proto::Error>, ScreenSession)>,
}

impl Running {
    fn start(mut session: ScreenSession) -> Running {
        let view = FakeView::default();
        let mut v = view.clone();
        let thread = std::thread::spawn(move || (session.run(&mut v), session));
        Running { view, thread }
    }

    fn end(self) -> (Result<End, avalon_proto::Error>, ScreenSession) {
        let start = std::time::Instant::now();
        while !self.thread.is_finished() {
            assert!(start.elapsed() < SLOW, "the session is still running");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.thread.join().unwrap()
    }
}

fn attach(server: &avalon::ServerThread, vm: VmRef) -> ScreenSession {
    let conn = Connection::connect(server.addr(), herald::AGENT).unwrap();
    ScreenSession::attach(conn, &vm).unwrap_or_else(|e| panic!("attach {vm}: {e}"))
}

/// What Grail's display shows after running `program` to its end: the frame with the
/// cursor plane blended over it, as Grail's window and dumps show it.
fn grail_picture(program: &[u8]) -> Vec<u8> {
    let mut m = grail::Machine::with_ram(16 * 1024 * 1024);
    m.load_elf(program).unwrap();
    assert_eq!(m.run(Some(100_000_000), |_| {}), grail::RunResult::Exited(0));
    m.cpu
        .bus
        .frame()
        .composited()
        .expect("the display is on")
        .into_owned()
}

/// What Grail's framebuffer holds after running `program` to its end.
fn grail_screen(program: &[u8]) -> Vec<u8> {
    let mut m = grail::Machine::with_ram(16 * 1024 * 1024);
    m.load_elf(program).unwrap();
    assert_eq!(m.run(Some(100_000_000), |_| {}), grail::RunResult::Exited(0));
    // What the display shows, from VRAM or RAM, at its mode's size.
    m.cpu.bus.frame().pixels.expect("the display is on").to_vec()
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

fn create(server: &avalon::ServerThread, name: &str, program: Vec<u8>) -> u32 {
    let mut rt = connect(server);
    match rt.request(&Message::CreateVm {
        name: name.into(),
        profile: avalon_proto::HardwareProfile::with_ram(16 * MIB),
        program,
    }) {
        Ok(Message::VmCreated { id }) => id,
        other => panic!("create {name}: {other:?}"),
    }
}

/// The framebuffer example's output reaches the view, pixel for pixel what Grail
/// draws, and the session ends with the program, the last frame drawn.
#[test]
fn the_framebuffer_example_reaches_the_view() {
    let server = server();
    let id = create(&server, "bars", example("colorbars"));
    let session = attach(&server, VmRef::Name("bars".into()));
    assert_eq!((session.vm.id, session.vm.state), (id, VmState::Created));
    assert_eq!((session.screen.width(), session.screen.height()), (640, 480));
    let running = Running::start(session);
    wait_until("the first drawing", || running.view.draws() > 0);
    assert_eq!(running.view.pixel(0, 0), Some(0), "black before the start");

    connect(&server)
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let view = running.view.clone();
    let (end, session) = running.end();
    assert_eq!(end.unwrap(), End::VmEnded(VmState::Exited(0)));
    let expected = grail_screen(&example("colorbars"));
    assert_eq!(session.screen.pixels(), expected);
    let st = view.0.lock().unwrap();
    assert_eq!(st.drawn.as_ref().unwrap().pixels(), expected, "drawn as received");
    assert_eq!(
        st.states,
        [VmState::Created, VmState::Running, VmState::Exited(0)]
    );
    drop(st);

    // Attached after the end: the last frame, then the session ends at once.
    let mut late = attach(&server, VmRef::Id(id));
    let mut view = FakeView::default();
    assert_eq!(late.run(&mut view).unwrap(), End::VmEnded(VmState::Exited(0)));
    assert_eq!(late.screen.pixels(), expected);
    assert_eq!(late.frames, 1);
    server.stop().unwrap();
}

/// A guest that changes modes (`examples/flip`, 800x600 page flipped from RAM): the
/// session's screen takes the new size, and the view is drawn at it, ending with the
/// last frame Grail shows.
#[test]
fn a_mode_change_resizes_the_view() {
    let server = server();
    let id = create(&server, "flip", example("flip"));
    let session = attach(&server, VmRef::Id(id));
    assert_eq!((session.screen.width(), session.screen.height()), (640, 480));
    let running = Running::start(session);
    wait_until("the first drawing", || running.view.draws() > 0);
    connect(&server)
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let view = running.view.clone();
    let (end, session) = running.end();
    assert_eq!(end.unwrap(), End::VmEnded(VmState::Exited(0)));
    assert_eq!(session.resizes, 1);
    assert_eq!((session.screen.width(), session.screen.height()), (800, 600));
    let expected = grail_screen(&example("flip"));
    assert!(session.screen.pixels() == expected);
    let st = view.0.lock().unwrap();
    let drawn = st.drawn.as_ref().unwrap();
    assert_eq!((drawn.width(), drawn.height()), (800, 600));
    assert!(drawn.pixels() == expected, "drawn as received");
    drop(st);
    server.stop().unwrap();
}

/// Acceptance of #203, Herald shows the cursor: a guest that moves the framebuffer's
/// cursor over a frame it draws once (`examples/cursor`) ends with the view showing
/// pixel for pixel what Grail's display does, its frame with the cursor blended over it
/// where the guest left it, though no frame carried the cursor: the session's screen is
/// Grail's frame without it, and its cursor the guest's.
#[test]
fn the_view_shows_the_cursor() {
    let server = server();
    let id = create(&server, "cursor", example("cursor"));
    let running = Running::start(attach(&server, VmRef::Id(id)));
    wait_until("the first drawing", || running.view.draws() > 0);
    connect(&server)
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let view = running.view.clone();
    let (end, session) = running.end();
    assert_eq!(end.unwrap(), End::VmEnded(VmState::Exited(0)));
    assert!(
        session.screen.pixels() == grail_screen(&example("cursor")),
        "no cursor in the frames"
    );
    let (image, x, y) = session.cursor.shown().expect("the cursor shows");
    assert_eq!((image.width, image.height, x, y), (64, 64, 0, 0));
    let picture = grail_picture(&example("cursor"));
    assert!(
        picture != grail_screen(&example("cursor")),
        "the cursor changes the picture"
    );
    let st = view.0.lock().unwrap();
    assert!(st.shown == picture, "the view shows Grail's picture");
    assert_eq!(st.cursor, session.cursor);
    drop(st);
    server.stop().unwrap();
}

/// The mouse moved and clicked over the view reaches the guest's pointer device (its
/// cursor moves there and turns red), and a key its keyboard (Esc ends it). While
/// nothing changes, nothing more is drawn.
#[test]
fn typing_and_the_mouse_reach_the_guest() {
    let server = server();
    let id = create(&server, "ptr", example("pointer"));
    connect(&server)
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let running = Running::start(attach(&server, VmRef::Id(id)));
    let view = running.view.clone();
    view.wait_for_pixel(320, 240, 0x203048);

    // An unchanged screen: no frames, so nothing new to draw.
    std::thread::sleep(Duration::from_millis(200));
    let draws = view.draws();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(view.draws(), draws, "nothing changed, nothing drawn");
    // Unless the window asks.
    view.0.lock().unwrap().redraw = true;
    wait_until("the redraw", || view.draws() == draws + 1);

    // Many moves, as a mouse sends them, then a click where they end.
    for x in 0..=100 {
        view.send(Input::Pointer(PointerChange {
            motion: PointerMotion::To { x, y: x / 2 },
            buttons: 0,
            wheel: 0,
        }));
    }
    view.send(Input::Pointer(PointerChange {
        motion: PointerMotion::To { x: 100, y: 50 },
        buttons: POINTER_LEFT,
        wheel: 0,
    }));
    view.wait_for_pixel(100, 50, 0xff0000);
    assert_eq!(
        view.pixel(0, 0),
        Some(0x203048),
        "the cursor left its first place"
    );

    view.send(key(0x1b));
    let (end, _) = running.end();
    assert_eq!(end.unwrap(), End::VmEnded(VmState::Exited(0)));
    server.stop().unwrap();
}

/// Closing the view detaches; the VM carries on, and another viewer still works.
#[test]
fn closing_the_view_detaches() {
    let server = server();
    let id = create(&server, "ptr", example("pointer"));
    connect(&server)
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let running = Running::start(attach(&server, VmRef::Id(id)));
    running.view.wait_for_pixel(320, 240, 0x203048);
    assert_eq!(server.debug_vm(id).unwrap().viewers, 1);
    running.view.close();
    let (end, _) = running.end();
    assert_eq!(end.unwrap(), End::Detached);
    wait_until("the server to let go of the screen", || {
        server.debug_vm(id).unwrap().viewers == 0
    });
    assert_eq!(state(&server, id), VmState::Running);

    let again = Running::start(attach(&server, VmRef::Id(id)));
    again.view.wait_for_pixel(320, 240, 0x203048);
    again.view.send(key(0x1b));
    assert_eq!(again.end().0.unwrap(), End::VmEnded(VmState::Exited(0)));
    server.stop().unwrap();
}

/// The input of the pump that closes the view still goes out, before the detach: a
/// window closed with Alt+F4 sends the keys of that pump, and the releases of every key
/// held as it went. Here the last key is the Esc that ends the guest.
#[test]
fn input_taken_as_the_view_closes_goes_out() {
    let server = server();
    let id = create(&server, "ptr", example("pointer"));
    connect(&server)
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let running = Running::start(attach(&server, VmRef::Id(id)));
    running.view.wait_for_pixel(320, 240, 0x203048);
    {
        let mut st = running.view.0.lock().unwrap();
        st.input.push(key(0x1b));
        st.closed = true;
    }
    // The Esc goes before the detach, so the guest may end before the server answers
    // it: either way the session is over.
    let (end, _) = running.end();
    let end = end.unwrap();
    assert!(
        matches!(end, End::Detached | End::VmEnded(VmState::Exited(0))),
        "{end:?}"
    );
    wait_until("the guest to take the Esc", || {
        state(&server, id) == VmState::Exited(0)
    });
    server.stop().unwrap();
}

/// Pausing is reported to the view; destroying the VM ends the session.
#[test]
fn pause_and_destroy() {
    let server = server();
    let id = create(&server, "ptr", example("pointer"));
    let mut rt = connect(&server);
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let running = Running::start(attach(&server, VmRef::Id(id)));
    running.view.wait_for_pixel(320, 240, 0x203048);
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
    wait_until("the pause", || {
        running.view.0.lock().unwrap().states.contains(&VmState::Paused)
    });
    rt.request_ok(&Message::DestroyVm { vm: VmRef::Id(id) }).unwrap();
    let (end, _) = running.end();
    let end = end.unwrap();
    // Destroying a paused VM stops it first, which the session may hear about first.
    assert!(
        matches!(end, End::Destroyed | End::VmEnded(VmState::Stopped)),
        "{end:?}"
    );
    server.stop().unwrap();
}

/// Attaching to a VM that does not exist fails with the server's error.
#[test]
fn no_such_vm() {
    let server = server();
    let conn = Connection::connect(server.addr(), herald::AGENT).unwrap();
    match ScreenSession::attach(conn, &VmRef::Name("ghost".into())) {
        Err(avalon_proto::Error::Remote {
            code: avalon_proto::ErrorCode::NoSuchVm,
            ..
        }) => {}
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("attached"),
    }
    server.stop().unwrap();
}

/// The fake server's side of a screen attach: a `width` × `height` screen in
/// `format`, and (if the format is 0) a full frame of `colour`.
fn fake_screen_attach(conn: &mut Connection<std::net::TcpStream>, width: u32, height: u32, format: u32) {
    match conn.recv().unwrap() {
        Message::ScreenAttach { .. } => {}
        other => panic!("expected ScreenAttach, got {other:?}"),
    }
    conn.send(&Message::ScreenAttached {
        vm: 1,
        width,
        height,
        format,
    })
    .unwrap();
    if format != 0 {
        return;
    }
    conn.send(&Message::ScreenFrame {
        vm: 1,
        x: 0,
        y: 0,
        width,
        height,
        pixels: vec![0x11; (width * height * 4) as usize],
    })
    .unwrap();
    match conn.recv().unwrap() {
        Message::GetVmInfo { .. } => {}
        other => panic!("expected GetVmInfo, got {other:?}"),
    }
    conn.send(&Message::VmInfo(avalon_proto::VmDetails {
        vm: avalon_proto::VmSummary {
            id: 1,
            name: "fake".into(),
            state: VmState::Running,
            profile: Default::default(),
        },
        devices: Vec::new(),
        steps: 0,
    }))
    .unwrap();
}

/// A server whose frames do not fit the screen it announced is an error; frames for
/// other VMs and other events are passed over.
#[test]
fn a_frame_outside_the_screen_is_an_error() {
    let (addr, server) = fake_server(|mut conn| {
        fake_screen_attach(&mut conn, 4, 4, 0);
        for msg in [
            Message::ConsoleOutput {
                vm: 1,
                data: b"x".to_vec(),
            },
            Message::ScreenFrame {
                vm: 2,
                x: 100,
                y: 100,
                width: 1,
                height: 1,
                pixels: vec![0; 4],
            },
            Message::ScreenFrame {
                vm: 1,
                x: 1,
                y: 1,
                width: 1,
                height: 1,
                pixels: vec![0x22, 0, 0, 0],
            },
            Message::ScreenFrame {
                vm: 1,
                x: 3,
                y: 0,
                width: 2,
                height: 1,
                pixels: vec![0; 8],
            },
        ] {
            conn.send(&msg).unwrap();
        }
        // Hold the connection until the client goes.
        let _ = conn.recv();
    });
    let conn = Connection::connect(addr.as_str(), herald::AGENT).unwrap();
    let mut session = ScreenSession::attach(conn, &VmRef::Id(1)).unwrap();
    let mut view = FakeView::default();
    match session.run(&mut view) {
        Err(avalon_proto::Error::Invalid(why)) => assert!(why.contains("not inside"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(session.screen.pixel(1, 1), 0x22);
    assert_eq!(session.screen.pixel(0, 0), 0x111111);
    assert_eq!(session.frames, 2);
    server.join().unwrap();
}

/// `ScreenResized` starts the screen again, black, at the new size, which the frames
/// after it fill; a resize to a screen Herald cannot show is an error.
#[test]
fn a_resize_makes_a_new_screen() {
    let (addr, server) = fake_server(|mut conn| {
        fake_screen_attach(&mut conn, 4, 4, 0);
        let resized = |width, height, format| Message::ScreenResized {
            vm: 1,
            width,
            height,
            format,
        };
        for msg in [
            // Another VM's: passed over.
            Message::ScreenResized {
                vm: 2,
                width: 1,
                height: 1,
                format: 0,
            },
            resized(6, 2, 0),
            Message::ScreenFrame {
                vm: 1,
                x: 5,
                y: 1,
                width: 1,
                height: 1,
                pixels: vec![0x33, 0, 0, 0],
            },
            resized(6, 2, 7),
        ] {
            conn.send(&msg).unwrap();
        }
        let _ = conn.recv();
    });
    let conn = Connection::connect(addr.as_str(), herald::AGENT).unwrap();
    let mut session = ScreenSession::attach(conn, &VmRef::Id(1)).unwrap();
    let mut view = FakeView::default();
    match session.run(&mut view) {
        Err(avalon_proto::Error::Invalid(why)) => assert!(why.contains("pixel format 7"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!((session.screen.width(), session.screen.height()), (6, 2));
    assert_eq!(session.screen.pixel(5, 1), 0x33, "the frame after the resize");
    assert_eq!(session.screen.pixel(0, 0), 0, "black, not the old screen");
    assert_eq!((session.resizes, session.frames), (1, 2));
    server.join().unwrap();
}

/// The cursor from a scripted server: its image and place are drawn over the screen,
/// a move alone redraws it (no frame comes), a resize keeps it where it is, hiding it
/// shows the screen alone, and another VM's cursor is passed over.
#[test]
fn cursor_messages_draw_the_cursor() {
    // The test lets the server go on to its next batch once it has seen the last one.
    let (next, batches) = std::sync::mpsc::channel::<()>();
    let (addr, server) = fake_server(move |mut conn| {
        fake_screen_attach(&mut conn, 4, 4, 0);
        // A 2x1 image: an opaque red pixel, then a half-covered green one.
        let shape = Message::CursorShape {
            vm: 1,
            width: 2,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            pixels: vec![0, 0, 0xff, 0xff, 0, 0x80, 0, 0x80],
        };
        // VM 2's cursor, after VM 1's: an opaque blue pixel at (0, 0), which a session
        // that took it would show there instead.
        let other = Message::CursorShape {
            vm: 2,
            width: 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            pixels: vec![0xff, 0, 0, 0xff],
        };
        let at = |vm, visible, x, y| Message::CursorPosition { vm, visible, x, y };
        // Then a frame of VM 1's, a pixel at (3, 0), to know the session has taken them all.
        let mark = Message::ScreenFrame {
            vm: 1,
            x: 3,
            y: 0,
            width: 1,
            height: 1,
            pixels: vec![0x22, 0, 0, 0],
        };
        let script = [
            vec![shape, at(1, true, -1, 2), other, at(2, true, 0, 0), mark],
            vec![at(1, true, 3, 3)],
            vec![Message::ScreenResized {
                vm: 1,
                width: 6,
                height: 6,
                format: 0,
            }],
            vec![at(1, false, 0, 0)],
        ];
        for batch in script {
            for msg in batch {
                conn.send(&msg).unwrap();
            }
            batches.recv().unwrap();
        }
        let _ = conn.recv();
    });
    let conn = Connection::connect(addr.as_str(), herald::AGENT).unwrap();
    let session = ScreenSession::attach(conn, &VmRef::Id(1)).unwrap();
    let running = Running::start(session);
    let view = running.view.clone();
    // Pixel (x, y) of what the view shows, 0x00RRGGBB.
    let shown = |x: usize, y: usize, width: usize| {
        let st = view.0.lock().unwrap();
        let at = (y * width + x) * 4;
        st.shown
            .get(at..at + 4)
            .map(|p| u32::from_le_bytes(p.try_into().unwrap()) & 0xff_ffff)
    };
    // The screen is 0x11 in every byte; the green pixel of the image over it: blue and
    // red 0x11 * 0x7f / 255 = 8, green 0x80 + 8.
    wait_until("the frame after the cursors", || shown(3, 0, 4) == Some(0x22));
    assert_eq!(shown(0, 2, 4), Some(0x08_8808), "the cursor at (-1, 2)");
    assert_eq!(shown(1, 2, 4), Some(0x11_1111), "beside it");
    assert_eq!(shown(0, 0, 4), Some(0x11_1111), "VM 2's cursor passed over");
    let frames = running
        .view
        .0
        .lock()
        .unwrap()
        .drawn
        .as_ref()
        .map(|s| s.pixels().to_vec());
    next.send(()).unwrap();
    wait_until("the cursor moved to (3, 3)", || shown(3, 3, 4) == Some(0xff_0000));
    assert_eq!(shown(0, 2, 4), Some(0x11_1111), "gone from where it was");
    assert!(
        view.0.lock().unwrap().cursor_draws > 0,
        "only the cursor's part drawn again"
    );
    assert_eq!(
        running
            .view
            .0
            .lock()
            .unwrap()
            .drawn
            .as_ref()
            .map(|s| s.pixels().to_vec()),
        frames,
        "the screen itself unchanged"
    );
    next.send(()).unwrap();
    wait_until("the resize", || {
        view.0
            .lock()
            .unwrap()
            .drawn
            .as_ref()
            .is_some_and(|s| s.width() == 6)
    });
    wait_until("the cursor still at (3, 3)", || shown(3, 3, 6) == Some(0xff_0000));
    next.send(()).unwrap();
    wait_until("the cursor hidden", || shown(3, 3, 6) == Some(0));
    assert!(
        view.0.lock().unwrap().shown.iter().all(|&b| b == 0),
        "black, no cursor"
    );
    next.send(()).unwrap();
    running.view.close();
    let (_, session) = running.end();
    assert!(!session.cursor.visible);
    assert_eq!(
        session.cursor.image.as_ref().map(|i| i.width),
        Some(2),
        "the image kept"
    );
    server.join().unwrap();
}

/// A screen in a format Herald cannot show is refused at the attach.
#[test]
fn an_unknown_pixel_format_is_refused() {
    let (addr, server) = fake_server(|mut conn| {
        fake_screen_attach(&mut conn, 640, 480, 7);
        let _ = conn.recv();
    });
    let conn = Connection::connect(addr.as_str(), herald::AGENT).unwrap();
    match ScreenSession::attach(conn, &VmRef::Id(1)) {
        Err(avalon_proto::Error::Invalid(why)) => assert!(why.contains("pixel format 7"), "{why}"),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("attached"),
    }
    server.join().unwrap();
}
