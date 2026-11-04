//! merlin-dbg, the Camelot debugger: machine-level debugging of programs running in
//! Grail, over the GDB remote serial protocol (`docs/merlin-dbg.md`).
//!
//! - [`remote`]: the protocol client, talking to Grail's stub (`grail --gdb`);
//! - [`symbols`]: names from the program's ELF symbol table;
//! - [`source`]: source lines, from the program's DWARF line tables;
//! - [`frames`]: backtraces by frame pointer (the frame layout is explained there);
//! - [`session`]: the commands;
//! - [`run`]: the command line, which connects to a stub or starts a machine in this
//!   process.

pub mod frames;
pub mod remote;
pub mod session;
pub mod source;
pub mod symbols;

use remote::Remote;
use session::{Flow, OnQuit, Session};
use source::Lines;
use std::io::{BufRead, IsTerminal, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::AtomicBool;
use symbols::Symbols;

pub const USAGE: &str = "usage: merlin-dbg [--connect ADDR] [-x FILE] [program.elf]
       merlin-dbg [-x FILE] -- [grail options] program.elf

Debugs a program running in Grail, at the machine level: breakpoints,
watchpoints, steps, registers, memory, disassembly and backtraces; and by
source line where the program has line tables (merlin -g): break FILE:LINE,
step and next by line, list. Sources are found relative to the current
directory (directory DIR adds others).

  --connect ADDR  the GDB stub of a running grail --gdb (default 127.0.0.1:1234):
                  HOST:PORT or a port on 127.0.0.1. program.elf gives the symbols.
                  quit leaves the machine running
  -- ARGS         start Grail in this process with ARGS, its options and program
                  (as for the grail command), stopped at its first instruction.
                  The program's console output comes here; it gets no input.
                  quit ends the machine
  -x FILE         run the commands in FILE first, then those from stdin

Commands are read from stdin, one per line (help lists them). Ctrl-C stops a
running machine.";

/// The default stub address, `grail --gdb-wait`'s.
const DEFAULT_ADDR: &str = "127.0.0.1:1234";

/// Runs merlin-dbg with `args` (without the program name), reading commands from
/// `input` and writing to `out`. `interrupt` is set on Ctrl-C. Returns the exit code.
pub fn run(args: &[String], input: &mut dyn BufRead, out: &mut dyn Write, interrupt: &AtomicBool) -> i32 {
    match start(args, input, out, interrupt) {
        Ok(()) => 0,
        Err(Some(e)) => {
            let _ = writeln!(out, "merlin-dbg: {e}");
            2
        }
        Err(None) => {
            let _ = writeln!(out, "{USAGE}");
            0
        }
    }
}

/// `Err(None)` asks for the usage text.
fn start(
    args: &[String],
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    interrupt: &AtomicBool,
) -> Result<(), Option<String>> {
    let mut connect = None;
    let mut script = None;
    let mut program = None;
    let mut grail_args = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Err(None),
            "--connect" => {
                let v = it
                    .next()
                    .ok_or_else(|| Some("--connect needs an address".to_string()))?;
                connect = Some(parse_addr(v).map_err(|e| format!("--connect: {e}"))?);
            }
            "-x" => {
                script = Some(
                    it.next()
                        .ok_or_else(|| Some("-x needs a file".to_string()))?
                        .clone(),
                )
            }
            "--" => {
                grail_args = Some(it.by_ref().cloned().collect::<Vec<_>>());
            }
            _ if a.starts_with('-') => return Err(Some(format!("unknown option {a}\n{USAGE}"))),
            _ if program.is_none() => program = Some(a.clone()),
            _ => return Err(Some(format!("one program at a time\n{USAGE}"))),
        }
    }
    let script = match &script {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?,
        None => String::new(),
    };

    // Where the stub is, and the machine's thread when it is ours.
    let (addr, machine, on_quit) = match grail_args {
        Some(_) if connect.is_some() || program.is_some() => {
            return Err(Some(format!(
                "--connect and a program go before --, not with it\n{USAGE}"
            )))
        }
        Some(args) => {
            let (addr, thread, prog) = start_grail(args)?;
            program = prog;
            (addr, Some(thread), OnQuit::Kill)
        }
        None => (
            connect.unwrap_or_else(|| DEFAULT_ADDR.parse().expect("the default address parses")),
            None,
            OnQuit::Detach,
        ),
    };
    let mut lines = Lines::default();
    let (symbols, file) = match &program {
        Some(path) => {
            let file = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            lines.add_elf(&file).map_err(|e| format!("{path}: {e}"))?;
            let symbols = Symbols::from_elf(&file).map_err(|e| format!("{path}: {e}"))?;
            (symbols, Some((path, file)))
        }
        None => (Symbols::default(), None),
    };
    let remote = Remote::connect(addr)?;
    // The machine's width, from the stub, is the program's (an ELF32 program on an
    // RV64 machine would have the wrong symbols, and no RV32 code runs there).
    if let Some((path, file)) = &file {
        symbols::check_xlen(file, remote.xlen).map_err(|e| format!("{path}: {e}"))?;
    }
    let mut session = Session::new(remote, symbols, lines, &mut *out, on_quit, interrupt);
    session.start()?;

    // The script, then stdin. Commands that do not come from a terminal are echoed, so
    // a transcript reads like a session.
    let interactive = std::io::stdin().is_terminal();
    let mut quit = false;
    for line in script.lines() {
        session_echo(&mut session, line);
        if session.command(line) == Flow::Quit {
            quit = true;
            break;
        }
    }
    while !quit {
        if interactive {
            session_prompt(&mut session);
        }
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if !interactive {
            session_echo(&mut session, &line);
        }
        quit = session.command(&line) == Flow::Quit;
    }
    session.quit();
    drop(session);
    if let Some(thread) = machine {
        match thread.join() {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(Some(format!("grail: {e}"))),
            Err(_) => return Err(Some("the machine's thread panicked".into())),
        }
    }
    Ok(())
}

