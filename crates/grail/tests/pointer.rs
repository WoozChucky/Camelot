//! Xcal pointer end to end, with input injected through `Machine::pointer_event` (the
//! same entry point the framebuffer window uses): the bus registers seen from the hart,
//! the PLIC source 11 line, per-machine state, and `examples/pointer`, whose cursor is
//! checked pixel by pixel.

use excalibur::csr::IRQ_M_EXT;
use grail::bus::{PLIC_BASE, PTR_BASE, RAM_BASE};
use grail::devices::PTR_IRQ;
use grail::framebuffer::{Framebuffer, FB_HEIGHT, FB_WIDTH};
use grail::keyboard::{code, KeyEvent};
use grail::pointer::{
    PointerInput, BUTTONS_MASK, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT, CTRL_IRQ_ENABLE, CTRL_RELATIVE,
    FIFO_LEN,
};
use grail::{Machine, RunResult};
use std::path::Path;

const MEIP: u32 = 1 << IRQ_M_EXT;
const PTR_STATUS: u32 = PTR_BASE;
const PTR_EVENT: u32 = PTR_BASE + 0x4;
const PTR_EVENT_XY: u32 = PTR_BASE + 0x8;
const PTR_CTRL: u32 = PTR_BASE + 0xc;
const PTR_POS: u32 = PTR_BASE + 0x10;
const PENDING: u32 = PLIC_BASE + 0x1000;
const ENABLE0: u32 = PLIC_BASE + 0x2000;
const CLAIM0: u32 = PLIC_BASE + 0x20_0004;
const LOAD_ACCESS_FAULT: u32 = 5;
const STORE_ACCESS_FAULT: u32 = 7;

/// A machine with nops in RAM, pc at RAM, mtvec just past them.
fn bare_machine(program: &[u32]) -> Machine {
    let mut m = Machine::with_ram(64 * 1024);
    let mut code: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    code.extend(std::iter::repeat_n([0x13, 0, 0, 0], 64).flatten()); // nops
    m.cpu.bus.load(RAM_BASE, &code).unwrap();
    m.cpu.pc = RAM_BASE;
    m.cpu.csr.mtvec = RAM_BASE + 0x80;
    m
}

fn read(m: &mut Machine, addr: u32) -> u32 {
    m.cpu.bus.read(addr, 4).unwrap()
}

#[test]
fn line_reaches_the_plic_only_with_ptr_ctrl_set() {
    let mut m = bare_machine(&[]);
    let bus = &mut m.cpu.bus;
    bus.write(PLIC_BASE + 4 * PTR_IRQ, 4, 1).unwrap();
    bus.write(ENABLE0, 4, 1 << PTR_IRQ).unwrap();

    assert!(m.pointer_event(PointerInput::move_to(5, 6)));
    m.cpu.step();
    assert_eq!(read(&mut m, PENDING), 0, "PTR_CTRL.0 is clear");
    assert_eq!(m.cpu.mip() & MEIP, 0);

    m.cpu.bus.write(PTR_CTRL, 4, CTRL_IRQ_ENABLE).unwrap();
    m.cpu.step();
    assert_eq!(read(&mut m, PENDING), 1 << PTR_IRQ);
    assert_eq!(m.cpu.mip() & MEIP, MEIP);

    // Claim, drain, complete: nothing left.
    assert_eq!(read(&mut m, CLAIM0), PTR_IRQ);
    assert_eq!(read(&mut m, PTR_EVENT), 0x8000_0000);
    assert_eq!(read(&mut m, PTR_EVENT_XY), 5 | 6 << 16);
    assert_eq!(read(&mut m, PTR_STATUS), 0);
    m.cpu.bus.write(CLAIM0, 4, PTR_IRQ).unwrap();
    m.cpu.step();
    assert_eq!(read(&mut m, PENDING), 0);
    assert_eq!(m.cpu.mip() & MEIP, 0);
}

#[test]
fn line_stays_high_until_the_fifo_is_drained() {
    let mut m = bare_machine(&[]);
    let bus = &mut m.cpu.bus;
    bus.write(PLIC_BASE + 4 * PTR_IRQ, 4, 1).unwrap();
    bus.write(ENABLE0, 4, 1 << PTR_IRQ).unwrap();
    bus.write(PTR_CTRL, 4, CTRL_IRQ_ENABLE | CTRL_RELATIVE).unwrap();
    m.pointer_event(PointerInput::move_by(3, -2));
    m.pointer_event(PointerInput::buttons(BUTTON_RIGHT).with_wheel(-1));

    // Relative events: dx/dy as signed halfwords, the wheel as a signed byte.
    for (event, xy) in [(0x8000_0008, 0xfffe_0003), (0x8000_ff0a, 0)] {
        m.cpu.step();
        assert_eq!(read(&mut m, CLAIM0), PTR_IRQ);
        assert_eq!(read(&mut m, PTR_EVENT), event);
        assert_eq!(read(&mut m, PTR_EVENT_XY), xy);
        m.cpu.bus.write(CLAIM0, 4, PTR_IRQ).unwrap();
    }
    m.cpu.step();
    assert_eq!(read(&mut m, CLAIM0), 0, "one claim per event, then idle");
    assert_eq!(read(&mut m, PTR_POS), 3, "PTR_POS stays absolute: (3, 0)");
}

