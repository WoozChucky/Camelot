//! `grail [options] <program.elf>`

use grail::console;
use grail::display::{self, DisplayOptions, Outcome};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::process::ExitCode;

const USAGE: &str = "usage: grail [options] <program.elf>

  --trace         print every instruction before it runs
  --max-steps N   stop after N steps (exit code 124)
  --ram MiB       RAM size in MiB, 1 to 2048 (default 128)
  --clock FREQ    real time: run FREQ instructions per second, and mtime ticks at FREQ.
                  FREQ is a number with an optional unit, Hz, kHz or MHz (any case;
                  no unit = Hz); fractions are fine if the result is whole Hz:
                  --clock 10MHz, --clock 500kHz, --clock 1.5MHz, --clock 5Hz.
                  Without it Grail runs at full speed, deterministically, and reports
                  a nominal 10 MHz clock to software
  --disk IMG      attach the image file IMG as the next disk (spec section 9.6),
                  read-write: the program's writes change the file. IMG must be a
                  whole number of 512-byte sectors. Repeat for more disks (up to 8)
  --disk-ro IMG   the same, read-only
  --fb            show the framebuffer in a window (Windows only)
  --headless      no window, even with --fb (the default)
  --fb-dump DIR   write DIR/frame-NNNN.ppm on every FB_PRESENT and at exit";

fn main() -> ExitCode {
    let mut trace = false;
    let mut max_steps = None;
    let mut config = MachineConfig::default();
    let mut path = None;
    let mut fb = false;
    let mut headless = false;
    let mut dump_dir = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--trace" => trace = true,
            "--fb" => fb = true,
            "--headless" => headless = true,
            "--fb-dump" => match args.next() {
                Some(dir) => dump_dir = Some(dir.into()),
                None => {
                    eprintln!("grail: --fb-dump needs a directory");
                    return ExitCode::from(2);
                }
            },
            "--max-steps" => {
                let parse = |s: &str| s.parse().map_err(|_| format!("'{s}' is not a number"));
                match value(&mut args, "--max-steps", parse) {
                    Some(n) => max_steps = Some(n),
                    None => return ExitCode::from(2),
                }
            }
            "--ram" => match value(&mut args, "--ram", grail::parse_ram_mib) {
                Some(size) => config.ram_size = size,
                None => return ExitCode::from(2),
            },
            "--clock" => match value(&mut args, "--clock", grail::parse_clock) {
                Some(hz) => config.clock_hz = Some(hz),
                None => return ExitCode::from(2),
            },
            "--disk" | "--disk-ro" => match value(&mut args, &a, |s| Ok(DiskConfig::file(s))) {
                Some(disk) => config.disks.push(disk.with_read_only(a == "--disk-ro")),
                None => return ExitCode::from(2),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if a.starts_with('-') => {
                eprintln!("grail: unknown option {a}\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => path = Some(a),
        }
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let file = match std::fs::read(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("grail: cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut m = match Machine::new(config) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("grail: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = m.load_elf(&file) {
        eprintln!("grail: {path}: {e}");
        return ExitCode::from(2);
    }
    // The machine's console is this process's terminal. An interactive one is switched
    // to raw mode (each key goes to the UART as typed, unechoed) until Grail exits:
    // before the reader thread starts, so its first read is already a raw one.
    let raw = console::RawMode::enable();
    m.set_uart_input(match &raw {
        Some(raw) => raw.stdin_channel(),
        None => console::stdin_channel(),
    });
    m.set_uart_output(std::io::stdout());
    m.set_debug_output(std::io::stderr());

    if fb && !headless && !display::WINDOW_SUPPORTED {
        eprintln!("grail: no framebuffer window on this platform, running headless");
    }
    let name = std::path::Path::new(&path)
        .file_name()
        .map_or(path.clone(), |n| n.to_string_lossy().into());
    let opts = DisplayOptions {
        window: fb && !headless && display::WINDOW_SUPPORTED,
        dump_dir,
        title: format!("Grail - {name}"),
    };
    let result = display::run(
        &mut m,
        max_steps,
        |cpu| {
            if trace {
                let word = cpu.bus.peek(cpu.pc);
                let text = word.map_or("<unmapped>".to_string(), |w| excalibur::decode(w).to_string());
                eprintln!("{:?} {:08x}: {}", cpu.mode, cpu.pc, text);
            }
        },
        &opts,
    );
    match result {
        Ok(Outcome::Program(RunResult::Exited(code))) => ExitCode::from(code.min(255) as u8),
        Ok(Outcome::Program(RunResult::StepLimit)) => {
            eprintln!("grail: step limit reached at pc={:#010x}", m.cpu.pc);
            ExitCode::from(124)
        }
        // Closing the window is a normal way to stop the machine.
        Ok(Outcome::WindowClosed) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("grail: {e}");
            ExitCode::from(2)
        }
    }
}

/// The value of option `name`, parsed with `parse`. Prints why it is missing or wrong.
fn value<T>(
    args: &mut impl Iterator<Item = String>,
    name: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Option<T> {
    let Some(v) = args.next() else {
        eprintln!("grail: {name} needs a value\n{USAGE}");
        return None;
    };
    parse(&v).map_err(|e| eprintln!("grail: {name}: {e}")).ok()
}
