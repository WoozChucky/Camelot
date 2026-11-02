//! Screens and input over the protocol (protocol doc, section 6.4), against an
//! in-process Avalon: what a viewer rebuilds from the frames is pixel for pixel what
//! Grail draws; an unchanged screen sends nothing; a busy one is rate limited, and a
//! viewer that stops reading is held back rather than cut off; keys and pointer
//! changes reach the guest's devices, from viewers only.

mod common;

use avalon_proto::{
    AttachKind, ErrorCode, KeyEvent, Message, PointerChange, PointerMotion, Screen, VmRef, VmState,
    POINTER_LEFT,
};
use common::*;
use std::time::{Duration, Instant};

const FULL: u64 = 640 * 480;

fn key(code: u8, release: bool) -> KeyEvent {
    KeyEvent {
        code,
        release,
        shift: false,
        ctrl: false,
    }
}

fn exited(vm: u32) -> Message {
    Message::VmStateChanged {
        vm,
        state: VmState::Exited(0),
    }
}

fn stopped(vm: u32) -> Message {
    Message::VmStateChanged {
        vm,
        state: VmState::Stopped,
    }
}

/// Waits until a request on the viewer's connection is answered: the server has
/// handled everything the viewer sent before it.
fn sync(viewer: &mut Viewer) {
    assert_eq!(
        viewer.request(&Message::Ping { token: 9 }),
        Message::Pong { token: 9 }
    );
}

/// Turns the display on, then counts into the first pixel forever: a screen that
/// changes all the time, a little.
fn pixel_counter() -> Vec<u8> {
    guest(
        "
    .globl _start
_start:
    li    t0, 0x30000000
    li    t1, 1
    sw    t1, 0x10(t0)
    li    t2, 0x31000000
1:  addi  t3, t3, 1
    sw    t3, 0(t2)
    j     1b
",
    )
}

/// Turns the display on, then fills the whole screen with a new colour, again and
/// again: a screen that changes all the time, all of it.
fn screen_filler() -> Vec<u8> {
    guest(
        "
    .globl _start
_start:
    li    t0, 0x30000000
    li    t1, 1
    sw    t1, 0x10(t0)
    li    t4, 0x31000000
    li    t5, 0x3112c000
2:  addi  t3, t3, 0x111
    mv    t2, t4
1:  sw    t3, 0(t2)
    addi  t2, t2, 4
    bltu  t2, t5, 1b
    j     2b
",
    )
}

/// The framebuffer example, streamed: a viewer attached before the start sees black
/// (the display is off), then exactly the image Grail draws, before it hears that the
/// program has ended. A viewer attaching afterwards gets that last frame in full.
#[test]
fn colorbars_reach_a_viewer_as_grail_draws_them() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "bars", profile(16, None), example("colorbars"));
    let mut viewer = Viewer::attach(&server, VmRef::Id(id));
    assert_eq!((viewer.screen.width(), viewer.screen.height()), (640, 480));
    assert!(viewer.screen.pixels().iter().all(|&b| b == 0), "black");
    assert_eq!(server.debug_vm(id).unwrap().viewers, 1);

    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    viewer.wait_for_event(exited(id));
    let expected = grail_screen(&example("colorbars"));
    assert_eq!(
        viewer.screen.pixels(),
        expected,
        "the frames rebuild Grail's screen"
    );
    assert_eq!(viewer.screen.pixel(0, 0), 0x808080, "the border");
    assert_eq!(viewer.screen.pixel(20, 240), 0xffffff, "the first bar");
    assert_eq!(viewer.screen.pixel(620, 240), 0x000000, "the last bar");

    let late = Viewer::attach(&server, VmRef::Name("bars".into()));
    assert_eq!(late.screen, viewer.screen);
    assert_eq!(late.rects.len(), 1);
    server.stop().unwrap();
}

