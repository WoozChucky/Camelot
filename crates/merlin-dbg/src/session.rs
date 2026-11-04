//! merlin-dbg's commands: a debugging session over a [`Remote`], reading commands a
//! line at a time and writing what they show (`docs/merlin-dbg.md`, "Commands").
//!
//! With line tables ([`Lines`]) the session works by source line too: `break
//! FILE:LINE`, `step` and `next` by line, `list`, and every stop says the file,
//! line and its text. With the rest of the debugging information ([`Debug`]) it
//! knows the variables and types: `print`, `info locals`, `info args`, `ptype`,
//! `whatis`, the arguments of each frame in a backtrace, and `frame`, `up` and
//! `down` to choose the frame those look in.

use crate::frames::{self, Regs};
use crate::remote::{self, hex_value, wrap, Remote, StopEvent, Watch};
use crate::source::{Lines, Place};
use crate::symbols::{self, Symbols};
use crate::vars::{self, Debug, Eval};
use excalibur::{Inst, Syntax, Xlen};
use std::io::Write;
use std::sync::atomic::AtomicBool;

/// A breakpoint or a watchpoint, numbered from 1 as they are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    Break(u64),
    Watch(Watch, u64, u64),
}

/// Whether the session goes on after a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

/// What `quit` (or the end of the commands) does to the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnQuit {
    /// Stop it (`k`): merlin-dbg started it.
    Kill,
    /// Leave it running (`D`): it was there before merlin-dbg connected.
    Detach,
}

pub struct Session<'a, W: Write> {
    remote: Remote,
    /// The machine's width, which the stub gave: how its code decodes, how wide its
    /// registers and addresses are.
    xlen: Xlen,
    symbols: Symbols,
    lines: Lines,
    debug: Debug,
    /// The frame `print`, `info locals` and `info args` look in (0, the innermost,
    /// at every stop), which `frame`, `up` and `down` choose.
    frame: usize,
    /// How many values `print` has shown (`$1`, `$2`, ...).
    printed: usize,
    /// The source line shown last: an instruction step on the same line does
    /// not show it again.
    shown: Option<Place>,
    /// Where `list` goes on: a file and the next line to show.
    listed: Option<(String, u32)>,
    out: W,
    points: Vec<(usize, Point)>,
    next_point: usize,
    /// The program has exited: no machine to talk to any more.
    exited: bool,
    on_quit: OnQuit,
    interrupt: &'a AtomicBool,
}

type Result<T> = std::result::Result<T, String>;

/// A stop as [`Session::resume`] gives it: the hart, the watchpoint hit (its kind and
/// address), and the signal (5 for a trap, 2 for an interrupt).
type Halt = (usize, Option<(Watch, u64)>, u8);

const HELP: &str = "\
break|b FUNC            set a breakpoint on FUNC past its prologue (break *FUNC: at its start)
break|b ADDR            ... at ADDR (0x80000000, 1234, main+8, $ra)
break|b FILE:LINE       ... on a source line (kernel/main.c:42, or main.c:42)
watch [-r|-a] ADDR [N]  stop after a write (-r read, -a either) of N bytes at ADDR
delete|d [N]            delete breakpoint or watchpoint N, or all of them
info breakpoints|i b    list breakpoints and watchpoints
continue|c              run until something stops the machine (Ctrl-C to interrupt)
step|s [N]              run N source lines (1) of the selected hart, into calls
next|n [N]              the same, but run a call to its return
stepi|si [N]            run N instructions (1); step and next do too without lines
nexti|ni [N]            the same, but run a call to its return
finish                  run until the current function returns
list|l [FILE:]LINE|FUNC show ten source lines around LINE; list alone goes on
directory DIR           look for source files in DIR first
add-symbol-file FILE    add the symbols and lines of another program (a user one)
regs|info registers     the selected hart's registers, as GDB shows them
x/N[b|h|w|g] ADDR       N bytes (b), halfwords (h), words (w) or giants (g, 8) at ADDR
disas [ADDR] [N]        disassemble N instructions (8) from ADDR (the pc)
bt|backtrace            the call stack, by frame pointer, with arguments and lines
frame|f [N], up [N], down [N]  select frame N (or N frames out, or in)
print|p[/x] EXPR        an expression's value (variables, p->next, a[i], *p, &x)
info locals|args        the selected frame's local variables or arguments
ptype EXPR|TYPE         a type, with a struct's members; whatis: just its name
hart [N]                list the harts, or select hart N
quit|q                  leave (the machine is killed, or left running with --connect)";

impl<'a, W: Write> Session<'a, W> {
    pub fn new(
        remote: Remote,
        symbols: Symbols,
        lines: Lines,
        debug: Debug,
        out: W,
        on_quit: OnQuit,
        interrupt: &'a AtomicBool,
    ) -> Self {
        Session {
            xlen: remote.xlen,
            remote,
            symbols,
            lines,
            debug,
            frame: 0,
            printed: 0,
            shown: None,
            listed: None,
            out,
            points: Vec::new(),
            next_point: 1,
            exited: false,
            on_quit,
            interrupt,
        }
    }

    /// Says where the machine is stopped, as the session starts.
    pub fn start(&mut self) -> Result<()> {
        let harts = self.remote.harts;
        if harts > 1 {
            self.say(format!("connected: {harts} harts"));
        }
        self.show_pc()
    }

