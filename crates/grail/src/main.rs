//! `grail [options] <program.elf>`

use grail::display::{self, DisplayOptions, Outcome};
use grail::{Machine, RunResult, DEFAULT_RAM_SIZE};
use std::process::ExitCode;

const USAGE: &str = "usage: grail [options] <program.elf>

  --trace         print every instruction before it runs
  --max-steps N   stop after N steps (exit code 124)
  --ram MiB       RAM size in MiB (default 128)
  --fb            show the framebuffer in a window (Windows only)
  --headless      no window, even with --fb (the default)
  --fb-dump DIR   write DIR/frame-NNNN.ppm on every FB_PRESENT and at exit";

fn main() -> ExitCode {
    let mut trace = false;
    let mut max_steps = None;
    let mut ram = DEFAULT_RAM_SIZE;
    let mut path = None;
    let mut fb = false;
    let mut headless = false;
    let mut dump_dir = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut num = |name: &str| -> Option<u64> {
            let v = args.next().and_then(|s| s.parse().ok());
            if v.is_none() {
                eprintln!("grail: {name} needs a number");
            }
            v
        };
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
            "--max-steps" => match num("--max-steps") {
                Some(n) => max_steps = Some(n),
                None => return ExitCode::from(2),
            },
            "--ram" => match num("--ram") {
                Some(n) => ram = n as usize * 1024 * 1024,
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
    let mut m = Machine::new(ram);
    if let Err(e) = m.load_elf(&file) {
        eprintln!("grail: {path}: {e}");
        return ExitCode::from(2);
    }
    m.cpu.bus.uart.input = Some(spawn_stdin_reader());

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

/// Feeds host stdin to the UART from a background thread.
fn spawn_stdin_reader() -> std::sync::mpsc::Receiver<u8> {
    use std::io::Read;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut stdin = std::io::stdin();
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                if tx.send(b).is_err() {
                    return;
                }
            }
        }
    });
    rx
}