/// A guest that waits for input with its screen drawn sends nothing more; a pointer
/// change moves its cursor, which sends only the tiles that changed; Esc ends it.
#[test]
fn an_unchanged_screen_sends_no_frames_and_input_reaches_the_guest() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "ptr", profile(16, None), example("pointer"));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let mut viewer = Viewer::attach(&server, VmRef::Id(id));
    viewer.wait_until(|v| v.screen.pixel(320, 240) == 0x203048, "the background");
    viewer.idle(Duration::from_millis(200));
    let frames = viewer.rects.len();
    viewer.idle(Duration::from_millis(600));
    assert_eq!(viewer.rects.len(), frames, "an unchanged screen sends no frames");

    viewer.pointer(PointerChange {
        motion: PointerMotion::To { x: 100, y: 50 },
        buttons: POINTER_LEFT,
        wheel: 0,
    });
    viewer.wait_until(|v| v.screen.pixel(100, 50) == 0xff0000, "the cursor's tip, red");
    viewer.idle(Duration::from_millis(100));
    assert_eq!(viewer.screen.pixel(0, 0), 0x203048, "erased where it was");
    let moved = &viewer.rects[frames..];
    assert!(!moved.is_empty());
    let area: u64 = moved.iter().map(|r| r.area()).sum();
    assert!(area <= FULL / 50, "only what changed: {moved:?}");

    viewer.key(key(0x1b, false));
    viewer.wait_for_event(exited(id));
    server.stop().unwrap();
}

/// A guest that changes its screen all the time gets at most about one update per
/// `SCREEN_INTERVAL` to its viewers, and none once it is paused.
#[test]
fn a_busy_screen_is_rate_limited() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "busy", profile(1, None), pixel_counter());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let mut viewer = Viewer::attach(&server, VmRef::Id(id));
    let start = Instant::now();
    viewer.idle(Duration::from_secs(1));
    let elapsed = start.elapsed();
    let updates = viewer.rects.len() - 1;
    let most = (elapsed.as_millis() / avalon::SCREEN_INTERVAL.as_millis()) as usize + 2;
    assert!((5..=most).contains(&updates), "{updates} updates in {elapsed:?}");
    // Only the first pixel's tile changes.
    for r in &viewer.rects[1..] {
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 32, 32));
    }

    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
    viewer.wait_for_event(Message::VmStateChanged {
        vm: id,
        state: VmState::Paused,
    });
    // What the last slice drew may still come, once.
    viewer.idle(Duration::from_millis(200));
    let paused = viewer.rects.len();
    viewer.idle(Duration::from_millis(300));
    assert_eq!(viewer.rects.len(), paused, "a paused VM's screen does not change");

    // And it did come: stopping the VM, which captures its screen one last time,
    // finds nothing the viewer has not been sent already.
    rt.request_ok(&Message::StopVm { vm: VmRef::Id(id) }).unwrap();
    viewer.wait_for_event(stopped(id));
    viewer.idle(Duration::from_millis(100));
    assert_eq!(
        viewer.rects.len(),
        paused,
        "the paused VM's last pixels had reached the viewer"
    );
    server.stop().unwrap();
}

/// A viewer that stops reading while the whole screen changes all the time (~36 MB/s
/// at the full rate) is sent nothing more until it reads again, so it is not cut off
/// for falling 8 MiB behind; when it reads again it gets whole frames that apply.
#[test]
fn a_viewer_that_stops_reading_is_held_back_not_cut_off() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "fill", profile(1, None), screen_filler());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let mut conn = connect(&server);
    let Ok(Message::ScreenAttached { width, height, .. }) =
        conn.request(&Message::ScreenAttach { vm: VmRef::Id(id) })
    else {
        panic!("attach")
    };
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(server.debug_vm(id).unwrap().viewers, 1, "still attached");

    let mut screen = Screen::new(width, height);
    let mut frames = 0;
    let start = Instant::now();
    while frames < 60 {
        assert!(start.elapsed() < SLOW, "{frames} frames");
        let msg = conn.recv().unwrap();
        if let Message::ScreenFrame { .. } = msg {
            screen.apply_frame(&msg).unwrap();
            frames += 1;
        }
    }
    assert_eq!(
        conn.request(&Message::Ping { token: 5 }).unwrap(),
        Message::Pong { token: 5 },
        "the connection still works"
    );
    let colour = screen.pixel(0, 0);
    assert_ne!(colour, 0);
    server.stop().unwrap();
}

