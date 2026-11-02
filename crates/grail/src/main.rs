//! `grail [options] [program.elf]`: runs the program, or with none boots from disk 0.

use grail::cli;
use grail::console;
use grail::display::{self, DisplayOptions, Outcome};
use grail::{Machine, RunResult};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut o = match cli::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(cli::Error::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(cli::Error::Usage(msg, with_usage)) => {
            if with_usage {
                eprintln!("grail: {msg}\n{}", cli::USAGE);
            } else {
                eprintln!("grail: {msg}");
            }
            return ExitCode::from(2);
        }
    };
    // What the window's title calls the machine: the program, or the disk it boots from.
    let shown = o.shown();
    let mut m = match o.machine() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("grail: {e}");
            return ExitCode::from(2);
        }
    };
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

    let several_harts = m.config().harts > 1;
    let tracing = o.trace;
    let mut trace = |cpu: &grail::cpu::Cpu| {
        if tracing {
            let word = cpu.bus.peek(cpu.pc);
            let text = word.map_or("<unmapped>".to_string(), |w| excalibur::decode(w).to_string());
            if several_harts {
                eprintln!("h{} {:?} {:08x}: {}", cpu.id, cpu.mode, cpu.pc, text);
            } else {
                eprintln!("{:?} {:08x}: {}", cpu.mode, cpu.pc, text);
            }
        }
    };
    if let Some(addr) = o.gdb {
        return run_gdb(&mut m, addr, o.gdb_wait, &mut trace);
    }

    let (fb, headless, max_steps) = (o.fb, o.headless, o.max_steps);
    if fb && !headless && !display::WINDOW_SUPPORTED {
        eprintln!("grail: no framebuffer window on this platform, running headless");
    }
    let name = std::path::Path::new(&shown)
        .file_name()
        .map_or(shown.clone(), |n| n.to_string_lossy().into());
    let opts = DisplayOptions {
        window: fb && !headless && display::WINDOW_SUPPORTED,
        dump_dir: o.dump_dir.take(),
        title: format!("Grail - {name}"),
    };
    let result = display::run(&mut m, max_steps, trace, &opts);
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

/// Runs the machine under the GDB stub. Says where it listens on stderr, which is how a
/// script (or a test) given port 0 learns the port.
fn run_gdb(
    m: &mut Machine,
    addr: std::net::SocketAddr,
    wait: bool,
    trace: &mut impl FnMut(&grail::cpu::Cpu),
) -> ExitCode {
    let listener = match std::net::TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("grail: --gdb: cannot listen on {addr}: {e}");
            return ExitCode::from(2);
        }
    };
    let local = listener.local_addr().unwrap_or(addr);
    let waiting = if wait { ", waiting for a client" } else { "" };
    eprintln!("grail: gdb stub listening on {local}{waiting}");
    match grail::gdb::serve(m, listener, wait, trace) {
        Ok(grail::gdb::Outcome::Exited(code)) => ExitCode::from(code.min(255) as u8),
        Ok(grail::gdb::Outcome::Killed) => {
            eprintln!("grail: killed by the debugger");
            ExitCode::from(137)
        }
        Err(e) => {
            eprintln!("grail: gdb stub: {e}");
            ExitCode::from(2)
        }
    }
}