    /// Runs one command line. Errors are printed, and the session goes on.
    pub fn command(&mut self, line: &str) -> Flow {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Flow::Continue;
        }
        match self.run(line) {
            Ok(flow) => flow,
            Err(e) => {
                self.say(format!("error: {e}"));
                Flow::Continue
            }
        }
    }

    /// The end of the session: the machine is killed or left running.
    pub fn quit(&mut self) {
        if self.exited {
            return;
        }
        match self.on_quit {
            OnQuit::Kill => self.remote.kill(),
            OnQuit::Detach => {
                let _ = self.remote.detach();
            }
        }
        self.exited = true;
    }

    /// Writes `s` as it is (a prompt, an echoed command).
    pub fn print(&mut self, s: &str) {
        let _ = write!(self.out, "{s}");
        let _ = self.out.flush();
    }

    fn say(&mut self, s: impl AsRef<str>) {
        let _ = writeln!(self.out, "{}", s.as_ref());
        let _ = self.out.flush();
    }

    fn run(&mut self, line: &str) -> Result<Flow> {
        let (cmd, rest) = match line.find(char::is_whitespace) {
            Some(i) => (&line[..i], line[i..].trim()),
            None => (line, ""),
        };
        let args: Vec<&str> = rest.split_whitespace().collect();
        if matches!(cmd, "quit" | "q" | "exit") {
            self.quit();
            return Ok(Flow::Quit);
        }
        if matches!(cmd, "help" | "h" | "?") {
            self.say(HELP);
            return Ok(Flow::Continue);
        }
        if self.exited {
            return Err("the program is not running".into());
        }
        match cmd {
            "break" | "b" | "br" => self.break_(rest)?,
            "watch" => self.watch(&args)?,
            "delete" | "d" => self.delete(&args)?,
            "info" | "i" => match args.first().copied() {
                Some("breakpoints" | "b" | "break" | "watchpoints") => self.info_breakpoints(),
                Some("registers" | "r" | "reg") => self.regs()?,
                Some("harts" | "threads") => self.harts()?,
                Some("locals") => self.info_locals()?,
                Some("args") => self.info_args()?,
                _ => return Err("info breakpoints, registers, harts, locals or args".into()),
            },
            "continue" | "c" => self.continue_()?,
            "stepi" | "si" => {
                for _ in 0..count(&args)? {
                    if !self.step()? {
                        break;
                    }
                }
            }
            "nexti" | "ni" => {
                for _ in 0..count(&args)? {
                    if !self.next()? {
                        break;
                    }
                }
            }
            "step" | "s" | "next" | "n" => {
                let over = matches!(cmd, "next" | "n");
                for _ in 0..count(&args)? {
                    if !self.step_line(over)? {
                        break;
                    }
                }
            }
            "list" | "l" => self.list(rest)?,
            "directory" | "dir" => {
                if rest.is_empty() {
                    return Err("directory needs a directory".into());
                }
                self.lines.add_dir(rest.into());
                self.say(format!("source files are looked for in {rest} first"));
            }
            "add-symbol-file" => self.add_symbol_file(rest)?,
            "finish" => self.finish()?,
            "regs" | "registers" => self.regs()?,
            "disas" | "disassemble" => self.disas(&args)?,
            "bt" | "backtrace" | "where" => self.backtrace()?,
            "frame" | "f" => match args.first() {
                None => self.select_frame(self.frame)?,
                Some(n) => self.select_frame(parse_number(n).ok_or("frame needs a frame number")? as usize)?,
            },
            "up" => {
                let n = count(&args)? as usize;
                self.select_frame(self.frame + n)?
            }
            "down" => {
                let n = count(&args)? as usize;
                if self.frame == 0 {
                    return Err("bottom (innermost) frame selected; you cannot go down".into());
                }
                self.select_frame(self.frame.saturating_sub(n))?
            }
            "ptype" => self.ptype(rest, true)?,
            "whatis" => self.ptype(rest, false)?,
            _ if cmd == "print"
                || cmd == "p"
                || cmd == "inspect"
                || cmd.starts_with("print/")
                || cmd.starts_with("p/") =>
            {
                let format = cmd.split_once('/').map(|(_, f)| f);
                self.print_(rest, format)?
            }
            "hart" | "thread" => match args.first() {
                None => self.harts()?,
                Some(n) => self.select_hart(n)?,
            },
            _ if cmd.starts_with("x/") || cmd == "x" => self.examine(&cmd[1..], rest)?,
            _ => return Err(format!("unknown command '{cmd}' (try help)")),
        }
        Ok(Flow::Continue)
    }

    // ---- Expressions ----

    /// An address or a value: a number (`0x80000000`, `1234`), a symbol, a register
    /// (`$sp`, `$a0`, `$x5`, `$pc`), each optionally `+N` or `-N`. As wide as the
    /// machine's addresses: on RV32 the arithmetic wraps at 2^32, and a number
    /// must fit in 32 bits.
    fn eval(&mut self, expr: &str) -> Result<u64> {
        let expr = expr.trim();
        if let Some(i) = expr.rfind(['+', '-']).filter(|&i| i > 0) {
            let base = self.eval(&expr[..i])?;
            let off = parse_number(expr[i + 1..].trim()).ok_or_else(|| format!("bad offset in '{expr}'"))?;
            let v = if &expr[i..i + 1] == "+" {
                base.wrapping_add(off)
            } else {
                base.wrapping_sub(off)
            };
            return Ok(wrap(self.xlen, v));
        }
        if let Some(reg) = expr.strip_prefix('$') {
            let n = register_number(reg, self.xlen).ok_or_else(|| format!("no register ${reg}"))?;
            return self.remote.register(n);
        }
        if let Some(n) = parse_number(expr) {
            if wrap(self.xlen, n) != n {
                return Err(format!("{expr} is past the end of a 32-bit address space"));
            }
            return Ok(n);
        }
        self.symbols
            .lookup(expr)
            .ok_or_else(|| format!("no symbol '{expr}'"))
    }

    // ---- Breakpoints and watchpoints ----

    fn add_point(&mut self, p: Point) -> usize {
        let n = self.next_point;
        self.next_point += 1;
        self.points.push((n, p));
        n
    }

    fn break_(&mut self, expr: &str) -> Result<()> {
        if expr.is_empty() {
            return Err("break needs an address or a symbol".into());
        }
        let addrs = match source_line(expr) {
            Some((file, line)) => {
                let symbols = &self.symbols;
                let (addrs, place) = self
                    .lines
                    .breakpoints(file, line, |a| symbols.containing(a).map(|(_, start)| start))?;
                if place.line != line {
                    self.say(format!("line {line} has no code: {place} instead"));
                }
                // A line whose code is a function's prologue (its first line, the
                // one with its name) means the function past its prologue, as GDB
                // has it (skip_prologue_sal); a line further in stays where it is.
                addrs
                    .into_iter()
                    .map(|addr| match self.symbols.containing(addr) {
                        Some((_, start)) => addr.max(self.skip_prologue(start)),
                        None => addr,
                    })
                    .collect()
            }
            // `break *ADDR`: exactly there, as in GDB.
            None if expr.starts_with('*') => vec![self.eval(&expr[1..])?],
            // A function's name (a symbol in code) means the function past its
            // prologue, as GDB's `break FUNC`; anything else is an exact address.
            None => match self.symbols.code(expr) {
                Some(entry) => vec![self.skip_prologue(entry)],
                None => vec![self.eval(expr)?],
            },
        };
        for addr in addrs {
            if !self.has_breakpoint(addr) {
                self.remote.set_breakpoint(addr)?;
            }
            let n = self.add_point(Point::Break(addr));
            let at = self.where_(addr);
            self.say(format!("breakpoint {n} at {at}"));
        }
        Ok(())
    }

    /// Where `break` puts a breakpoint on the function at `entry`: past its
    /// prologue, as GDB finds it (`skip_prologue_sal`, then `riscv_skip_prologue`).
    /// From the line tables when the function has them ([`Lines::prologue_end`]:
    /// its `prologue_end` row, else its second row), else by GDB's prologue
    /// analysis of its code ([`frames::scan_prologue`]).
    fn skip_prologue(&mut self, entry: u64) -> u64 {
        let (start, end) = self
            .symbols
            .range(entry)
            .unwrap_or((entry, entry.wrapping_add(1)));
        match self.lines.prologue_end(start, end) {
            Some(body) => body.max(entry),
            None => frames::scan_prologue(&mut self.remote, entry, self.xlen),
        }
    }

    /// `0x80000024 <puts+4>`, and `: file:line` where the line tables say: without
    /// leading zeros, as GDB's `Breakpoint N at ADDR` has it.
    fn where_(&self, addr: u64) -> String {
        match self.lines.at(addr) {
            Some(place) => format!("{}: {place}", self.show_plain(addr)),
            None => self.show_plain(addr),
        }
    }

    /// `0x80000024 <puts+4>` with all of an address's digits (16 on RV64), as GDB
    /// shows a code address in a frame and in disassembly.
    fn show(&self, addr: u64) -> String {
        self.symbols.show(addr, self.xlen)
    }

    /// `0x80000024 <puts+4>` without leading zeros, as GDB shows an address
    /// everywhere else: `x`, `break`'s and `watch`'s messages, a code pointer's value.
    fn show_plain(&self, addr: u64) -> String {
        self.symbols.show_plain(addr)
    }

    /// A register's value in hex, with all its digits.
    fn hex(&self, v: u64) -> String {
        hex_value(v, self.xlen)
    }

    /// `add-symbol-file FILE`.
    fn add_symbol_file(&mut self, path: &str) -> Result<()> {
        if path.is_empty() {
            return Err("add-symbol-file needs a file".into());
        }
        let file = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        symbols::check_xlen(&file, self.xlen).map_err(|e| format!("{path}: {e}"))?;
        self.symbols.add_elf(&file).map_err(|e| format!("{path}: {e}"))?;
        self.lines.add_elf(&file).map_err(|e| format!("{path}: {e}"))?;
        self.debug.add_elf(&file).map_err(|e| format!("{path}: {e}"))?;
        self.say(format!("added the symbols of {path}"));
        Ok(())
    }

    fn has_breakpoint(&self, addr: u64) -> bool {
        self.points.iter().any(|(_, p)| *p == Point::Break(addr))
    }

    fn watch(&mut self, args: &[&str]) -> Result<()> {
        let (kind, args) = match args.first() {
            Some(&"-r") => (Watch::Read, &args[1..]),
            Some(&"-a") => (Watch::Access, &args[1..]),
            Some(&"-w") => (Watch::Write, &args[1..]),
            _ => (Watch::Write, args),
        };
        let (addr, len) = match args {
            [addr] => (self.eval(addr)?, self.watch_len(addr)),
            [addr, len] => (self.eval(addr)?, parse_number(len).ok_or("bad length")?),
            _ => return Err("watch [-r|-a] ADDR [LEN]".into()),
        };
        self.remote.set_watchpoint(kind, addr, len)?;
        let n = self.add_point(Point::Watch(kind, addr, len));
        let at = self.show_plain(addr);
        self.say(format!("watchpoint {n} ({}) at {at}, {len} bytes", kind.name()));
        Ok(())
    }

    /// How many bytes `watch ADDR` watches without a length. GDB watches an
    /// expression's value, as many bytes as its type has: here, when ADDR is a
    /// symbol with a size in the symbol table (a C variable), that size, at most 8
    /// (a register's worth on RV64, what `long` and pointers take); else a
    /// register's worth, 4 bytes on RV32 and 8 on RV64.
    fn watch_len(&self, expr: &str) -> u64 {
        let register = u64::from(self.xlen.bits() / 8);
        match self.symbols.size(expr.trim()) {
            Some(size) => size.min(8),
            None => register,
        }
    }

    fn delete(&mut self, args: &[&str]) -> Result<()> {
        let doomed: Vec<(usize, Point)> = match args {
            [] => self.points.clone(),
            [n] => {
                let n: usize = n
                    .parse()
                    .map_err(|_| format!("'{n}' is not a breakpoint number"))?;
                let p = self
                    .points
                    .iter()
                    .find(|(m, _)| *m == n)
                    .ok_or_else(|| format!("no breakpoint {n}"))?;
                vec![*p]
            }
            _ => return Err("delete [N]".into()),
        };
        for (n, p) in doomed {
            self.points.retain(|(m, _)| *m != n);
            match p {
                Point::Break(addr) if !self.has_breakpoint(addr) => self.remote.clear_breakpoint(addr)?,
                Point::Break(_) => {}
                Point::Watch(kind, addr, len) => self.remote.clear_watchpoint(kind, addr, len)?,
            }
        }
        Ok(())
    }

    fn info_breakpoints(&mut self) {
        if self.points.is_empty() {
            self.say("no breakpoints or watchpoints");
            return;
        }
        let mut lines = vec!["Num  Type        Where".to_string()];
        // A breakpoint's address with all its digits, as GDB's Address column has
        // it; a watchpoint's as it would be typed, GDB's What column (its expression).
        for (n, p) in &self.points {
            lines.push(match *p {
                Point::Break(addr) => format!("{n:<4} breakpoint  {}", self.show(addr)),
                Point::Watch(kind, addr, len) => format!(
                    "{n:<4} watchpoint  {}, {len} bytes, {}",
                    self.show_plain(addr),
                    kind.name()
                ),
            });
        }
        self.say(lines.join("\n"));
    }

    // ---- Running ----

    /// Resumes (a step of `step`'s hart, or every hart) and reports the stop. Returns
    /// the stop, or `None` once the program has exited.
    fn resume(&mut self, step: Option<usize>) -> Result<Option<Halt>> {
        // Every stop selects the innermost frame again.
        self.frame = 0;
        if step.is_none() {
            // After a run, the stop shows its source line whatever it is.
            self.shown = None;
        }
        match self.remote.resume(step, self.interrupt)? {
            StopEvent::Exited(code) => {
                self.exited = true;
                self.say(format!("program exited with code {code}"));
                Ok(None)
            }
            StopEvent::Stopped { signal, hart, watch } => Ok(Some((hart, watch, signal))),
        }
    }

    /// Reports why the machine stopped at the selected hart's pc.
    fn report(&mut self, hart: usize, watch: Option<(Watch, u64)>, signal: u8) -> Result<()> {
        let pc = self.remote.register(remote::PC)?;
        let who = self.hart_prefix(hart);
        if let Some((kind, addr)) = watch {
            let n = self.points.iter().find_map(|(n, p)| match *p {
                Point::Watch(k, a, l) if k == kind && addr.wrapping_sub(a) < l => Some(*n),
                _ => None,
            });
            let n = n.map_or(String::new(), |n| format!(" {n}"));
            let at = self.show_plain(addr);
            self.say(format!("{who}watchpoint{n} ({}) hit at {at}", kind.name()));
        } else if signal == 2 {
            self.say(format!("{who}interrupted"));
        } else if let Some(n) = self
            .points
            .iter()
            .find_map(|(n, p)| (*p == Point::Break(pc)).then_some(*n))
        {
            self.say(format!("{who}breakpoint {n}"));
        }
        self.show_pc()
    }

    fn hart_prefix(&self, hart: usize) -> String {
        if self.remote.harts > 1 {
            format!("[hart {hart}] ")
        } else {
            String::new()
        }
    }

    /// `=> pc <sym+off>: instruction`, then the source line the pc is on
    /// (`kernel/main.c:42:     text`), unless it is the one shown last.
    fn show_pc(&mut self) -> Result<()> {
        let pc = self.remote.register(remote::PC)?;
        let line = self.disas_line(pc, true);
        self.say(line);
        let place = self.lines.at(pc);
        if let Some(place) = place.clone().filter(|p| Some(p) != self.shown.as_ref()) {
            let line = match self.lines.text(&place.file, place.line) {
                Some(text) => format!("{place}: {text}"),
                None => place.to_string(),
            };
            self.say(line);
            // `list` then shows the lines around it.
            self.listed = Some((place.file.clone(), place.line.saturating_sub(5).max(1)));
        }
        self.shown = place;
        Ok(())
    }

    fn continue_(&mut self) -> Result<()> {
        if let Some((hart, watch, signal)) = self.resume(None)? {
            self.report(hart, watch, signal)?;
        }
        Ok(())
    }

    /// One instruction of the selected hart. False once the program has exited, or
    /// another stop got in the way (a breakpoint hit by a hart before it).
    fn step(&mut self) -> Result<bool> {
        let stepped = self.step_quietly()?;
        if stepped.is_some() {
            self.show_pc()?;
        }
        Ok(stepped.is_some())
    }

    /// One instruction of the selected hart, shown only if something else stopped
    /// the machine. The new pc, or `None` then (or once the program has exited).
    fn step_quietly(&mut self) -> Result<Option<u64>> {
        let hart = self.remote.selected()?;
        let Some((stopped, watch, signal)) = self.resume(Some(hart))? else {
            return Ok(None);
        };
        let pc = self.remote.register(remote::PC)?;
        if stopped != hart || watch.is_some() || signal != 5 || self.has_breakpoint(pc) {
            self.report(stopped, watch, signal)?;
            return Ok(None);
        }
        Ok(Some(pc))
    }

    /// The instruction at `pc`, decoded for the machine's width.
    fn inst_at(&mut self, pc: u64) -> Option<Inst> {
        let word = self.remote.read_word(pc)?;
        Some(excalibur::decode(word, self.xlen))
    }

    /// The address `n` bytes past `addr` (code or data), wrapping as the machine's
    /// addresses do.
    fn advance(&self, addr: u64, n: u64) -> u64 {
        wrap(self.xlen, addr.wrapping_add(n))
    }

    /// Where a call at `pc` returns to: after a `jal`/`jalr` that links (`call`,
    /// `jal f`), or after the `auipc ra` + `jalr ra` pair of a far call. `None` if
    /// the instruction there is no call.
    fn call_at(&mut self, pc: u64) -> Option<u64> {
        match self.inst_at(pc) {
            Some(Inst::Jal { rd, .. } | Inst::Jalr { rd, .. }) if rd != 0 => Some(self.advance(pc, 4)),
            Some(Inst::Auipc { rd: 1, .. }) => match self.inst_at(self.advance(pc, 4)) {
                Some(Inst::Jalr { rd: 1, rs1: 1, .. }) => Some(self.advance(pc, 8)),
                _ => None,
            },
            _ => None,
        }
    }

    /// One instruction, or a whole call, which runs until it returns.
    fn next(&mut self) -> Result<bool> {
        let pc = self.remote.register(remote::PC)?;
        match self.call_at(pc) {
            None => self.step(),
            Some(ret) => self.run_to(ret, |sp, sp0| sp >= sp0, true),
        }
    }

    /// `step` (`over` false) or `next`: runs the selected hart to the start of
    /// another source line. A call is run to its return by `next`, and by `step`
    /// too when the function has no line information; else `step` stops in the
    /// function, past its prologue. A return stops in the caller, on the line of
    /// the call. Without line information at the pc, one instruction (`stepi`,
    /// `nexti`). False if something else stopped the machine.
    fn step_line(&mut self, over: bool) -> Result<bool> {
        let pc = self.remote.register(remote::PC)?;
        let Some(start) = self.lines.at(pc) else {
            return if over { self.next() } else { self.step() };
        };
        let mut pc = pc;
        for _ in 0..STEP_LIMIT {
            if self.interrupt.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.say("interrupted");
                self.shown = None;
                self.show_pc()?;
                return Ok(false);
            }
            let returning = matches!(self.inst_at(pc), Some(Inst::Jalr { rd: 0, rs1: 1, .. }));
            pc = match self.call_at(pc) {
                Some(ret) if over => match self.run_to(ret, |sp, sp0| sp >= sp0, false)? {
                    true => ret,
                    false => return Ok(false),
                },
                Some(ret) => {
                    // Into the call: one step for `jal`/`jalr`, two for `auipc` + `jalr`.
                    let mut at = pc;
                    for _ in 0..wrap(self.xlen, ret.wrapping_sub(pc)) / 4 {
                        match self.step_quietly()? {
                            Some(next) => at = next,
                            None => return Ok(false),
                        }
                    }
                    if self.lines.at(at).is_none() {
                        // No lines in there: run it to its return, as gdb does.
                        if !self.run_to(ret, |sp, sp0| sp >= sp0, false)? {
                            return Ok(false);
                        }
                        ret
                    } else {
                        let (entry, end) = self.symbols.range(at).unwrap_or((at, at.wrapping_add(1)));
                        let body = self.lines.after_prologue(entry, end);
                        if body != at && !self.run_to(body, |_, _| true, false)? {
                            return Ok(false);
                        }
                        self.shown = None;
                        self.show_pc()?;
                        return Ok(true);
                    }
                }
                None => match self.step_quietly()? {
                    Some(next) => next,
                    None => return Ok(false),
                },
            };
            let here = self.lines.at(pc);
            let new_line = here.is_some() && here.as_ref() != Some(&start) && self.lines.starts_statement(pc);
            if new_line || (returning && here.is_some()) {
                self.shown = None;
                self.show_pc()?;
                return Ok(true);
            }
        }
        self.say(format!("still on {start} after {STEP_LIMIT} instructions"));
        self.shown = None;
        self.show_pc()?;
        Ok(false)
    }

    /// Runs until the function the selected hart is in returns to its caller.
    fn finish(&mut self) -> Result<()> {
        let frames = self.frames()?;
        let Some(caller) = frames.get(1) else {
            return Err("cannot find the caller (no frame above this one)".into());
        };
        let at = self.show(caller.pc);
        self.say(format!("run till exit, back to {at}"));
        // Back in the caller, sp is above where it is now (or the same, from a leaf).
        if self.run_to(caller.pc, |sp, sp0| sp >= sp0, true)? {
            let a0 = self.remote.register(10)?;
            let signed = self.signed(a0);
            self.say(format!("returned: a0 = {} ({signed})", self.hex(a0)));
        }
        Ok(())
    }

    /// Runs the selected hart until it reaches `target` with its stack pointer passing
    /// `stack_ok(sp, sp now)` (so a recursive call returning there does not count), with
    /// a temporary breakpoint. Another stop (a breakpoint, a watchpoint, an interrupt,
    /// the end) is reported and ends the run. True if it got there, where the pc is
    /// shown if `show` says so.
    fn run_to(&mut self, target: u64, stack_ok: impl Fn(u64, u64) -> bool, show: bool) -> Result<bool> {
        let hart = self.remote.selected()?;
        let sp0 = self.remote.register(2)?;
        let temporary = !self.has_breakpoint(target);
        if temporary {
            self.remote.set_breakpoint(target)?;
        }
        let result = loop {
            let stop = match self.resume(None) {
                Ok(Some(stop)) => stop,
                Ok(None) => break Ok(false),
                Err(e) => break Err(e),
            };
            let (stopped, watch, signal) = stop;
            let pc = self.remote.register(remote::PC)?;
            let ours = stopped == hart && watch.is_none() && signal == 5 && pc == target;
            if ours && stack_ok(self.remote.register(2)?, sp0) {
                if !temporary {
                    self.report(stopped, watch, signal)?;
                } else if show {
                    self.show_pc()?;
                }
                break Ok(true);
            }
            if !(ours && temporary) {
                self.report(stopped, watch, signal)?;
                break Ok(false);
            }
            // Our breakpoint, but another call deeper down: on with the run. The stop
            // selected the hart that hit it, ours.
        };
        if temporary && !self.exited {
            self.remote.clear_breakpoint(target)?;
        }
        if !self.exited {
            self.remote.select(hart)?;
        }
        result
    }

    // ---- Looking ----

    /// `regs`, `info registers`: GDB's layout for RISC-V, a register to a line, its
    /// name, its value in hex (no leading zeros) and then as its type in GDB's
    /// target description shows it. `ra` and `pc` are code pointers (with their
    /// symbol), `sp`, `gp`, `tp` and `fp` (`s0`) data pointers, and the rest signed
    /// integers of the register's width. `zero` is left out, as GDB leaves it, and
    /// `priv` (the privilege mode) comes last, as `info registers priv` shows it.
    fn regs(&mut self) -> Result<()> {
        let r = self.remote.registers()?;
        let mut lines = Vec::new();
        let line = |name: &str, hex: String, natural: String| format!("{name:<15}{hex}\t{natural}");
        for (n, &v) in r.iter().enumerate().take(32).skip(1) {
            // GDB calls x8 fp.
            let name = if n == 8 {
                "fp"
            } else {
                excalibur::reg::abi_name(n as u8)
            };
            let natural = match n {
                1 => self.show_plain(v),
                2 | 3 | 4 | 8 => format!("{v:#x}"),
                _ => self.signed(v).to_string(),
            };
            lines.push(line(name, format!("{v:#x}"), natural));
        }
        lines.push(line("pc", format!("{:#x}", r[32]), self.show_plain(r[32])));
        if let Ok(p) = self.remote.register(remote::PRIV) {
            let mode = match p {
                0 => "User/Application",
                1 => "Supervisor",
                2 => "Hypervisor",
                3 => "Machine",
                _ => "INVALID",
            };
            lines.push(line("priv", format!("{p:#x}"), format!("prv:{p} [{mode}]")));
        }
        self.say(lines.join("\n"));
        Ok(())
    }

    /// A register's value as a signed number of the register's width.
    fn signed(&self, v: u64) -> i64 {
        match self.xlen {
            Xlen::Rv32 => i64::from(v as u32 as i32),
            Xlen::Rv64 => v as i64,
        }
    }

    /// `x/N[b|h|w|g] ADDR`: GDB's unit letters, a byte, a halfword, a word (the
    /// default) and a giant (8 bytes), at both widths; `x`, hex, is the only format,
    /// and may come before or after the unit. As many on a line as GDB puts.
    fn examine(&mut self, format: &str, addr: &str) -> Result<()> {
        let format = format.strip_prefix('/').unwrap_or(format);
        let count = format.trim_end_matches(['b', 'h', 'w', 'g', 'x']);
        let unit = format[count.len()..].chars().rfind(|&c| c != 'x').unwrap_or('w');
        let count = if count.is_empty() {
            1
        } else {
            parse_number(count).ok_or_else(|| format!("bad count '{count}'"))? as usize
        };
        if addr.is_empty() {
            return Err("x/N ADDR".into());
        }
        let addr = self.eval(addr)?;
        let (size, per_line) = match unit {
            'b' => (1, 8),
            'h' => (2, 8),
            'w' => (4, 4),
            _ => (8, 2),
        };
        let bytes = self.remote.read_memory(addr, count * size)?;
        let mut lines = Vec::new();
        for (i, chunk) in bytes.chunks(per_line * size).enumerate() {
            let at = self.advance(addr, (i * per_line * size) as u64);
            let values: Vec<String> = chunk
                .chunks(size)
                .map(|v| match v.len() == size {
                    true => {
                        let n = v.iter().rev().fold(0u64, |n, &b| n << 8 | u64::from(b));
                        format!("{n:#0digits$x}", digits = 2 + 2 * size)
                    }
                    false => "?".into(),
                })
                .collect();
            lines.push(format!("{}:  {}", self.show_plain(at), values.join("  ")));
        }
        if bytes.len() < count * size {
            let end = self.advance(addr, bytes.len() as u64);
            lines.push(format!("(cannot read past {end:#x})"));
        }
        self.say(lines.join("\n"));
        Ok(())
    }

    fn disas_line(&mut self, pc: u64, here: bool) -> String {
        let mark = if here { "=>" } else { "  " };
        let at = self.show(pc);
        match self.remote.read_word(pc) {
            Some(word) => {
                let d = excalibur::decode(word, self.xlen).format(Some(pc), Syntax::objdump(self.xlen));
                let target = d
                    .target
                    .and_then(|t| self.symbols.describe(wrap(self.xlen, t)))
                    .map_or(String::new(), |s| format!(" <{s}>"));
                format!("{mark} {at}:  {d}{target}")
            }
            None => format!("{mark} {at}:  (cannot read memory)"),
        }
    }

    fn disas(&mut self, args: &[&str]) -> Result<()> {
        let pc = self.remote.register(remote::PC)?;
        let start = match args.first() {
            Some(a) => self.eval(a)?,
            None => pc,
        };
        let count = match args.get(1) {
            Some(n) => parse_number(n).ok_or("bad count")?,
            None => 8,
        };
        let lines: Vec<String> = (0..count)
            .map(|i| {
                let at = self.advance(start, 4 * i);
                self.disas_line(at, at == pc)
            })
            .collect();
        self.say(lines.join("\n"));
        Ok(())
    }

    fn frames(&mut self) -> Result<Vec<frames::Frame>> {
        let r = self.remote.registers()?;
        let regs = Regs {
            pc: r[32],
            ra: r[1],
            fp: r[8],
            xlen: self.xlen,
        };
        let symbols = &self.symbols;
        Ok(frames::backtrace(&mut self.remote, regs, |pc| {
            symbols.containing(pc).map(|(_, start)| start)
        }))
    }

    fn backtrace(&mut self) -> Result<()> {
        let frames = self.frames()?;
        let regs = self.remote.registers()?;
        let contexts = frame_contexts(&frames, &regs);
        let lines: Vec<String> = (0..frames.len())
            .map(|i| self.frame_line(i, &frames, &contexts[i]))
            .collect();
        self.say(lines.join("\n"));
        Ok(())
    }

    /// Frame `i` as GDB's backtrace shows it: `#1  0x... in walk (list=0x80001010
    /// <nodes+8>, depth=1) at vars.c:29`. The address is left out of the innermost
    /// frame when its pc starts a line; the arguments come from the debugging
    /// information and the name from it or the symbols (`??` without either).
    fn frame_line(&mut self, i: usize, frames: &[frames::Frame], ctx: &vars::Frame) -> String {
        let pc = frames[i].pc;
        // A caller's pc is the return address: its call is the instruction
        // before, on the line (and in the block) that made the call.
        let looked_up = if i == 0 { pc } else { pc.wrapping_sub(4) };
        let place = self.lines.at(looked_up);
        let address = if i == 0 && place.is_some() && self.lines.starts_statement(pc) {
            String::new()
        } else {
            format!("{} in ", self.hex(pc))
        };
        let func = self.debug.function_at(ctx.pc);
        let (name, args) = match func {
            Some(f) => {
                let name = self.debug.name(f).unwrap_or("??").to_string();
                let mut eval = Eval {
                    debug: &self.debug,
                    symbols: &self.symbols,
                    target: &mut self.remote,
                    frame: Some(ctx.clone()),
                    xlen: self.xlen,
                };
                (name, eval.show_args(f))
            }
            None => (
                self.symbols
                    .containing(looked_up)
                    .map_or("??".to_string(), |(n, _)| n.to_string()),
                String::new(),
            ),
        };
        let at = place.map_or(String::new(), |p| format!(" at {p}"));
        format!("#{i}  {address}{name} ({args}){at}")
    }

    /// The selected frame's registers, as far as they are known.
    fn frame_context(&mut self) -> Result<Option<vars::Frame>> {
        let frames = self.frames()?;
        if self.frame >= frames.len() {
            self.frame = 0;
        }
        let regs = self.remote.registers()?;
        Ok(frame_contexts(&frames, &regs).into_iter().nth(self.frame))
    }

    /// `frame N`, `up`, `down`: selects frame `n` and shows it, and its line.
    fn select_frame(&mut self, n: usize) -> Result<()> {
        let frames = self.frames()?;
        if n >= frames.len() {
            return Err("initial frame selected; you cannot go up".into());
        }
        self.frame = n;
        let regs = self.remote.registers()?;
        let contexts = frame_contexts(&frames, &regs);
        let line = self.frame_line(n, &frames, &contexts[n]);
        self.say(line);
        let pc = if n == 0 {
            frames[0].pc
        } else {
            frames[n].pc.wrapping_sub(4)
        };
        if let Some(place) = self.lines.at(pc) {
            let line = match self.lines.text(&place.file, place.line) {
                Some(text) => format!("{place}: {text}"),
                None => place.to_string(),
            };
            self.say(line);
            self.listed = Some((place.file.clone(), place.line.saturating_sub(5).max(1)));
        }
        Ok(())
    }

    /// An evaluator in the selected frame.
    fn eval_in_frame(&mut self) -> Result<Eval<'_, crate::remote::Remote>> {
        let frame = self.frame_context()?;
        Ok(Eval {
            debug: &self.debug,
            symbols: &self.symbols,
            target: &mut self.remote,
            frame,
            xlen: self.xlen,
        })
    }

    /// `print EXPR`, `print/x EXPR`.
    fn print_(&mut self, expr: &str, format: Option<&str>) -> Result<()> {
        let hex = match format {
            None | Some("d") => false,
            Some("x") => true,
            Some(f) => return Err(format!("undefined output format \"{f}\"")),
        };
        if expr.is_empty() {
            return Err("print needs an expression".into());
        }
        let mut eval = self.eval_in_frame()?;
        let v = eval.value(expr)?;
        let shown = eval.show(&v, true, hex)?;
        self.printed += 1;
        let n = self.printed;
        self.say(format!("${n} = {shown}"));
        Ok(())
    }

    /// `ptype` (`full`) and `whatis`, of a type name or an expression.
    fn ptype(&mut self, arg: &str, full: bool) -> Result<()> {
        if arg.is_empty() {
            return Err("ptype and whatis need an expression or a type".into());
        }
        let mut eval = self.eval_in_frame()?;
        let text = match eval.type_name(arg) {
            Some(t) if full => self.debug.ptype(&t),
            Some(t) => self.debug.whatis_type(&t),
            None => {
                let v = eval.value(arg)?;
                let t = eval.type_of(&v);
                if full {
                    self.debug.ptype(&t)
                } else {
                    self.debug.declare(&t, "")
                }
            }
        };
        self.say(format!("type = {text}"));
        Ok(())
    }

    /// `info locals`: the variables of the selected frame's blocks, the
    /// innermost first, as GDB lists them.
    fn info_locals(&mut self) -> Result<()> {
        let Some(ctx) = self.frame_context()? else {
            return Err("no frame selected".into());
        };
        let Some(func) = self.debug.function_at(ctx.pc) else {
            self.say("No symbol table info available.");
            return Ok(());
        };
        let vars: Vec<_> = self.debug.locals(func, ctx.pc).into_iter().flatten().collect();
        if vars.is_empty() {
            self.say("No locals.");
            return Ok(());
        }
        let mut eval = self.eval_in_frame()?;
        let lines = eval.show_vars(&vars);
        self.say(lines.join("\n"));
        Ok(())
    }

    /// `info args`: the selected frame's arguments.
    fn info_args(&mut self) -> Result<()> {
        let Some(ctx) = self.frame_context()? else {
            return Err("no frame selected".into());
        };
        let Some(func) = self.debug.function_at(ctx.pc) else {
            self.say("No symbol table info available.");
            return Ok(());
        };
        let params = self.debug.params(func);
        if params.is_empty() {
            self.say("No arguments.");
            return Ok(());
        }
        let mut eval = self.eval_in_frame()?;
        let lines = eval.show_vars(&params);
        self.say(lines.join("\n"));
        Ok(())
    }

    /// `list`: ten lines of source around a line (`FILE:LINE`, `LINE` of the file
    /// listed last, a function's first line), or the ten after those listed last,
    /// or around the pc.
    fn list(&mut self, arg: &str) -> Result<()> {
        let (file, first) = if arg.is_empty() {
            match self.listed.clone() {
                Some(next) => next,
                None => {
                    let pc = self.remote.register(remote::PC)?;
                    let place = self.lines.at(pc).ok_or("no line information here")?;
                    (place.file, place.line.saturating_sub(5).max(1))
                }
            }
        } else {
            let place = if let Some((file, line)) = source_line(arg) {
                Place {
                    file: self.lines.file_named(file)?,
                    line,
                }
            } else if let Some(line) = arg.parse::<u32>().ok().filter(|_| !arg.starts_with('0')) {
                let file = match &self.listed {
                    Some((file, _)) => file.clone(),
                    None => {
                        let pc = self.remote.register(remote::PC)?;
                        self.lines.at(pc).ok_or("no line information here")?.file
                    }
                };
                Place { file, line }
            } else {
                let addr = self.eval(arg)?;
                self.lines
                    .at(addr)
                    .ok_or_else(|| format!("no line information for {arg}"))?
            };
            (place.file, place.line.saturating_sub(5).max(1))
        };
        let count = self
            .lines
            .line_count(&file)
            .ok_or_else(|| format!("cannot find {file} (try directory DIR)"))?;
        if first > count {
            return Err(format!("{file} has {count} lines"));
        }
        let last = (first + 9).min(count);
        let mut out = Vec::new();
        for n in first..=last {
            out.push(format!("{n}\t{}", self.lines.text(&file, n).unwrap_or_default()));
        }
        self.say(out.join("\n"));
        self.listed = Some((file, last + 1));
        Ok(())
    }

    /// Every hart: the stub's description and its pc. The selection stays.
    fn harts(&mut self) -> Result<()> {
        let selected = self.remote.selected()?;
        let mut lines = Vec::new();
        for h in 0..self.remote.harts {
            self.remote.select(h)?;
            let pc = self.remote.register(remote::PC)?;
            let info = self.remote.hart_info(h).unwrap_or_else(|_| format!("hart {h}"));
            let mark = if h == selected { "*" } else { " " };
            lines.push(format!("{mark} {info}, at {}", self.show(pc)));
        }
        self.remote.select(selected)?;
        self.say(lines.join("\n"));
        Ok(())
    }

    fn select_hart(&mut self, n: &str) -> Result<()> {
        let h: usize = n.parse().map_err(|_| format!("'{n}' is not a hart number"))?;
        if h >= self.remote.harts {
            return Err(format!("no hart {h}: the machine has {}", self.remote.harts));
        }
        self.remote.select(h)?;
        self.say(format!("hart {h}"));
        self.show_pc()
    }
}