/// A viewer that fell behind is sent one full frame once it has caught up, which
/// gives it the screen the other viewers have; here it catches up while the VM is
/// paused, so after the pause nothing else could repaint what it missed.
#[test]
fn a_viewer_held_back_catches_up_with_a_full_frame() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "fill", profile(1, None), screen_filler());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let mut conn = connect(&server);
    let Ok(Message::ScreenAttached { width, height, .. }) =
        conn.request(&Message::ScreenAttach { vm: VmRef::Id(id) })
    else {
        panic!("attach")
    };
    // Not reading: held back.
    std::thread::sleep(Duration::from_millis(1500));
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
    // The screen the paused VM shows, as a viewer that kept up has it.
    let mut late = Viewer::attach(&server, VmRef::Id(id));
    late.idle(Duration::from_millis(200));

    let mut screen = Screen::new(width, height);
    let mut paused = false;
    let mut full_after_pause = false;
    let start = Instant::now();
    while !(full_after_pause && screen == late.screen) {
        assert!(
            start.elapsed() < SLOW,
            "paused {paused}, full frame {full_after_pause}"
        );
        let msg = conn.recv().unwrap();
        match msg {
            Message::ScreenFrame { .. } => {
                let rect = screen.apply_frame(&msg).unwrap();
                full_after_pause |= paused && rect == screen.bounds();
            }
            Message::VmStateChanged {
                state: VmState::Paused,
                ..
            } => paused = true,
            _ => {}
        }
    }
    server.stop().unwrap();
}

/// Stopping a VM sends its viewers its last frame before the state change, as an exit
/// does: a viewer that ends when it hears the VM stopped holds its last screen.
#[test]
fn a_stopped_vm_sends_its_last_frame_first() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "fill", profile(1, None), screen_filler());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let mut viewer = Viewer::attach(&server, VmRef::Id(id));
    viewer.idle(Duration::from_millis(150));
    rt.request_ok(&Message::StopVm { vm: VmRef::Id(id) }).unwrap();
    viewer.wait_for_event(stopped(id));
    let frames = viewer.rects.len();
    let late = Viewer::attach(&server, VmRef::Id(id));
    assert!(
        late.screen == viewer.screen,
        "the last frame came before the state change"
    );
    viewer.idle(Duration::from_millis(100));
    assert_eq!(viewer.rects.len(), frames, "and nothing after it");
    server.stop().unwrap();
}

/// A display the guest has turned off streams as black, whatever its VRAM holds.
#[test]
fn a_display_that_is_off_is_black() {
    // Draws white with the display off; turns it on, draws, turns it off again.
    let program = guest(
        "
    .globl _start
_start:
    li    t0, 0x30000000
    li    t2, 0x31000000
    li    t3, 0xffffff
    sw    t3, 0(t2)
    li    t1, 1
    sw    t1, 0x10(t0)
    sw    t3, 4(t2)
    sw    zero, 0x10(t0)
    xcal.halt zero
",
    );
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "dark", profile(1, None), program);
    let mut viewer = Viewer::attach(&server, VmRef::Id(id));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    viewer.wait_for_event(exited(id));
    assert!(viewer.screen.pixels().iter().all(|&b| b == 0), "black");
    let late = Viewer::attach(&server, VmRef::Id(id));
    assert!(late.screen.pixels().iter().all(|&b| b == 0), "black");
    server.stop().unwrap();
}

