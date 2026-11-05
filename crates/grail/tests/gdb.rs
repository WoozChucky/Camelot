//! `grail --gdb`: the stub behind the command line, talked to over TCP with raw
//! packets, acks included. merlin-dbg's tests drive the same stub with the debugger
//! (crates/merlin-dbg/tests/session.rs). The ignored test checks it with LLDB.

use grail::gdb::packet::{self, Decoder, Event};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

/// `grail --gdb 127.0.0.1:0 args...`, and the address it says it listens on.
fn grail(args: &[&str], elf: &str) -> (Child, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--gdb", "127.0.0.1:0"])
        .args(args)
        .arg(path(elf))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let addr = line
        .strip_prefix("grail: gdb stub listening on ")
        .and_then(|rest| rest.split([',', '\n']).next())
        .and_then(|a| a.trim().parse().ok())
        .unwrap_or_else(|| panic!("{line}"));
    // Keep draining stderr so grail never blocks on it.
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    (child, addr)
}

/// A client that acks, as GDB does before no-ack mode.
struct Client {
    stream: TcpStream,
    decoder: Decoder,
}

impl Client {
    fn connect(addr: SocketAddr) -> Client {
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        Client {
            stream,
            decoder: Decoder::default(),
        }
    }

    fn event(&mut self) -> Event {
        let mut b = [0u8];
        loop {
            self.stream.read_exact(&mut b).unwrap();
            if let Some(e) = self.decoder.push(b[0]) {
                return e;
            }
        }
    }

    /// Sends a packet, expects the `+`, returns the reply (acked).
    fn ask(&mut self, payload: &str) -> String {
        self.send(payload);
        self.reply()
    }

    fn send(&mut self, payload: &str) {
        self.stream.write_all(&packet::frame(payload.as_bytes())).unwrap();
        assert_eq!(self.event(), Event::Ack, "{payload}");
    }

    fn reply(&mut self) -> String {
        match self.event() {
            Event::Packet(p) => {
                self.stream.write_all(b"+").unwrap();
                String::from_utf8(p).unwrap()
            }
            other => panic!("{other:?}"),
        }
    }
}

fn symbol(elf: &str, name: &str) -> u32 {
    let file = std::fs::read(path(elf)).unwrap();
    let elf = elf::Elf::parse(&file).unwrap();
    let s = elf.symbols().unwrap();
    s.iter().find(|s| s.name == name).unwrap().value as u32
}