/// How many instructions `step` and `next` run at most looking for another line.
const STEP_LIMIT: u32 = 1_000_000;

/// `FILE:LINE` (a file name, a colon, a decimal line number), if `s` is one.
/// The registers each frame has, as far as they are known: all of them in the
/// innermost; in a caller its frame pointer, and its `sp`, which is where the frame
/// it called began (that frame's CFA, its `s0`) or, past a frame not built yet
/// (whose `s0` is still the caller's), that frame's own `sp`. The pc to look its
/// scopes up at is the call's for a caller.
fn frame_contexts(frames: &[frames::Frame], regs: &[u64]) -> Vec<vars::Frame> {
    let mut out: Vec<vars::Frame> = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let mut r = [None; 32];
        if i == 0 {
            for (n, v) in regs.iter().take(32).enumerate() {
                r[n] = Some(*v);
            }
            r[0] = Some(0);
        } else {
            let callee = &frames[i - 1];
            r[8] = Some(f.fp);
            r[2] = if callee.fp != f.fp {
                Some(callee.fp)
            } else {
                out[i - 1].regs[2]
            };
        }
        let pc = if i == 0 { f.pc } else { f.pc.wrapping_sub(1) };
        out.push(vars::Frame { pc, regs: r });
    }
    out
}

fn source_line(s: &str) -> Option<(&str, u32)> {
    let (file, line) = s.rsplit_once(':')?;
    let line = line.trim().parse::<u32>().ok().filter(|&l| l > 0)?;
    let file = file.trim();
    (!file.is_empty()).then_some((file, line))
}

