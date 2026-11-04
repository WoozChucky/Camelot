//! Host side of the framebuffer: runs the machine in slices and, between slices,
//! shows the frame in a window (Windows only) and/or dumps it as PPM files.
//!
//! Everything happens on the calling thread, and the window belongs to this call, not to
//! a process-wide singleton: each machine can have its own. The CPU runs until the
//! program shows a new frame (writes `FB_PRESENT` or `FB_MODE`, or a page flip takes
//! effect at a vertical blank) or a slice of `SLICE` steps is done (or, with a real-time
//! clock, until it is ahead of the wall clock); then the window's messages are pumped
//! and it is redrawn on a present, on a repaint request, or when ~16 ms of host time
//! have passed since the last redraw (~60 Hz). The window shows what the framebuffer
//! scans out, VRAM or a buffer in RAM, at the size of its mode. Keys typed into the
//! keyboard device, and the mouse over it the pointer device, at the same points,
//! between slices.

use crate::cpu::Cpu;
use crate::framebuffer::FrameDumper;
use crate::{Machine, RunResult};
use std::path::PathBuf;

/// Steps between two looks at the host side (window messages, redraw timer).
const SLICE: u64 = 20_000;
/// Longest sleep between two looks at the host side when a real-time clock is pacing
/// the machine.
const MAX_WAIT: std::time::Duration = std::time::Duration::from_millis(10);

#[derive(Default)]
pub struct DisplayOptions {
    /// Open a window showing the framebuffer. Ignored where `WINDOW_SUPPORTED` is false.
    pub window: bool,
    /// Write `frame-NNNN.ppm` here on every new frame shown (`FB_PRESENT`, a modeset, a
    /// page flip taking effect) and once at exit.
    pub dump_dir: Option<PathBuf>,
    /// Window title.
    pub title: String,
}

/// Whether this build can open a framebuffer window.
pub const WINDOW_SUPPORTED: bool = cfg!(windows);

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The program stopped (or hit the step limit).
    Program(RunResult),
    /// The user closed the window while the program was still running.
    WindowClosed,
}