fn le(v: u32) -> String {
    v.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// An RV64 register's value as the stub sends it: 8 bytes, little-endian, in hex.
fn le64(v: u64) -> String {
    v.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// A program for an RV64 machine, as an ELF64 file in the temporary directory (named
/// after `name`): one `PT_LOAD` segment at the start of RAM, its entry, holding
///
/// ```text
/// 0x80000000  addi a0, zero, 5
/// 0x80000004  slli a0, a0, 40     a0 = 0x500_0000_0000, past 32 bits
/// 0x80000008  addi a1, a0, 1
/// 0x8000000c  j .
/// ```
fn rv64_program(name: &str) -> PathBuf {
    let words: [u32; 4] = [
        0x0050_0513, // addi a0, zero, 5
        0x0285_1513, // slli a0, a0, 40
        0x0015_0593, // addi a1, a0, 1
        0x0000_006f, // j .
    ];
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut f = vec![0u8; 64 + 56];
    f[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\0");
    f[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    f[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    f[20..24].copy_from_slice(&1u32.to_le_bytes());
    f[24..32].copy_from_slice(&0x8000_0000u64.to_le_bytes()); // e_entry
    f[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    f[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    f[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    f[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    f[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
    let ph = &mut f[64..];
    ph[..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    ph[4..8].copy_from_slice(&5u32.to_le_bytes()); // R + X
    ph[8..16].copy_from_slice(&120u64.to_le_bytes()); // p_offset
    ph[16..24].copy_from_slice(&0x8000_0000u64.to_le_bytes()); // p_vaddr
    ph[24..32].copy_from_slice(&0x8000_0000u64.to_le_bytes()); // p_paddr
    ph[32..40].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_filesz
    ph[40..48].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_memsz
    ph[48..56].copy_from_slice(&4u64.to_le_bytes()); // p_align
    f.extend_from_slice(&code);
    let path = std::env::temp_dir().join(format!("grail-gdb-{name}-{}.elf", std::process::id()));
    std::fs::write(&path, f).unwrap();
    path
}

#[test]
fn gdb_wait_breaks_steps_and_runs_to_the_end() {
    let hello = "examples/hello/hello.elf";
    let puts = symbol(hello, "puts");
    let (child, addr) = grail(&["--gdb-wait"], hello);
    let mut c = Client::connect(addr);
    assert!(c.ask("qSupported:swbreak+").contains("qXfer:features:read+"));
    // Stopped at the first instruction, the boot ROM's.
    assert_eq!(c.ask("?"), "T02thread:1;20:00100000;");
    assert_eq!(c.ask(&format!("Z0,{puts:x},4")), "OK");
    // A bad checksum is answered with '-', and the packet sent again.
    c.stream.write_all(b"$g#00").unwrap();
    assert_eq!(c.event(), Event::Nack);
    assert_eq!(c.ask("c"), format!("T05swbreak:;thread:1;20:{};", le(puts)));
    let g = c.ask("g");
    assert_eq!(&g[32 * 8..], le(puts), "pc");
    assert_eq!(&g[10 * 8..11 * 8], le(symbol(hello, "message")), "a0");
    assert_eq!(c.ask(&format!("m{:x},5", symbol(hello, "message"))), "48656c6c6f");
    assert_eq!(c.ask("s"), format!("T05thread:1;20:{};", le(puts + 4)));
    // Into no-ack mode: no more '+'.
    assert_eq!(c.ask("QStartNoAckMode"), "OK");
    c.stream
        .write_all(&packet::frame(format!("z0,{puts:x},4").as_bytes()))
        .unwrap();
    assert_eq!(c.reply(), "OK");
    c.stream.write_all(&packet::frame(b"c")).unwrap();
    assert_eq!(c.reply(), "W00");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hello from Camelot!\n");
}

#[test]
fn an_rv64_machine_is_debugged_with_64_bit_registers() {
    // The same protocol, as wide as the machine: the description says riscv:rv64,
    // and the registers, the pc and the addresses are 64-bit.
    let program = rv64_program("rv64");
    let (child, addr) = grail(&["--gdb-wait"], program.to_str().unwrap());
    let mut c = Client::connect(addr);
    c.ask("qSupported:swbreak+");
    let xml = c.ask("qXfer:features:read:target.xml:0,3fff");
    assert!(xml.contains("<architecture>riscv:rv64</architecture>"), "{xml}");
    assert!(xml.contains("bitsize=\"64\""), "{xml}");
    assert_eq!(c.ask("?"), format!("T02thread:1;20:{};", le64(0x1000)));
    assert_eq!(c.ask("Z0,80000008,4"), "OK");
    assert_eq!(
        c.ask("c"),
        format!("T05swbreak:;thread:1;20:{};", le64(0x8000_0008))
    );
    let g = c.ask("g");
    assert_eq!(g.len(), 33 * 16);
    assert_eq!(&g[10 * 16..11 * 16], le64(5 << 40), "a0");
    assert_eq!(c.ask("pa"), le64(5 << 40));
    assert_eq!(c.ask("s"), format!("T05thread:1;20:{};", le64(0x8000_000c)));
    assert_eq!(c.ask("pb"), le64((5 << 40) + 1), "a1");
    assert_eq!(c.ask("m80000000,4"), "13055000");
    assert_eq!(c.ask(&format!("Pa={}", le64(0x1234_5678_9abc_def0))), "OK");
    assert_eq!(c.ask("pa"), le64(0x1234_5678_9abc_def0));
    c.stream.write_all(&packet::frame(b"k")).unwrap();
    assert_eq!(c.event(), Event::Ack);
    assert_eq!(c.event(), Event::Packet(b"X09".to_vec()));
    drop(child.wait_with_output().unwrap());
    std::fs::remove_file(program).unwrap();
}

#[test]
fn a_client_stops_a_running_machine_interrupts_and_kills_it() {
    // Without --gdb-wait the kernel boots on its own; it then waits in its shell for
    // input that never comes.
    let (mut child, addr) = grail(&[], "kernel/kernel.elf");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("Type help") {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "the kernel stopped");
    }
    let mut c = Client::connect(addr);
    // Connecting stopped it.
    assert!(c.ask("?").starts_with("T02thread:1;"));
    c.send("vCont;c");
    std::thread::sleep(std::time::Duration::from_millis(200));
    c.stream.write_all(&[0x03]).unwrap();
    assert!(c.reply().starts_with("T02thread:1;"));
    // The machine is in the kernel or its idle loop, in S-mode (priv is 4161).
    assert_eq!(c.ask("p1041"), "01000000");
    c.stream.write_all(&packet::frame(b"k")).unwrap();
    assert_eq!(c.event(), Event::Ack);
    // A reply LLDB waits for (GDB does not): ended by SIGKILL.
    assert_eq!(c.event(), Event::Packet(b"X09".to_vec()));
    assert_eq!(child.wait().unwrap().code(), Some(137));
}

#[test]
fn detaching_leaves_the_machine_running() {
    let (child, addr) = grail(&["--gdb-wait"], "examples/hello/hello.elf");
    let mut c = Client::connect(addr);
    assert_eq!(c.ask("Z0,80000020,4"), "OK");
    assert_eq!(c.ask("D"), "OK");
    // The breakpoint went with the client: the program runs to its end.
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hello from Camelot!\n");
}

/// A client whose connection ends without `D` leaves the machine as it was, as QEMU's
/// stub does: running, the breakpoints it set still stop it, and it then stays stopped
/// with nobody to tell until the next client comes; that one's `?` removes them.
#[test]
fn a_connection_that_ends_leaves_the_machine_as_it_was() {
    let hello = "examples/hello/hello.elf";
    let puts = symbol(hello, "puts");
    let (child, addr) = grail(&["--gdb-wait"], hello);
    let mut c = Client::connect(addr);
    assert_eq!(c.ask(&format!("Z0,{puts:x},4")), "OK");
    // The loop over the message's bytes, hit once a byte.
    assert_eq!(c.ask(&format!("Z0,{:x},4", puts + 8)), "OK");
    c.send("c");
    drop(c);
    // Stopped at puts, what puts writes not written yet.
    let mut c = Client::connect(addr);
    assert_eq!(c.ask("?"), format!("T02thread:1;20:{};", le(puts)));
    // The `?` took the breakpoints away: the program runs to its end.
    assert_eq!(c.ask("c"), "W00");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hello from Camelot!\n");
}

#[test]
fn gdb_options_are_checked() {
    for args in [
        &["--gdb"][..],
        &["--gdb", "1234", "--max-steps", "9"],
        &["--gdb", "1234", "--fb"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_grail"))
            .args(args)
            .arg(path("examples/hello/hello.elf"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}

/// LLDB (from the LLVM installer) connected to `grail --gdb`, as a standard client:
/// it must break at a symbol, read registers and step. Ignored: it needs LLDB, and
/// LLDB on Windows needs a Python runtime on the PATH (`python3.dll` of any Python 3
/// for LLVM 23; LLVM 21 wanted exactly `python310.dll`). `LLDB` names the binary if it
/// is not on the PATH.
#[test]
#[ignore]
fn lldb_breaks_steps_and_reads_registers() {
    let lldb = std::env::var("LLDB").unwrap_or_else(|_| "lldb".into());
    let hello = path("examples/hello/hello.elf");
    let (child, addr) = grail(&["--gdb-wait"], "examples/hello/hello.elf");
    let script = [
        format!("target create {}", hello.display()),
        format!("gdb-remote {addr}"),
        "breakpoint set --name puts".into(),
        "continue".into(),
        "register read pc a0".into(),
        "stepi".into(),
        "register read pc t0".into(),
        "memory read --size 1 --count 5 --format x 0x80000050".into(),
        "continue".into(),
    ];
    let mut cmd = Command::new(&lldb);
    cmd.arg("-b");
    for line in &script {
        cmd.args(["-o", line]);
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("cannot run {lldb}: {e}"));
    let text = String::from_utf8_lossy(&out.stdout);
    // Each check is a line holding all its parts: LLDB's spacing and how it shows
    // symbols change between versions (21: `pc = 0x80000020  hello.elf`puts`, 23:
    // `pc = 0x80000020 hello.elf`puts  hello.elf`puts`).
    let checks: [&[&str]; 7] = [
        &["stop reason = breakpoint 1.1"],
        &["pc = 0x80000020", "hello.elf`puts"],
        &["a0 = 0x80000050", "hello.elf`message"],
        &["pc = 0x80000024", "hello.elf`puts + 4"],
        &["t0 = 0x10000000"],
        &["0x48 0x65 0x6c 0x6c 0x6f"],
        &["exited with status = 0"],
    ];
    for parts in checks {
        assert!(
            text.lines().any(|l| parts.iter().all(|p| l.contains(p))),
            "no line with {parts:?} in:
{text}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hello from Camelot!\n");
}

/// LLDB attached to an RV64 machine, as to an RV32 one above: it breaks at an address,
/// reads 64-bit registers and steps. Ignored, for the same reasons.
#[test]
#[ignore]
fn lldb_debugs_an_rv64_machine() {
    let lldb = std::env::var("LLDB").unwrap_or_else(|_| "lldb".into());
    let program = rv64_program("lldb");
    let (child, addr) = grail(&["--gdb-wait"], program.to_str().unwrap());
    let script = [
        format!("target create {}", program.display()),
        format!("gdb-remote {addr}"),
        "breakpoint set --address 0x80000008".into(),
        "continue".into(),
        "register read pc a0".into(),
        "stepi".into(),
        "register read pc a1".into(),
        "process kill".into(),
    ];
    let mut cmd = Command::new(&lldb);
    cmd.arg("-b");
    for line in &script {
        cmd.args(["-o", line]);
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("cannot run {lldb}: {e}"));
    let text = String::from_utf8_lossy(&out.stdout);
    let checks: [&[&str]; 4] = [
        &["stop reason = breakpoint 1.1"],
        &["pc = 0x0000000080000008"],
        &["a0 = 0x0000050000000000"],
        &["a1 = 0x0000050000000001"],
    ];
    for parts in checks {
        assert!(
            text.lines().any(|l| parts.iter().all(|p| l.contains(p))),
            "no line with {parts:?} in:\n{text}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    drop(child.wait_with_output());
    std::fs::remove_file(program).unwrap();
}
