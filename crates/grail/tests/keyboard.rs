//! Xcal keyboard end to end, with events injected through `Machine::key_event` (the
//! same entry point the framebuffer window uses): the bus registers seen from the hart,
//! the PLIC source 2 line, and both `examples/kbd-echo` programs.

use excalibur::csr::IRQ_M_EXT;
use grail::bus::{KBD_BASE, PLIC_BASE, RAM_BASE};
use grail::devices::KBD_IRQ;
use grail::keyboard::{code, KeyEvent, FIFO_LEN};
use grail::{Machine, RunResult};
use std::path::{Path, PathBuf};
use std::process::Command;

const MEIP: u32 = 1 << IRQ_M_EXT;
const KBD_STATUS: u32 = KBD_BASE;
const KBD_DATA: u32 = KBD_BASE + 4;
const KBD_CTRL: u32 = KBD_BASE + 8;
const PENDING: u32 = PLIC_BASE + 0x1000;
const ENABLE0: u32 = PLIC_BASE + 0x2000;
const CLAIM0: u32 = PLIC_BASE + 0x20_0004;
const LOAD_ACCESS_FAULT: u32 = 5;

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/kbd-echo")
        .join(name)
}

fn load(name: &str) -> Machine {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&std::fs::read(example(name)).unwrap()).unwrap();
    m.cpu.bus.uart.capture = true;
    m
}

fn output(m: &mut Machine) -> String {
    String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap()
}

/// A press and a release of `code`.
fn tap(m: &mut Machine, ev: KeyEvent) {
    assert!(m.key_event(ev));
    assert!(m.key_event(KeyEvent { release: true, ..ev }));
}

/// Taps every character of `text` (Shift for uppercase, as the window reports it).
fn type_text(m: &mut Machine, text: &str) {
    for c in text.bytes() {
        tap(m, KeyEvent::press(c).with_shift(c.is_ascii_uppercase()));
    }
}

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

#[test]
fn line_reaches_the_plic_only_with_kbd_ctrl_set() {
    let mut m = bare_machine(&[]);
    let bus = &mut m.cpu.bus;
    bus.write(PLIC_BASE + 4 * KBD_IRQ, 4, 1).unwrap();
    bus.write(ENABLE0, 4, 1 << KBD_IRQ).unwrap();

    m.key_event(KeyEvent::press(b'a'));
    m.cpu.step();
    assert_eq!(m.cpu.bus.read(PENDING, 4).unwrap(), 0, "KBD_CTRL.0 is clear");
    assert_eq!(m.cpu.mip() & MEIP, 0);

    m.cpu.bus.write(KBD_CTRL, 4, 1).unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.bus.read(PENDING, 4).unwrap(), 1 << KBD_IRQ);
    assert_eq!(m.cpu.mip() & MEIP, MEIP);

    // Claim, drain, complete: nothing left.
    assert_eq!(m.cpu.bus.read(CLAIM0, 4).unwrap(), KBD_IRQ);
    assert_eq!(m.cpu.bus.read(KBD_DATA, 4).unwrap(), b'a' as u32);
    assert_eq!(m.cpu.bus.read(KBD_STATUS, 4).unwrap(), 0);
    m.cpu.bus.write(CLAIM0, 4, KBD_IRQ).unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.bus.read(PENDING, 4).unwrap(), 0);
    assert_eq!(m.cpu.mip() & MEIP, 0);
}

#[test]
fn line_stays_high_until_the_fifo_is_drained() {
    let mut m = bare_machine(&[]);
    let bus = &mut m.cpu.bus;
    bus.write(PLIC_BASE + 4 * KBD_IRQ, 4, 1).unwrap();
    bus.write(ENABLE0, 4, 1 << KBD_IRQ).unwrap();
    bus.write(KBD_CTRL, 4, 1).unwrap();
    m.key_event(KeyEvent::press(b'x'));
    m.key_event(KeyEvent::release(b'x'));

    for want in [b'x' as u32, b'x' as u32 | 0x100] {
        m.cpu.step();
        assert_eq!(m.cpu.bus.read(CLAIM0, 4).unwrap(), KBD_IRQ);
        assert_eq!(m.cpu.bus.read(KBD_DATA, 4).unwrap(), want);
        m.cpu.bus.write(CLAIM0, 4, KBD_IRQ).unwrap();
    }
    m.cpu.step();
    assert_eq!(
        m.cpu.bus.read(CLAIM0, 4).unwrap(),
        0,
        "one claim per event, then idle"
    );
}

#[test]
fn byte_loads_from_kbd_data_fault_and_pop_nothing() {
    // lb t0, 4(t1) with t1 = KBD_BASE
    let mut m = bare_machine(&[(4 << 20) | (6 << 15) | (5 << 7) | 0x03]);
    m.cpu.x[6] = KBD_BASE;
    m.key_event(KeyEvent::press(b'z'));
    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, LOAD_ACCESS_FAULT);
    assert_eq!(m.cpu.csr.mtval, KBD_DATA);
    assert_eq!(m.cpu.bus.kbd.len(), 1);
}