fn session_echo<W: Write>(session: &mut Session<'_, W>, line: &str) {
    let line = line.trim_end();
    if !line.trim().is_empty() {
        session.print(&format!(
            "(merlin-dbg) {line}
"
        ));
    }
}

fn session_prompt<W: Write>(session: &mut Session<'_, W>) {
    session.print("(merlin-dbg) ");
}

/// `--connect`'s address: `host:port` or a port on 127.0.0.1.
fn parse_addr(s: &str) -> Result<SocketAddr, String> {
    if let Ok(port) = s.parse::<u16>() {
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    use std::net::ToSocketAddrs;
    s.to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| format!("'{s}' is not an address (HOST:PORT)"))
}

/// The machine's thread, which ends with the stub's outcome.
type MachineThread = std::thread::JoinHandle<std::io::Result<grail::gdb::Outcome>>;

/// Starts Grail in this process: the machine `args` describe, under a stub on a free
/// loopback port, stopped until merlin-dbg resumes it. Returns the stub's address, the
/// machine's thread and the program (for its symbols).
fn start_grail(args: Vec<String>) -> Result<(SocketAddr, MachineThread, Option<String>), String> {
    let mut o = match grail::cli::parse(args) {
        Ok(o) => o,
        Err(grail::cli::Error::Help) => return Err(grail::cli::USAGE.into()),
        Err(grail::cli::Error::Usage(msg, _)) => return Err(format!("grail: {msg}")),
    };
    if o.gdb.is_some() {
        return Err("grail: no --gdb after --: merlin-dbg starts the stub itself".into());
    }
    if o.max_steps.is_some() || o.fb || o.dump_dir.is_some() {
        return Err("grail: --max-steps, --fb and --fb-dump do not work under the debugger".into());
    }
    let mut m = o.machine().map_err(|e| format!("grail: {e}"))?;
    m.set_uart_output(std::io::stdout());
    m.set_debug_output(std::io::stderr());
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("cannot listen: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let tracing = o.trace;
    let thread = std::thread::spawn(move || {
        let mut trace = |cpu: &grail::cpu::Cpu| {
            if tracing {
                let word = cpu.bus.peek(cpu.pc);
                let text = word.map_or("<unmapped>".to_string(), |w| {
                    excalibur::decode(w, cpu.xlen()).to_string()
                });
                let digits = cpu.xlen().bits() as usize / 4;
                eprintln!("h{} {:?} {:0digits$x}: {}", cpu.id, cpu.mode, cpu.pc, text);
            }
        };
        grail::gdb::serve(&mut m, listener, true, &mut trace)
    });
    Ok((addr, thread, o.program.clone()))
}