#[test]
fn narrow_accesses_fault_and_pop_nothing() {
    // lh t0, 4(t1) with t1 = PTR_BASE, then sb t0, 12(t1)
    let lh = (4 << 20) | (6 << 15) | (1 << 12) | (5 << 7) | 0x03;
    let sb = (5 << 20) | (6 << 15) | (12 << 7) | 0x23;
    let mut m = bare_machine(&[lh]);
    m.cpu.x[6] = PTR_BASE;
    m.pointer_event(PointerInput::move_to(1, 1));
    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, LOAD_ACCESS_FAULT);
    assert_eq!(m.cpu.csr.mtval, PTR_EVENT);
    assert_eq!(m.cpu.bus.ptr.len(), 1);

    let mut m = bare_machine(&[sb]);
    m.cpu.x[5] = CTRL_IRQ_ENABLE;
    m.cpu.x[6] = PTR_BASE;
    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, STORE_ACCESS_FAULT);
    assert_eq!(m.cpu.csr.mtval, PTR_CTRL);
    assert_eq!(m.cpu.bus.ptr.ctrl, 0);
}

#[test]
fn every_machine_has_its_own_pointer() {
    let mut a = Machine::with_ram(64 * 1024);
    let mut b = Machine::with_ram(64 * 1024);
    a.cpu.bus.write(PTR_CTRL, 4, CTRL_RELATIVE).unwrap();
    assert!(a.pointer_event(PointerInput::move_to(10, 10)));
    assert!(b.pointer_event(PointerInput::move_to(20, 30).with_buttons(BUTTON_LEFT)));
    assert_eq!(read(&mut a, PTR_POS), 10 | 10 << 16);
    assert_eq!(read(&mut b, PTR_POS), 20 | 30 << 16);
    assert_eq!(read(&mut a, PTR_CTRL), CTRL_RELATIVE);
    assert_eq!(read(&mut b, PTR_CTRL), 0);
    assert_eq!(read(&mut a, PTR_EVENT), 0x8000_0008);
    assert_eq!(a.cpu.bus.ptr.len(), 0);
    assert_eq!(b.cpu.bus.ptr.len(), 1);
}

// ---------------------------------------------------------------------------
// examples/pointer

const BACKGROUND: u32 = 0x0020_3048;
const WHITE: u32 = 0x00ff_ffff;
const RED: u32 = 0x00ff_0000;
const GREEN: u32 = 0x0000_ff00;
const MAGENTA: u32 = 0x00ff_00ff;
const YELLOW: u32 = 0x00ff_ff00;
const BLUE: u32 = 0x0000_00ff;
const CYAN: u32 = 0x0000_ffff;
const ORANGE: u32 = 0x00ff_8000;

/// The example's cursor: one byte per row, bit 7 = leftmost pixel, tip at the top left.
const ARROW: [u8; 12] = [
    0x80, 0xc0, 0xe0, 0xf0, 0xf8, 0xfc, 0xfe, 0xff, 0xf8, 0xd8, 0x8c, 0x0c,
];

fn load() -> Machine {
    let elf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/pointer/pointer.elf");
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&std::fs::read(elf).unwrap()).unwrap();
    m.cpu.bus.uart.capture = true;
    m
}

/// Runs until the program waits for input again.
fn settle(m: &mut Machine) {
    assert_eq!(m.run(Some(3_000_000), |_| {}), RunResult::StepLimit);
    assert!(m.cpu.waiting, "parked in wfi");
}

/// The whole frame is the background except for the cursor at (`x`, `y`) in `colour`,
/// clipped at the edges.
fn assert_frame(fb: &Framebuffer, x: u32, y: u32, colour: u32) {
    assert!(fb.enabled());
    let mut cursor = 0;
    for py in 0..FB_HEIGHT {
        for px in 0..FB_WIDTH {
            let (row, col) = (py.wrapping_sub(y), px.wrapping_sub(x));
            let on = row < ARROW.len() as u32 && col < 8 && ARROW[row as usize] & (0x80 >> col) != 0;
            let want = if on { colour } else { BACKGROUND };
            cursor += on as u32;
            assert_eq!(fb.pixel(px, py), want, "pixel ({px}, {py}), cursor at ({x}, {y})");
        }
    }
    assert!(cursor > 0, "some of the cursor is on screen");
}