/// A count argument (1 if absent).
fn count(args: &[&str]) -> Result<u32> {
    match args.first() {
        None => Ok(1),
        Some(n) => parse_number(n)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("bad count '{n}'")),
    }
}

/// `0x...` hex or decimal, up to 64 bits.
pub fn parse_number(s: &str) -> Option<u64> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

/// GDB's number for a register name: `pc`, `x0`-`x31`, an ABI name (`fp` too), a CSR
/// name (those the machine's width has: RV64 has no `cycleh`), or `priv`.
pub fn register_number(name: &str, xlen: Xlen) -> Option<usize> {
    match name {
        "pc" => return Some(remote::PC),
        "fp" => return Some(8),
        "priv" => return Some(remote::PRIV),
        _ => {}
    }
    if let Some(n) = name.strip_prefix('x').and_then(|n| n.parse::<usize>().ok()) {
        return (n < 32).then_some(n);
    }
    if let Some(r) = excalibur::reg::parse(name) {
        return Some(r as usize);
    }
    excalibur::csr::parse(name, xlen).map(remote::csr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_lines() {
        assert_eq!(source_line("kernel/main.c:42"), Some(("kernel/main.c", 42)));
        assert_eq!(source_line("main.c:7"), Some(("main.c", 7)));
        assert_eq!(source_line("C:/src/main.c:7"), Some(("C:/src/main.c", 7)));
        assert_eq!(source_line("main"), None);
        assert_eq!(source_line("main.c:"), None);
        assert_eq!(source_line(":3"), None);
        assert_eq!(source_line("main.c:0"), None);
        assert_eq!(source_line("main.c:x"), None);
    }

    #[test]
    fn numbers_and_registers() {
        assert_eq!(parse_number("0x80000000"), Some(0x8000_0000));
        assert_eq!(parse_number("42"), Some(42));
        assert_eq!(parse_number("0xffffffff80000000"), Some(0xffff_ffff_8000_0000));
        assert_eq!(parse_number("main"), None);
        for xlen in [Xlen::Rv32, Xlen::Rv64] {
            assert_eq!(register_number("sp", xlen), Some(2));
            assert_eq!(register_number("x31", xlen), Some(31));
            assert_eq!(register_number("x32", xlen), None);
            assert_eq!(register_number("fp", xlen), Some(8));
            assert_eq!(register_number("s0", xlen), Some(8));
            assert_eq!(register_number("pc", xlen), Some(32));
            assert_eq!(register_number("mstatus", xlen), Some(65 + 0x300));
            assert_eq!(register_number("priv", xlen), Some(4161));
            assert_eq!(register_number("nope", xlen), None);
        }
        // The high halves of the counters are RV32's alone.
        assert_eq!(register_number("cycleh", Xlen::Rv32), Some(65 + 0xc80));
        assert_eq!(register_number("cycleh", Xlen::Rv64), None);
    }
}
