//! Framebuffer end to end: runs `examples/colorbars` headless through the `grail`
//! binary, dumps PPM frames, and checks them against the expected test pattern.

use grail::display::{self, DisplayOptions, Outcome};
use grail::framebuffer::{FB_HEIGHT, FB_WIDTH};
use grail::{Machine, MachineConfig, RunResult};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// FNV-1a 64 of the expected `frame-*.ppm` for `examples/colorbars`. If the pattern
/// changes on purpose, update `expected_pixel` and this hash together.
const COLORBARS_PPM_FNV1A: u64 = 0x180c_7adf_4a97_66f1;

const PPM_HEADER: &[u8] = b"P6\n640 480\n255\n";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn colorbars_elf() -> PathBuf {
    repo_root().join("examples/colorbars/colorbars.elf")
}

/// A fresh, empty directory under cargo's per-target scratch space.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the `grail` binary. A run that takes more than a minute is killed and
/// fails the test, so a regression that opens a window (which waits for a
/// person to close it) or never stops cannot hang the test run.
///
/// stdout and stderr are drained on their own threads while the child runs:
/// a pipe holds only a few KiB (4 KiB on Windows), and a child that fills one
/// blocks on its write and would never exit if they were read only after it
/// had.
fn grail(args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot run grail");
    fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).unwrap();
            buf
        })
    }
    let stdout = drain(child.stdout.take().unwrap());
    let stderr = drain(child.stderr.take().unwrap());
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("grail {args:?} still running after a minute");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    std::process::Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