#[test]
fn example_cursor_follows_the_mouse() {
    let mut m = load();
    settle(&mut m);
    assert_frame(&m.cpu.bus.fb, 0, 0, WHITE);
    assert!(m.cpu.bus.fb.take_present());

    let mut events = 0;
    let mut step = |m: &mut Machine, input: PointerInput, x, y, colour| {
        assert!(m.pointer_event(input), "{input:?}");
        events += 1;
        settle(m);
        assert!(m.cpu.bus.fb.take_present(), "{input:?}: presented");
        assert_frame(&m.cpu.bus.fb, x, y, colour);
    };
    step(&mut m, PointerInput::move_to(100, 50), 100, 50, WHITE);
    // A click turns it red until the release; a drag moves it while red.
    step(&mut m, PointerInput::buttons(BUTTON_LEFT), 100, 50, RED);
    step(
        &mut m,
        PointerInput::move_to(200, 300).with_buttons(BUTTON_LEFT),
        200,
        300,
        RED,
    );
    step(&mut m, PointerInput::move_to(200, 300), 200, 300, WHITE);
    step(&mut m, PointerInput::buttons(BUTTON_RIGHT), 200, 300, GREEN);
    step(
        &mut m,
        PointerInput::buttons(BUTTON_LEFT | BUTTON_MIDDLE),
        200,
        300,
        MAGENTA,
    );
    // Every other mix of buttons has its own colour too.
    for (buttons, colour) in [
        (BUTTON_LEFT | BUTTON_RIGHT, YELLOW),
        (BUTTON_MIDDLE, BLUE),
        (BUTTON_RIGHT | BUTTON_MIDDLE, CYAN),
        (BUTTONS_MASK, ORANGE),
    ] {
        step(&mut m, PointerInput::buttons(buttons), 200, 300, colour);
    }
    // The wheel is an event too, but changes nothing on screen.
    step(&mut m, PointerInput::buttons(0).with_wheel(3), 200, 300, WHITE);
    // At the corners the arrow is clipped, not wrapped to the next row or the top.
    step(&mut m, PointerInput::move_to(10_000, 10_000), 639, 479, WHITE);
    step(&mut m, PointerInput::move_to(636, 470), 636, 470, WHITE);
    step(&mut m, PointerInput::move_to(-5, 475), 0, 475, WHITE);

    // Esc ends it, reporting the events it read.
    assert!(m.key_event(KeyEvent::press(code::ESC)));
    assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(0));
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    assert!(
        out.ends_with(&format!("s4 = 0x{events:08x} ({events})\n")),
        "{out}"
    );
}

#[test]
fn example_draws_once_per_batch_of_events() {
    // Events that arrive together are all read in one interrupt: the cursor is drawn
    // only where the last one left it, and the frame is presented once.
    let mut m = load();
    settle(&mut m);
    m.cpu.bus.fb.take_present();
    let presents = m.cpu.bus.fb.presents;
    for i in 0..10 {
        m.pointer_event(PointerInput::move_to(30 * i, 20 * i));
    }
    m.pointer_event(PointerInput::move_to(300, 200).with_buttons(BUTTON_RIGHT));
    settle(&mut m);
    assert_eq!(m.cpu.bus.fb.presents, presents + 1);
    assert_frame(&m.cpu.bus.fb, 300, 200, GREEN);
}

#[test]
fn example_catches_up_after_an_overflow() {
    // More events than the FIFO holds arrive before the program reads any: it reads the
    // 64 that were kept, sees PTR_STATUS.OVERFLOW and takes the final position and
    // buttons from PTR_POS and PTR_BUTTONS.
    let mut m = load();
    for i in 1..=FIFO_LEN as i32 + 10 {
        m.pointer_event(PointerInput::move_to(i, 2 * i));
    }
    m.pointer_event(PointerInput::move_to(400, 100).with_buttons(BUTTON_LEFT));
    assert_eq!(m.cpu.bus.ptr.dropped, 11);
    settle(&mut m);
    assert_frame(&m.cpu.bus.fb, 400, 100, RED);
    assert_eq!(read(&mut m, PTR_STATUS), 0, "overflow acknowledged");
    assert!(m.key_event(KeyEvent::press(code::ESC)));
    assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(0));
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    assert!(out.ends_with("s4 = 0x00000040 (64)\n"), "{out}");
}

#[test]
fn example_waits_for_input() {
    // Without input the program sleeps: no new frames, no output.
    let mut m = load();
    settle(&mut m);
    m.cpu.bus.fb.take_present();
    for _ in 0..3 {
        settle(&mut m);
        assert!(!m.cpu.bus.fb.take_present());
    }
    // An input that changes nothing makes no event and wakes nothing.
    assert!(!m.pointer_event(PointerInput::move_to(0, 0)));
    settle(&mut m);
    assert!(!m.cpu.bus.fb.take_present());
    assert!(m.cpu.bus.uart.captured.is_empty());
}
