//! Host side of the framebuffer: runs the machine in slices and, between slices,
//! shows the frame in a window (Windows only) and/or dumps it as PPM files.
//!
//! Everything happens on one thread. The CPU runs until the program writes
//! `FB_PRESENT` or a slice of `SLICE` steps is done; then the window's messages are
//! pumped and it is redrawn on a present, on a repaint request, or when ~16 ms of
//! host time have passed since the last redraw (~60 Hz). Keys typed into the window
//! reach the keyboard device at the same points, between slices.

use crate::cpu::Cpu;
use crate::framebuffer::FrameDumper;
use crate::{Machine, RunResult};
use std::path::PathBuf;

/// Steps between two looks at the host side (window messages, redraw timer).
const SLICE: u64 = 20_000;

#[derive(Default)]
pub struct DisplayOptions {
    /// Open a window showing the framebuffer. Ignored where `WINDOW_SUPPORTED` is false.
    pub window: bool,
    /// Write `frame-NNNN.ppm` here on every `FB_PRESENT` and once at exit.
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
            d.dump(&m.cpu.bus.fb)
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
        steps += m.run_slice(budget, &mut trace);
        let presented = m.cpu.bus.fb.take_present();
        if presented {
            dump(m)?;
        }
        if !screen.update(m, presented) {
            break Outcome::WindowClosed;
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
        let fb = &m.cpu.bus.fb;
        w.draw(fb.enabled().then_some(fb.vram.as_slice()));
    }

    /// Pumps the window, hands the keys typed into it to the machine's keyboard, and
    /// redraws if due. Returns false once the window has been closed.
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
            w.take_keys(); // nothing runs any more to read them
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