/// A run that writes far more than a pipe holds still finishes and comes back
/// whole: `grail()` reads the output while the child runs.
#[test]
fn helper_returns_large_output() {
    let dir = scratch("colorbars-trace");
    let out = grail(&[
        "--max-steps",
        "5000",
        "--trace",
        "--fb-dump",
        dir.to_str().unwrap(),
        colorbars_elf().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(124));
    assert!(
        out.stderr.len() > 64 * 1024,
        "{} bytes of trace",
        out.stderr.len()
    );
}

/// The pattern `colorbars.S` draws, as 0x00RRGGBB: an 8-pixel grey border around
/// eight vertical bars of 80 pixels.
fn expected_pixel(x: u32, y: u32) -> u32 {
    const BARS: [u32; 8] = [
        0xffffff, 0xffff00, 0x00ffff, 0x00ff00, 0xff00ff, 0xff0000, 0x0000ff, 0x000000,
    ];
    if x < 8 || y < 8 || x >= FB_WIDTH - 8 || y >= FB_HEIGHT - 8 {
        0x808080
    } else {
        BARS[(x / (FB_WIDTH / 8)) as usize]
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn check_colorbars_ppm(ppm: &[u8]) {
    assert_eq!(&ppm[..PPM_HEADER.len()], PPM_HEADER, "PPM header");
    let pixels = &ppm[PPM_HEADER.len()..];
    assert_eq!(pixels.len(), (FB_WIDTH * FB_HEIGHT * 3) as usize, "PPM size");
    for y in 0..FB_HEIGHT {
        for x in 0..FB_WIDTH {
            let i = ((y * FB_WIDTH + x) * 3) as usize;
            let got = u32::from_be_bytes([0, pixels[i], pixels[i + 1], pixels[i + 2]]);
            let want = expected_pixel(x, y);
            assert_eq!(got, want, "pixel ({x}, {y}): got {got:06x}, want {want:06x}");
        }
    }
    assert_eq!(fnv1a(ppm), COLORBARS_PPM_FNV1A, "PPM hash");
}

#[test]
fn colorbars_headless_dump() {
    let dir = scratch("colorbars");
    let out = grail(&[
        "--fb-dump",
        dir.to_str().unwrap(),
        colorbars_elf().to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut frames: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    frames.sort();
    // One frame for the FB_PRESENT, one at exit.
    assert_eq!(frames, ["frame-0000.ppm", "frame-0001.ppm"]);
    for f in &frames {
        check_colorbars_ppm(&std::fs::read(dir.join(f)).unwrap());
    }
}

#[test]
fn headless_overrides_fb() {
    // --headless wins over --fb, so this never opens a window, even on Windows.
    let dir = scratch("colorbars-headless");
    let out = grail(&[
        "--fb",
        "--headless",
        "--fb-dump",
        dir.to_str().unwrap(),
        colorbars_elf().to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    check_colorbars_ppm(&std::fs::read(dir.join("frame-0001.ppm")).unwrap());
}

#[test]
fn step_limit_dumps_the_disabled_display() {
    // Stopped long before FB_CTRL is written: one frame at exit, all black.
    let dir = scratch("colorbars-step-limit");
    let out = grail(&[
        "--max-steps",
        "1000",
        "--fb-dump",
        dir.to_str().unwrap(),
        colorbars_elf().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(124));
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["frame-0000.ppm"]);
    let ppm = std::fs::read(dir.join("frame-0000.ppm")).unwrap();
    assert_eq!(&ppm[..PPM_HEADER.len()], PPM_HEADER);
    assert!(ppm[PPM_HEADER.len()..].iter().all(|&b| b == 0));
}

#[test]
fn fb_dump_needs_a_directory() {
    let out = grail(&["--fb-dump"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn run_for_stops_right_after_present() {
    let file = std::fs::read(colorbars_elf()).unwrap();
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&file).unwrap();

    // Bounded budgets: a regression must fail, not hang.
    const BUDGET: u64 = 10_000_000;
    let steps = m.run_for(BUDGET, &mut |_| {});
    assert!(steps > 640 * 480, "drew the whole frame first ({steps} steps)");
    assert!(steps < BUDGET, "stopped at the present");
    assert_eq!(m.exit_code(), None);
    let fb = &mut m.cpu.bus.fb;
    assert!(fb.enabled());
    assert!(fb.take_present());
    assert_eq!(fb.presents, 1);
    for (x, y) in [
        (0, 0),
        (7, 7),
        (8, 8),
        (100, 240),
        (639, 479),
        (560, 100),
        (559, 100),
    ] {
        assert_eq!(fb.pixel(x, y), expected_pixel(x, y), "pixel ({x}, {y})");
    }
    check_colorbars_ppm(&m.cpu.bus.frame().to_ppm());

    // The next instruction is xcal.halt.
    assert_eq!(m.run_for(BUDGET, &mut |_| {}), 1);
    assert_eq!(m.exit_code(), Some(0));
    assert_eq!(m.run_for(10, &mut |_| {}), 0);
}

#[test]
fn run_for_stops_only_at_a_present_of_its_own() {
    // A present the host has not taken yet is not a reason to stop again: the
    // next call runs its whole budget.
    let file = std::fs::read(colorbars_elf()).unwrap();
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&file).unwrap();
    let present = grail::bus::FB_CTRL_BASE + u64::from(grail::framebuffer::REG_PRESENT);
    m.cpu.bus.write(present, 4, 1).unwrap();
    assert!(m.cpu.bus.fb.present_pending());
    assert_eq!(m.run_for(1000, &mut |_| {}), 1000);
    assert_eq!(m.run_for(1000, &mut |_| {}), 1000);
    assert!(m.cpu.bus.fb.take_present(), "still pending for the host");
}

/// Runs one load or store with `a0` = `addr` and `a1` = 0x0000_0101 at
/// RAM_BASE; returns the machine and, if it trapped, (mcause, mtval).
fn one_access(inst: u32, addr: u64) -> (Machine, Option<(u64, u64)>) {
    use grail::bus::RAM_BASE;
    let trap_vector = RAM_BASE + 0x4000;
    let mut m = Machine::with_ram(1 << 20);
    m.cpu.bus.load(RAM_BASE, &inst.to_le_bytes()).unwrap();
    m.cpu.pc = RAM_BASE;
    m.cpu.csr.mtvec = trap_vector;
    m.cpu.set_reg(10, addr);
    m.cpu.x[11] = 0x0000_0101;
    m.cpu.step();
    let trap = (m.cpu.pc == trap_vector).then_some((m.cpu.csr.mcause, m.cpu.csr.mtval));
    (m, trap)
}

#[test]
fn programs_see_the_register_rules_as_traps() {
    // Spec 9.3 as a program sees it: the registers take 32-bit accesses
    // only, VRAM is plain memory, with the alignment rules of RAM.
    use grail::bus::{FB_CTRL_BASE, VRAM_BASE};
    use grail::framebuffer::{REG_CTRL, REG_PRESENT, VRAM_SIZE};
    // `lb/lh/lw a2, 0(a0)` and `sb/sh/sw a1, 0(a0)`.
    let load = |funct3: u32| 10 << 15 | funct3 << 12 | 12 << 7 | 0x03;
    let store = |funct3: u32| 11 << 20 | 10 << 15 | funct3 << 12 | 0x23;
    let (lb, lh, lw, sb, sh, sw) = (load(0), load(1), load(2), store(0), store(1), store(2));
    let ctrl = FB_CTRL_BASE + u64::from(REG_CTRL);
    let present = FB_CTRL_BASE + u64::from(REG_PRESENT);
    for (inst, addr, cause) in [
        (lb, ctrl, 5),
        (lh, ctrl, 5),
        (sb, ctrl, 7),
        (sh, ctrl, 7),
        (sb, present, 7),
        (lw, FB_CTRL_BASE + 2, 4), // misaligned
        (sw, FB_CTRL_BASE + 2, 6),
        (lw, VRAM_BASE + u64::from(VRAM_SIZE), 5), // just past VRAM
        (sb, VRAM_BASE + u64::from(VRAM_SIZE), 7),
        (lw, VRAM_BASE + 2, 4),
    ] {
        let (m, trap) = one_access(inst, addr);
        assert_eq!(trap, Some((cause, addr)), "{inst:#010x} at {addr:#x}");
        // No side effect: the display is still off and nothing was presented.
        assert_eq!(
            (m.cpu.bus.fb.ctrl, m.cpu.bus.fb.presents),
            (0, 0),
            "{inst:#010x} at {addr:#x}"
        );
    }
    // The 32-bit forms work.
    let (m, trap) = one_access(sw, ctrl);
    assert_eq!((trap, m.cpu.bus.fb.ctrl), (None, 1));
    let (m, trap) = one_access(sw, present);
    assert_eq!((trap, m.cpu.bus.fb.presents), (None, 1));
    // A byte of VRAM loads sign-extended, as from RAM.
    let (mut m, _) = one_access(0x0000_0013, 0); // nop: just a machine
    m.cpu.bus.write(VRAM_BASE + 5, 1, 0x80).unwrap();
    m.cpu.bus.load(grail::bus::RAM_BASE, &lb.to_le_bytes()).unwrap();
    m.cpu.pc = grail::bus::RAM_BASE;
    m.cpu.set_reg(10, VRAM_BASE + 5);
    m.cpu.step();
    assert_eq!(m.cpu.x[12] as u32, 0xffff_ff80);
}

#[test]
fn colorbars_runs_under_plain_run() {
    // Machine::run ignores FB_PRESENT and just runs to the end.
    let file = std::fs::read(colorbars_elf()).unwrap();
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&file).unwrap();
    assert_eq!(m.run(Some(10_000_000), |_| {}), RunResult::Exited(0));
    assert_eq!(m.cpu.bus.fb.presents, 1);
}

#[test]
fn display_run_follows_a_real_time_clock() {
    // The host loop behind --fb and --fb-dump: at a clock that makes colorbars last at
    // least 0.25 s, it must take that long and still dump the right frames.
    let file = std::fs::read(colorbars_elf()).unwrap();
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&file).unwrap();
    let mut steps = 0u64;
    assert_eq!(m.run(None, |_| steps += 1), RunResult::Exited(0));
    let hz = u32::try_from(steps * 4).unwrap();

    let mut m = Machine::new(MachineConfig {
        clock_hz: Some(hz),
        ..MachineConfig::default()
    })
    .unwrap();
    m.load_elf(&file).unwrap();
    let dir = scratch("colorbars-clock");
    let opts = DisplayOptions {
        dump_dir: Some(dir.clone()),
        ..DisplayOptions::default()
    };
    let t = Instant::now();
    let outcome = display::run(&mut m, None, |_| {}, &opts).unwrap();
    let elapsed = t.elapsed();
    assert_eq!(outcome, Outcome::Program(RunResult::Exited(0)));
    assert!(
        elapsed >= Duration::from_millis(240),
        "{steps} steps at {hz} Hz in {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    check_colorbars_ppm(&std::fs::read(dir.join("frame-0000.ppm")).unwrap());
    check_colorbars_ppm(&std::fs::read(dir.join("frame-0001.ppm")).unwrap());
}

/// `examples/flip`, for an RV32 or an RV64 machine.
fn flip_elf(rv64: bool) -> PathBuf {
    let name = if rv64 { "flip-rv64.elf" } else { "flip.elf" };
    repo_root().join("examples/flip").join(name)
}

/// Frames `examples/flip` shows, and the bands it draws: row `y` of frame `k` is
/// `BANDS[(y + 25 k) / 75 % 8]`, as 0x00RRGGBB.
const FLIP_FRAMES: u32 = 8;
const BANDS: [u32; 8] = [
    0xffffff, 0xffff00, 0x00ffff, 0x00ff00, 0xff00ff, 0xff0000, 0x0000ff, 0x000000,
];

/// The PPM of frame `k` of `examples/flip`: 800x600, every row one band's colour.
fn flip_ppm(k: u32) -> Vec<u8> {
    let mut ppm = b"P6\n800 600\n255\n".to_vec();
    for y in 0..600 {
        let [_, r, g, b] = BANDS[((y + 25 * k) / 75 % 8) as usize].to_be_bytes();
        for _ in 0..800 {
            ppm.extend_from_slice(&[r, g, b]);
        }
    }
    ppm
}

/// The frames dumped into `dir` are each of `examples/flip`'s eight frames in turn
/// (frame-0000 from the modeset, the next seven from the page flips), then the last one
/// again at exit. A dump is written as a flip takes effect, so it shows the new buffer
/// whole whatever the program does: this checks `--fb-dump`, not tearing
/// ([`the_display_never_shows_a_buffer_being_drawn`] does).
fn check_flip_dumps(dir: &Path) {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let want: Vec<String> = (0..=FLIP_FRAMES).map(|i| format!("frame-{i:04}.ppm")).collect();
    assert_eq!(names, want);
    for (i, name) in names.iter().enumerate() {
        let k = (i as u32).min(FLIP_FRAMES - 1);
        let got = std::fs::read(dir.join(name)).unwrap();
        assert!(got == flip_ppm(k), "{name} is not frame {k} whole");
    }
}

/// `examples/flip` through `grail --fb-dump`, at both widths: one dump per frame it
/// shows, at 800x600, in order.
#[test]
fn flip_dumps_every_frame_it_shows() {
    for rv64 in [false, true] {
        let dir = scratch(&format!("flip-{}", if rv64 { 64 } else { 32 }));
        let out = grail(&[
            "--fb-dump",
            dir.to_str().unwrap(),
            flip_elf(rv64).to_str().unwrap(),
        ]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        check_flip_dumps(&dir);
    }
}

/// Acceptance of #176: `examples/flip` switches to 800x600 and double-buffers in RAM
/// with page flips on the vertical-blank interrupt, and shows its moving bands with no
/// tearing, at both widths.
///
/// The display scans out the buffer it shows for a whole frame, from one vertical blank
/// to the next, so a frame is torn if that buffer is drawn into meanwhile. The machine
/// runs a step at a time, and the frame shown is checked at the start of every frame
/// (right after the modeset or the vertical blank that began it) and at its end (right
/// before the tick of the next vertical blank): both times it must be one of the eight
/// frames whole, the same one, and never an earlier one than the frame before. A
/// program that drew into the buffer on screen (one that went on without waiting for
/// FLIP_PENDING to clear) fails at the end of the frame it tore.
#[test]
fn the_display_never_shows_a_buffer_being_drawn() {
    let frames: Vec<Vec<u8>> = (0..FLIP_FRAMES).map(flip_ppm).collect();
    // Which of the eight frames the display shows whole, if any.
    let shown = |m: &Machine| {
        let ppm = m.cpu.bus.frame().to_ppm();
        frames.iter().position(|f| *f == ppm).map(|k| k as u32)
    };
    for rv64 in [false, true] {
        let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
        m.load_elf(&std::fs::read(flip_elf(rv64)).unwrap()).unwrap();
        // The frame shown since the current frame began; None before the modeset.
        let mut current: Option<u32> = None;
        let mut seen = Vec::new();
        let (mut ends, mut first_vblank) = (0u64, 0u64);
        let mut steps = 0u64;
        while m.exit_code().is_none() {
            assert!(steps < 50_000_000, "never halts");
            let fb = &m.cpu.bus.fb;
            if let Some(k) = current.filter(|_| fb.ticks_to_vblank() == 1) {
                // The end of a frame: the buffer shown all along it is still the same
                // frame, whole.
                assert_eq!(
                    shown(&m),
                    Some(k),
                    "torn: the end of the frame before vertical blank {} (rv64 {rv64})",
                    fb.vblanks + 1
                );
                ends += 1;
            }
            let (modesets, vblanks) = (fb.modesets, fb.vblanks);
            m.cpu.step();
            steps += 1;
            let fb = &m.cpu.bus.fb;
            if fb.modesets != modesets || (current.is_some() && fb.vblanks != vblanks) {
                // A frame begins, with the buffer the modeset or the flip latched.
                if current.is_none() {
                    first_vblank = fb.vblanks;
                }
                let k = shown(&m).unwrap_or_else(|| {
                    panic!(
                        "torn: the start of the frame at vertical blank {} (rv64 {rv64})",
                        fb.vblanks
                    )
                });
                assert!(
                    current.is_none_or(|last| k >= last),
                    "frame {k} after frame {current:?}"
                );
                current = Some(k);
                if seen.last() != Some(&k) {
                    seen.push(k);
                }
            }
        }
        assert_eq!(m.exit_code(), Some(0));
        let fb = &m.cpu.bus.fb;
        assert_eq!((fb.mode().width, fb.mode().height), (800, 600));
        assert_eq!(
            seen,
            (0..FLIP_FRAMES).collect::<Vec<_>>(),
            "each frame shown, in order"
        );
        // Every frame since the modeset was checked at its end: no idle jump skipped one.
        assert!(ends > u64::from(FLIP_FRAMES), "{ends} frames");
        assert_eq!(ends, fb.vblanks - first_vblank, "a frame's end was skipped");
    }
}

/// A hart waiting in `wfi` for the vertical blank wakes at each one: the jump over
/// idle time stops short of it, so every vertical blank is an interrupt of its own
/// (the handler counts them in s8), none merged into the next, and the idle time is
/// skipped rather than run a tick per step.
#[test]
fn a_waiting_hart_wakes_at_every_vertical_blank() {
    for rv64 in [false, true] {
        let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
        m.load_elf(&std::fs::read(flip_elf(rv64)).unwrap()).unwrap();
        // Frame 0 is drawn before the interrupt is enabled: those vertical blanks
        // interrupt nobody.
        let mut steps = 0u64;
        while !m.cpu.bus.fb.vblank_irq_enabled() {
            m.cpu.step();
            steps += 1;
            assert!(steps < 10_000_000, "the interrupt is never enabled");
        }
        let before = m.cpu.bus.fb.vblanks;
        assert_eq!(m.run(Some(50_000_000), |_| steps += 1), RunResult::Exited(0));
        let fb = &m.cpu.bus.fb;
        assert_eq!((fb.modesets, fb.flips), (1, u64::from(FLIP_FRAMES) - 1));
        assert_eq!((fb.mode().width, fb.mode().height), (800, 600));
        let interrupts = m.cpu.x[24]; // s8
        assert!(interrupts > 0);
        assert_eq!(
            interrupts,
            fb.vblanks - before,
            "one interrupt per vertical blank"
        );
        assert!(m.cpu.bus.clint.mtime > steps, "idle time skipped");
        // The last frame is what the display shows, from RAM.
        let frame = m.cpu.bus.frame();
        assert_eq!((frame.width, frame.height), (800, 600));
        assert!(frame.to_ppm() == flip_ppm(FLIP_FRAMES - 1));
    }
}

/// Paced by a real-time clock, the vertical blank wakes the waiting hart too (the
/// pacer's idle skip stops short of it), and the dumps are the same.
#[test]
fn flip_follows_a_real_time_clock() {
    let mut m = Machine::new(MachineConfig {
        clock_hz: Some(400_000_000),
        ..MachineConfig::default()
    })
    .unwrap();
    m.load_elf(&std::fs::read(flip_elf(false)).unwrap()).unwrap();
    let dir = scratch("flip-clock");
    let opts = DisplayOptions {
        dump_dir: Some(dir.clone()),
        ..DisplayOptions::default()
    };
    let outcome = display::run(&mut m, None, |_| {}, &opts).unwrap();
    assert_eq!(outcome, Outcome::Program(RunResult::Exited(0)));
    // At 400 MHz a frame is 6.7 M ticks, more than drawing frame 0 takes, so every
    // vertical blank comes after the interrupt is enabled.
    assert_eq!(
        m.cpu.x[24], m.cpu.bus.fb.vblanks,
        "one interrupt per vertical blank"
    );
    check_flip_dumps(&dir);
}