#[test]
fn polling_example_echoes_presses() {
    let mut m = load("kbd-echo.elf");
    type_text(&mut m, "Hi");
    tap(&mut m, KeyEvent::press(b'c').with_ctrl(true));
    tap(&mut m, KeyEvent::press(b'!').with_shift(true));
    tap(&mut m, KeyEvent::press(code::BACKSPACE));
    for c in [code::UP, code::RIGHT, code::F1, code::F12, code::TAB, code::ENTER] {
        tap(&mut m, KeyEvent::press(c));
    }
    tap(&mut m, KeyEvent::press(code::ESC));
    assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    assert_eq!(
        output(&mut m),
        "Hi^c!\x08 \x08[80][83][90][9b]\t\n[dbg] pc=80000054 s1 = 0x0000000c (12)\n"
    );
    assert_eq!(m.cpu.bus.kbd.len(), 1, "only the release of Esc is left");
}

#[test]
fn polling_example_waits_for_keys_between_slices() {
    // Like the window does: events arrive while the program is already running.
    let mut m = load("kbd-echo.elf");
    assert_eq!(m.run(Some(10_000), |_| {}), RunResult::StepLimit);
    assert_eq!(output(&mut m), "", "no keys, no output");

    for (i, c) in "abc".bytes().enumerate() {
        m.key_event(KeyEvent::press(c));
        assert_eq!(m.run(Some(10_000), |_| {}), RunResult::StepLimit);
        assert_eq!(output(&mut m), &"abc"[i..=i]);
        m.key_event(KeyEvent::release(c));
        assert_eq!(m.run(Some(10_000), |_| {}), RunResult::StepLimit);
        assert_eq!(output(&mut m), "", "releases are not echoed");
    }
    m.key_event(KeyEvent::press(code::ESC));
    assert_eq!(m.run(Some(10_000), |_| {}), RunResult::Exited(0));
    assert!(output(&mut m).ends_with("s1 = 0x00000004 (4)\n"));
}

#[test]
fn full_fifo_drops_what_does_not_fit() {
    let mut m = load("kbd-echo.elf");
    for i in 0..FIFO_LEN {
        assert!(m.key_event(KeyEvent::press(b'a' + (i % 26) as u8)));
    }
    assert!(
        !m.key_event(KeyEvent::press(code::ESC)),
        "dropped: the FIFO is full"
    );
    assert_eq!(m.cpu.bus.kbd.dropped, 1);
    assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::StepLimit);
    let want: String = (0..FIFO_LEN).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    assert_eq!(output(&mut m), want);
    // Room again: Esc gets through now.
    assert!(m.key_event(KeyEvent::press(code::ESC)));
    assert_eq!(m.run(Some(10_000), |_| {}), RunResult::Exited(0));
}

#[test]
fn interrupt_example_takes_one_interrupt_per_event() {
    let mut m = load("kbd-echo-irq.elf");
    type_text(&mut m, "Hello");
    tap(&mut m, KeyEvent::press(code::DOWN));
    tap(&mut m, KeyEvent::press(code::ENTER));
    assert!(m.key_event(KeyEvent::press(code::ESC)));
    assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    // 7 taps (14 events) plus the Esc press.
    assert_eq!(
        output(&mut m),
        "Hello[81]\n[dbg] pc=800000ac s0 = 0x0000000f (15)\n"
    );
}

#[test]
fn interrupt_example_sleeps_until_a_key_arrives() {
    let mut m = load("kbd-echo-irq.elf");
    assert_eq!(m.run(Some(10_000), |_| {}), RunResult::StepLimit);
    assert!(m.cpu.waiting, "parked in wfi");
    assert_eq!(output(&mut m), "");

    for (i, c) in "xyz".bytes().enumerate() {
        tap(&mut m, KeyEvent::press(c));
        assert_eq!(m.run(Some(10_000), |_| {}), RunResult::StepLimit);
        assert!(m.cpu.waiting, "back in wfi");
        assert_eq!(output(&mut m), &"xyz"[i..=i]);
    }
    assert!(m.key_event(KeyEvent::press(code::ESC)));
    assert_eq!(m.run(Some(10_000), |_| {}), RunResult::Exited(0));
    assert!(output(&mut m).ends_with("s0 = 0x00000007 (7)\n"));
}

#[test]
fn without_a_window_the_keyboard_stays_empty() {
    // Headless (the default): nothing produces key events, so the polling example
    // waits until the step limit, and stdin goes to the UART, not the keyboard.
    for args in [&[][..], &["--fb", "--headless"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_grail"))
            .args(args)
            .args(["--max-steps", "200000"])
            .arg(example("kbd-echo.elf"))
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(124), "args {args:?}");
        assert!(out.stdout.is_empty());
    }
}