/// Input for a VM that is created or paused waits for it, up to `MAX_INPUT_QUEUE`
/// keys and as many pointer changes; past that, and once the VM has ended, input is
/// dropped.
#[test]
fn input_waits_up_to_a_limit_and_not_after_the_end() {
    use avalon::MAX_INPUT_QUEUE;
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "kbd", profile(1, None), example("kbd-echo"));
    let mut console = Console::attach(&server, VmRef::Id(id));
    let mut viewer = Viewer::attach(&server, VmRef::Id(id));
    let queued = || {
        let debug = server.debug_vm(id).unwrap();
        (debug.keys, debug.pointer)
    };
    let to = |x| PointerChange {
        motion: PointerMotion::To { x, y: 0 },
        buttons: 0,
        wheel: 0,
    };

    // Before the start: waits for it.
    viewer.key(key(b'a', false));
    sync(&mut viewer);
    assert_eq!(queued(), (1, 0));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    console.wait_for("a");

    // Paused: waits, up to the limit.
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
    for i in 0..MAX_INPUT_QUEUE + 20 {
        viewer.key(key(b'b', false));
        viewer.pointer(to(i as u32));
    }
    sync(&mut viewer);
    assert_eq!(queued(), (MAX_INPUT_QUEUE, MAX_INPUT_QUEUE));
    rt.request_ok(&Message::ResumeVm { vm: VmRef::Id(id) }).unwrap();
    wait_until("the machine to take the input", SLOW, || queued() == (0, 0));
    // The keyboard's own FIFO keeps the first 64 of them.
    let echoed = format!("a{}", "b".repeat(64));
    assert_eq!(console.wait_for(&echoed), echoed);
    console.idle(Duration::from_millis(100));
    assert_eq!(String::from_utf8_lossy(&console.output), echoed);

    // Ended: dropped.
    viewer.key(key(0x1b, false));
    viewer.wait_for_event(exited(id));
    viewer.key(key(b'c', false));
    viewer.pointer(to(1));
    sync(&mut viewer);
    assert_eq!(queued(), (0, 0), "nothing waits for a VM that has ended");
    server.stop().unwrap();
}

/// Keys reach the keyboard device only from a client attached to the VM's screen.
#[test]
fn keys_need_a_screen_attachment() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "kbd", profile(1, None), example("kbd-echo"));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let mut console = Console::attach(&server, VmRef::Id(id));

    // Not attached to the screen: dropped, unanswered.
    let mut stranger = connect(&server);
    stranger
        .send(&Message::KeyInput {
            vm: id,
            key: key(b'x', false),
        })
        .unwrap();
    assert_eq!(
        stranger.request(&Message::Ping { token: 1 }).unwrap(),
        Message::Pong { token: 1 }
    );

    let mut viewer = Viewer::attach_on(stranger, VmRef::Id(id));
    viewer.key(key(b'h', false));
    viewer.key(key(b'h', true));
    viewer.key(key(b'i', false));
    assert_eq!(console.wait_for("hi"), "hi");

    // Detached again: dropped again.
    assert_eq!(viewer.request(&Message::ScreenDetach { vm: id }), Message::Ok);
    assert_eq!(server.debug_vm(id).unwrap().viewers, 0);
    viewer.key(key(b'z', false));
    match viewer.request(&Message::ScreenDetach { vm: id }) {
        Message::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidState),
        other => panic!("{other:?}"),
    }
    let mut again = Viewer::attach(&server, VmRef::Id(id));
    again.key(key(b'!', false));
    assert_eq!(console.wait_for("!"), "hi!");
    again.key(key(0x1b, false));
    again.wait_for_event(exited(id));
    server.stop().unwrap();
}

/// A client attached to both the console and the screen hears each state change once;
/// destroying the VM detaches it from both.
#[test]
fn console_and_screen_on_one_connection() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "spin", profile(1, None), spinner());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let mut conn = connect(&server);
    assert!(matches!(
        conn.request(&Message::ConsoleAttach { vm: VmRef::Id(id) }),
        Ok(Message::ConsoleAttached { .. })
    ));
    let mut viewer = Viewer::attach_on(conn, VmRef::Id(id));
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
    assert_eq!(
        viewer.request(&Message::Ping { token: 2 }),
        Message::Pong { token: 2 }
    );
    let paused = Message::VmStateChanged {
        vm: id,
        state: VmState::Paused,
    };
    assert_eq!(viewer.events.iter().filter(|e| **e == paused).count(), 1);

    rt.request_ok(&Message::DestroyVm { vm: VmRef::Id(id) }).unwrap();
    for kind in [AttachKind::Console, AttachKind::Screen] {
        viewer.wait_for_event(Message::Detached { vm: id, kind });
    }
    server.stop().unwrap();
}
