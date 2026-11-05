//! merlin-dbg, the Camelot debugger: machine-level debugging of programs running in
//! Grail, over the GDB remote serial protocol (`docs/merlin-dbg.md`).
//!
//! - [`remote`]: the protocol client, talking to Grail's stub (`grail --gdb`);
//! - [`symbols`]: names from the program's ELF symbol table;
//! - [`source`]: source lines, from the program's DWARF line tables;
//! - [`vars`]: variables and types, from the program's DWARF debugging information;
//! - [`frames`]: backtraces by frame pointer (the frame layout is explained there);
//! - [`session`]: the commands;
//! - [`run`]: the command line, which connects to a stub (`grail --gdb`'s, or a VM's on
//!   an Avalon server) or starts a machine in this process.

pub mod camelot;
pub mod frames;
pub mod paging;
pub mod remote;
pub mod session;
pub mod source;
pub mod symbols;
pub mod vars;

use remote::Remote;
use session::{Flow, OnQuit, Session};
use source::Lines;
use std::io::{BufRead, IsTerminal, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::AtomicBool;
use symbols::Symbols;

pub const USAGE: &str = "usage: merlin-dbg [--connect ADDR] [-x FILE] [program.elf]
       merlin-dbg --vm VM [--server ADDR] [-x FILE] [program.elf]
       merlin-dbg [-x FILE] -- [grail options] program.elf

Debugs a program running in Grail, at the machine level: breakpoints,
watchpoints, steps, registers, memory, disassembly and backtraces; and by
source line and variable where the program has debug information (merlin
-g): break FILE:LINE, step and next by line, list, print, info locals, info
args, ptype, frames with their arguments, up and down. Sources are found
relative to the current directory (directory DIR adds others).

  --connect ADDR  the GDB stub of a running grail --gdb (default 127.0.0.1:1234):
                  HOST:PORT or a port on 127.0.0.1. program.elf gives the symbols.
                  quit leaves the machine running
  --vm VM         the GDB stub of a VM on an Avalon server (its name or id), which
                  stops for the debugger; quit leaves it running
  --server ADDR   with --vm, the Avalon server: host:port, or a host or IP address
                  for port 7400 (default 127.0.0.1:7400)
  -- ARGS         start Grail in this process with ARGS, its options and program
                  (as for the grail command), stopped at its first instruction.
                  The program's console output comes here; it gets no input.
                  quit ends the machine
  -x FILE         run the commands in FILE first, then those from stdin

Commands are read from stdin, one per line (help lists them). Ctrl-C stops a
running machine.";

/// The default stub address, `grail --gdb-wait`'s.
const DEFAULT_ADDR: &str = "127.0.0.1:1234";

/// Avalon's default port, for a `--server` without one.
const AVALON_PORT: u16 = 7400;

/// The name merlin-dbg gives an Avalon server.
const AGENT: &str = "merlin-dbg";

/// Where the stub is.
enum Stub {
    /// `grail --gdb`'s, at this address.
    Tcp(SocketAddr),
    /// The stub of this VM on the Avalon server at this address.
    Vm(String, avalon_proto::VmRef),
}

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
    let mut vm = None;
    let mut server = None;
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
            "--vm" => {
                let v = it.next().ok_or_else(|| Some("--vm needs a VM".to_string()))?;
                vm = Some(avalon_proto::VmRef::parse(v));
            }
            "--server" => {
                let v = it
                    .next()
                    .ok_or_else(|| Some("--server needs an address".to_string()))?;
                server =
                    Some(avalon_client::parse_addr(v, AVALON_PORT).map_err(|e| format!("--server: {e}"))?);
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

    if server.is_some() && vm.is_none() {
        return Err(Some(format!("--server goes with --vm\n{USAGE}")));
    }
    if vm.is_some() && (connect.is_some() || grail_args.is_some()) {
        return Err(Some(format!(
            "--vm, --connect and -- are three ways to reach a machine: one at a time\n{USAGE}"
        )));
    }
    // Where the stub is, and the machine's thread when it is ours.
    let (stub, machine, on_quit) = match grail_args {
        Some(_) if connect.is_some() || program.is_some() => {
            return Err(Some(format!(
                "--connect and a program go before --, not with it\n{USAGE}"
            )))
        }
        Some(args) => {
            let (addr, thread, prog) = start_grail(args)?;
            program = prog;
            (Stub::Tcp(addr), Some(thread), OnQuit::Kill)
        }
        None => match vm {
            Some(vm) => {
                let server = server.unwrap_or_else(|| avalon_proto::DEFAULT_AVALON_ADDR.to_string());
                (Stub::Vm(server, vm), None, OnQuit::Detach)
            }
            None => (
                Stub::Tcp(
                    connect.unwrap_or_else(|| DEFAULT_ADDR.parse().expect("the default address parses")),
                ),
                None,
                OnQuit::Detach,
            ),
        },
    };
    let mut lines = Lines::default();
    let mut debug = vars::Debug::default();
    let (symbols, file) = match &program {
        Some(path) => {
            let file = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            lines.add_elf(&file).map_err(|e| format!("{path}: {e}"))?;
            debug.add_elf(&file).map_err(|e| format!("{path}: {e}"))?;
            let symbols = Symbols::from_elf(&file).map_err(|e| format!("{path}: {e}"))?;
            (symbols, Some((path, file)))
        }
        None => (Symbols::default(), None),
    };
    let remote = match stub {
        Stub::Tcp(addr) => Remote::connect(addr)?,
        Stub::Vm(server, vm) => {
            let client = avalon_client::Client::connect(&server, AGENT).map_err(|e| e.to_string())?;
            let tunnel =
                avalon_client::DebugTunnel::open(client, &vm).map_err(|e| format!("VM {vm}: {e}"))?;
            Remote::over(Box::new(tunnel))?
        }
    };
    // The machine's width, from the stub, must be the program's: an ELF32 program's
    // symbols would be wrong for an RV64 machine. A program of the other width is
    // left out with a warning and the session goes on without its symbols, as GDB
    // warns and goes on: ending it would drop the connection, which the stub takes
    // as a detach, and a machine waiting for its debugger would run off on its own.
    // (Started with `--`, the machine is the program's width.)
    let (symbols, lines, debug, file) = match file {
        Some((path, file)) => match symbols::check_xlen(&file, remote.xlen) {
            Ok(()) => (symbols, lines, debug, Some(file)),
            Err(e) => {
                let _ = writeln!(out, "warning: {path}: {e}: going on without its symbols");
                (Symbols::default(), Lines::default(), vars::Debug::default(), None)
            }
        },
        None => (symbols, lines, debug, None),
    };
    let mut session = Session::new(remote, symbols, lines, debug, &mut *out, on_quit, interrupt);
    if let Some(file) = file {
        session.set_program(file);
    }
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