/// Runs the machine with the framebuffer host attached. With a window, after the
/// program stops the window stays open on the last frame until the user closes it.
pub fn run(
    m: &mut Machine,
    max_steps: Option<u64>,
    mut trace: impl FnMut(&Cpu),
    opts: &DisplayOptions,
) -> Result<Outcome, String> {
    let mut dumper = match &opts.dump_dir {
        Some(dir) => {
            Some(FrameDumper::new(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?)
        }
        None => None,
    };
    let mut dump = |m: &Machine| -> Result<(), String> {
        if let Some(d) = &mut dumper {
            d.dump(&m.cpu.bus.frame())
                .map_err(|e| format!("cannot write frame: {e}"))?;
        }
        Ok(())
    };
    let mut screen = Screen::open(opts)?;

    let mut steps = 0u64;
    let outcome = loop {
        if let Some(code) = m.exit_code() {
            break Outcome::Program(RunResult::Exited(code));
        }
        let budget = max_steps.map_or(SLICE, |max| SLICE.min(max - steps));
        if budget == 0 {
            break Outcome::Program(RunResult::StepLimit);
        }
        steps += m.run_for(budget, &mut trace);
        let presented = m.cpu.bus.fb.take_present();
        if presented {
            dump(m)?;
        }
        if !screen.update(m, presented) {
            break Outcome::WindowClosed;
        }
        // Real-time clock: wait for the wall clock to catch up, but keep pumping the
        // window at least every `MAX_WAIT`.
        if let Some(d) = m.pace_delay() {
            std::thread::sleep(d.min(MAX_WAIT));
        }
    };
    dump(m)?;
    if let Outcome::Program(r) = &outcome {
        screen.hold(m, r);
    }
    Ok(outcome)
}

#[cfg(windows)]
struct Screen {
    window: Option<crate::window::Window>,
    title: String,
    last_draw: std::time::Instant,
}

#[cfg(windows)]
impl Screen {
    const REFRESH: std::time::Duration = std::time::Duration::from_millis(16);

    fn open(opts: &DisplayOptions) -> Result<Screen, String> {
        let window = if opts.window {
            Some(crate::window::Window::open(&opts.title)?)
        } else {
            None
        };
        Ok(Screen {
            window,
            title: opts.title.clone(),
            last_draw: std::time::Instant::now(),
        })
    }

    fn draw(w: &crate::window::Window, m: &Machine) {
        w.draw(m.cpu.bus.frame());
    }

    /// Pumps the window, hands the keys typed into it to the machine's keyboard and what
    /// the mouse did to its pointer, and redraws if due. Returns false once the window
    /// has been closed.
    fn update(&mut self, m: &mut Machine, presented: bool) -> bool {
        let Some(w) = &mut self.window else {
            return true;
        };
        if !w.pump() {
            return false;
        }
        for key in w.take_keys() {
            m.key_event(key);
        }
        for input in w.take_pointer() {
            m.pointer_event(input);
        }
        if presented || w.take_repaint() || self.last_draw.elapsed() >= Self::REFRESH {
            Self::draw(w, m);
            self.last_draw = std::time::Instant::now();
        }
        true
    }

    /// Keeps showing the final frame until the window is closed.
    fn hold(&mut self, m: &Machine, result: &RunResult) {
        let Some(w) = &mut self.window else {
            return;
        };
        let status = match result {
            RunResult::Exited(code) => format!("exited with code {code}"),
            RunResult::StepLimit => "step limit reached".to_string(),
        };
        w.set_title(&format!("{} ({status}, close to quit)", self.title));
        Self::draw(w, m);
        while w.pump() {
            // Nothing runs any more to read them.
            w.take_keys();
            w.take_pointer();
            if w.take_repaint() {
                Self::draw(w, m);
            }
            w.wait();
        }
    }
}

/// No window support: the framebuffer is only visible through PPM dumps.
#[cfg(not(windows))]
struct Screen;

#[cfg(not(windows))]
impl Screen {
    fn open(_opts: &DisplayOptions) -> Result<Screen, String> {
        Ok(Screen)
    }

    fn update(&mut self, _m: &mut Machine, _presented: bool) -> bool {
        true
    }

    fn hold(&mut self, _m: &Machine, _result: &RunResult) {}
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::win32::*;
    use std::ptr::null;
    use std::time::{Duration, Instant};

    /// Waits up to `limit` for a top-level window titled `title`.
    fn find_window(title: &str, limit: Duration) -> Option<usize> {
        let title = wide(title);
        let start = Instant::now();
        loop {
            // SAFETY: a NUL-terminated title; no class filter.
            let h = unsafe { FindWindowW(null(), title.as_ptr()) };
            if !h.is_null() {
                return Some(h as usize);
            }
            if start.elapsed() > limit {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Keys typed into the window reach the running program: `run` opens the window,
    /// another thread types "hi", Enter and Esc into it, and `kbd-echo.elf` echoes them
    /// and halts. Needs a desktop session:
    /// `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_keys_reach_the_program() {
        let elf = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/kbd-echo/kbd-echo.elf"
        );
        let mut m = Machine::with_ram(crate::DEFAULT_RAM_SIZE);
        m.load_elf(&std::fs::read(elf).unwrap()).unwrap();
        m.cpu.bus.uart.capture = true;
        let title = format!("Grail display key test {}", std::process::id());
        let held = format!("{title} (exited with code 0, close to quit)");

        let typist = std::thread::spawn({
            let title = title.clone();
            move || {
                let hwnd = find_window(&title, Duration::from_secs(10)).expect("window") as HWND;
                for (vk, scan) in [
                    (b'H' as u32, 0x23),
                    (b'I' as u32, 0x17),
                    (0x0d, 0x1c),
                    (0x1b, 0x01),
                ] {
                    for (msg, up) in [(WM_KEYDOWN, 0), (WM_KEYUP, 0xc000_0000u32)] {
                        let lparam = (1 | scan << 16 | up) as i32 as LPARAM;
                        // SAFETY: posting a key message carries no pointers.
                        unsafe { PostMessageW(hwnd, msg, vk as WPARAM, lparam) };
                    }
                }
                // The program halts on Esc and the window stays open on its last frame,
                // with the result in its title, until it is closed.
                let halted = find_window(&held, Duration::from_secs(10)).is_some();
                // SAFETY: WM_CLOSE carries no pointers.
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                halted
            }
        });

        // Typed keys carry this thread's Shift and Ctrl state, so start with none held.
        // SAFETY: a 256-byte key state table.
        unsafe { SetKeyboardState([0u8; 256].as_ptr()) };
        let opts = DisplayOptions {
            window: true,
            dump_dir: None,
            title,
        };
        let outcome = run(&mut m, None, |_| {}, &opts).unwrap();
        assert!(typist.join().unwrap(), "the program did not halt");
        assert_eq!(outcome, Outcome::Program(RunResult::Exited(0)));
        assert_eq!(
            String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap(),
            "hi\n[dbg] pc=8000005c s1 = 0x00000004 (4)\n"
        );
    }

    /// The mouse over the window reaches the running program: another thread moves it to
    /// (100, 50) and presses the left button there, then presses Esc, and
    /// `examples/pointer` leaves its cursor there, red. Needs a desktop session, with
    /// the real mouse kept off the window: `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_mouse_reaches_the_program() {
        let elf = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/pointer/pointer.elf");
        let mut m = Machine::with_ram(crate::DEFAULT_RAM_SIZE);
        m.load_elf(&std::fs::read(elf).unwrap()).unwrap();
        m.cpu.bus.uart.capture = true;
        let title = format!("Grail display mouse test {}", std::process::id());
        let held = format!("{title} (exited with code 0, close to quit)");

        let user = std::thread::spawn({
            let title = title.clone();
            move || {
                let hwnd = find_window(&title, Duration::from_secs(10)).expect("window") as HWND;
                let at = (100 | 50 << 16) as LPARAM; // the window is 640x480: 1:1
                for (msg, wparam, lparam) in [
                    (WM_MOUSEMOVE, 0, at),
                    (WM_LBUTTONDOWN, MK_LBUTTON, at),
                    (WM_KEYDOWN, 0x1b, (1 | 0x01 << 16) as LPARAM),
                ] {
                    // SAFETY: posting mouse and key messages carries no pointers.
                    unsafe { PostMessageW(hwnd, msg, wparam, lparam) };
                }
                let halted = find_window(&held, Duration::from_secs(10)).is_some();
                // SAFETY: WM_CLOSE carries no pointers.
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                halted
            }
        });

        // SAFETY: a 256-byte key state table.
        unsafe { SetKeyboardState([0u8; 256].as_ptr()) };
        let opts = DisplayOptions {
            window: true,
            dump_dir: None,
            title,
        };
        let outcome = run(&mut m, None, |_| {}, &opts).unwrap();
        assert!(user.join().unwrap(), "the program did not halt");
        assert_eq!(outcome, Outcome::Program(RunResult::Exited(0)));
        let fb = &m.cpu.bus.fb;
        assert_eq!(fb.pixel(100, 50), 0xff0000, "the cursor's tip, red");
        assert_eq!(fb.pixel(0, 0), 0x203048, "background where it started");
        assert_eq!(m.cpu.bus.ptr.held(), crate::pointer::BUTTON_LEFT);
    }

    /// Closing the window while the program still runs stops it: `run` returns
    /// `Outcome::WindowClosed`. The program is an endless loop, so nothing else
    /// would. Needs a desktop session: `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn closing_the_window_stops_the_emulator() {
        let mut m = Machine::with_ram(1 << 20);
        m.cpu
            .bus
            .load(crate::bus::RAM_BASE, &0x0000_006fu32.to_le_bytes())
            .unwrap(); // j .
        m.cpu.pc = crate::bus::RAM_BASE;
        let title = format!("Grail display close test {}", std::process::id());
        let user = std::thread::spawn({
            let title = title.clone();
            move || {
                let hwnd = find_window(&title, Duration::from_secs(10)).expect("window") as HWND;
                std::thread::sleep(Duration::from_millis(200));
                // SAFETY: WM_CLOSE carries no pointers.
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            }
        });
        let opts = DisplayOptions {
            window: true,
            dump_dir: None,
            title,
        };
        let outcome = run(&mut m, None, |_| {}, &opts).unwrap();
        user.join().unwrap();
        assert_eq!(outcome, Outcome::WindowClosed);
        assert_eq!(m.exit_code(), None);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    /// A step limit bigger than a slice is still exact: `run` stops after exactly
    /// that many steps, not at the next slice boundary.
    #[test]
    fn the_step_limit_is_exact() {
        for limit in [1, SLICE - 1, SLICE, 25_000, 3 * SLICE + 7] {
            let mut m = Machine::with_ram(1 << 20);
            m.cpu
                .bus
                .load(crate::bus::RAM_BASE, &0x0000_006fu32.to_le_bytes())
                .unwrap(); // j .
            m.cpu.pc = crate::bus::RAM_BASE;
            let mut steps = 0u64;
            let outcome = run(&mut m, Some(limit), |_| steps += 1, &DisplayOptions::default()).unwrap();
            assert_eq!(outcome, Outcome::Program(RunResult::StepLimit), "{limit}");
            assert_eq!(steps, limit);
        }
    }
}
