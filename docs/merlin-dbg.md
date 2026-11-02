# merlin-dbg and Grail's GDB stub

merlin-dbg is the Camelot debugger. This document covers its first layer: machine-level debugging of any program running in Grail, with no help from the compiler. Breakpoints, watchpoints, single steps, registers, CSRs, memory, disassembly and backtraces by frame pointer, on one hart or several.

It has three parts:

- **Grail's debug interface** (`grail::debug`): what a debugger needs from the emulator, as methods on `Machine`.
- **Grail's GDB stub** (`grail::gdb`, `grail --gdb ADDR`): that interface over TCP, in the GDB remote serial protocol. merlin-dbg, GDB and LLDB can all connect to it.
- **merlin-dbg** (`crates/merlin-dbg`): a GDB remote protocol client with a command line. It either connects to a running `grail --gdb` or starts a machine in its own process.

The spec's section 7, "Debugging", says what the machine guarantees under a debugger.

## Quick start

Start the program under the debugger and give it commands. merlin-dbg starts Grail itself with whatever follows `--`:

```
cargo run --release -p merlin-dbg -- -- examples/hello/hello.elf
```

```
=> 0x00001000:  auipc t0, 0x0
(merlin-dbg) break puts
breakpoint 1 at 0x80000020 <puts>
(merlin-dbg) continue
breakpoint 1
=> 0x80000020 <puts>:  lui t0, 0x10000
(merlin-dbg) x/5b $a0
0x80000050 <message>:  0x48  0x65  0x6c  0x6c  0x6f
(merlin-dbg) stepi
=> 0x80000024 <puts+4>:  mv t1, a0
(merlin-dbg) bt
#0  0x80000024 <puts+4>
#1  0x80000018 <_start+24>
(merlin-dbg) continue
Hello from Camelot!
[dbg] pc=80000018 a0 = 0x00000014 (20)
program exited with code 0
```

Or run Grail with its stub and connect to it, from another terminal or from GDB or LLDB:

```
cargo run --release -p grail -- --gdb-wait kernel/kernel.elf
cargo run --release -p merlin-dbg -- --connect 1234 kernel/kernel.elf
```

## The debug interface (`grail::debug`)

A `Machine` that is not being run is paused. The debugger runs it with:

- `debug_continue(steps, trace)`: runs until something stops the machine or `steps` steps have run. In the second case it returns `None` and gives the host control back, so the host can check for a pause request, such as a client's Ctrl-C, and then call again. With a real-time clock it also returns once the machine is ahead of the wall clock.
- `debug_step(trace)`: one instruction of the selected hart.
- `debug_pause()`: marks a stop between two calls (a pause the host decided on).

The stops (`Stop`) are:

- `Breakpoint { hart, pc }`: the hart is about to execute the instruction at `pc`.
- `Watchpoint(WatchHit { hart, kind, addr })`: an instruction of the hart read or wrote a watched byte. The instruction has completed, and the pc is the next one.
- `Stepped { hart }`.
- `Exited(code)`.

Breakpoints (`set_breakpoint`, `clear_breakpoint`) are not written into guest memory. Before each hart's turn, the machine checks whether that hart is about to execute an instruction at a breakpoint's address. It must really be about to execute it: a hart that will take an interrupt or wait in `wfi` instead is not stopped. The program never sees a breakpoint, can't overwrite one, and checksums of its own code stay right. Breakpoint addresses are virtual addresses, compared with each hart's pc, whatever the hart's mode.

Watchpoints (`set_watchpoint(addr, len, kind)`, with `kind` `Write`, `Read` or `Access`) watch a range of virtual addresses for loads and stores by any hart:

- Loads and stores count, and so do `lr.w`, a successful `sc.w`, and AMOs (which both read and write).
- Instruction fetches, page-table walks and device DMA don't count.
- An access that faults doesn't count either.

The selected hart (`select_hart`, hart 0 to begin with) is the one these read and write:

- `reg`, `set_reg`, `pc`, `set_pc`.
- `mode`, `set_mode`.
- `csr`, `set_csr`: any CSR Grail has, whatever the hart's privilege, with the CSR's WARL rules on writes.

`read_memory` and `write_memory` access memory, in one of two spaces:

- `Space::Physical`: physical addresses.
- `Space::Virtual`: translated as the hart's instruction fetches are now. That means physical in M-mode or with `satp` off, through its Sv32 page table otherwise, with no permission checks, so a debugger can read a user page from S-mode.

Memory reads and writes stop at the first byte that is not memory (RAM, VRAM, or the boot ROM for reads). Device registers are never accessed, since reading one can change it. A write breaks the LR/SC reservations on the words it changes, as a store would.

### Determinism

A debugged run executes exactly the instructions of an undebugged one, in the same order, with the same results. Three things guarantee it:

- **Stops between turns.** A step runs one instruction on each hart in turn (spec section 7, "Harts"). The machine stops between two harts' turns (`Cpu::turn_traced`), and it resumes from that same turn.
- **Resuming steps over a breakpoint.** After any stop, the first turn executes its instruction even if there is a breakpoint at its pc, so continuing from a breakpoint does not stop on it again.
- **Reads have no side effects.** Nothing the debugger reads changes the machine: no device register reads, no A/D bits set by its translations.

`crates/grail/tests/debug.rs` checks this on hello, smp (2 and 3 harts) and the kernel. It stops the program many times and compares every turn (hart, pc, mode, `mtime`), the console output, the registers and the counters with an undebugged run.

## The GDB stub

`grail --gdb ADDR` runs the machine under the stub, listening on ADDR. ADDR can be:

- `HOST:PORT`;
- a host or IP address, which gets port 1234;
- a port alone, on 127.0.0.1.

Grail says where it listens on stderr: `grail: gdb stub listening on 127.0.0.1:1234`. Port 0 picks a free port, and that line tells you which.

What happens next depends on `--gdb-wait`:

- **Without it**, the machine starts at once and runs on its own, and a client that connects stops it.
- **With it**, the machine waits for a client, stopped at its first instruction (hart 0 at the boot ROM's 0x1000). `--gdb-wait` alone listens on 127.0.0.1:1234.

How the session ends:

- **Exit:** the program exits, the client gets `W` and its exit code, and Grail exits with that code.
- **Kill** (`k`): Grail exits with status 137.
- **Detach** (`D`), or the client disconnects: the machine runs on without breakpoints or watchpoints, and the next client to connect stops it.

The stub serves one client at a time. `--gdb` cannot be used with `--fb`, `--fb-dump` or `--max-steps`. It can be used with `--trace` and every machine option.

> **Security:** the stub can read and write all of the machine's memory and registers, and it has no authentication. Keep it on 127.0.0.1 (the default) unless you trust everything on the network.

### Protocol subset

Packets are `$payload#checksum`, with `+`/`-` acks until `QStartNoAckMode`. A 0x03 byte outside a packet interrupts a running machine (Ctrl-C). Binary data is escaped with `}`. Harts are threads: hart `i` is thread `i + 1`, thread 0 means "any", and thread -1 means "all".

| Packet | Reply |
|---|---|
| `qSupported[:features]` | `PacketSize=4000;qXfer:features:read+;QStartNoAckMode+;swbreak+;vContSupported+`. A client that sends `swbreak+` gets `swbreak:;` in breakpoint stop replies |
| `QStartNoAckMode` | `OK`, then no more acks either way |
| `?` | the last stop (see below) |
| `g` / `G data` | `x0`-`x31` and `pc` of the selected hart, 4 little-endian bytes each in hex / writes as many of them as `data` holds |
| `p n` / `P n=value` | register `n`: 0-31 `x0`-`x31`, 32 `pc`, 65 + `a` CSR `a`, 4161 `priv` (0 U, 1 S, 3 M; writing it changes the mode). GDB's own numbering, so a client agrees with it with or without the target description |
| `m addr,len` | the bytes that can be read from `addr` on, through the selected hart's translation, at most 4096 |
| `M addr,len:hex`, `X addr,len:binary` | `OK` if all `len` bytes were written |
| `c [addr]`, `C sig[;addr]`, `s [addr]`, `S sig[;addr]` | resumes (at `addr` if given; signals are ignored). `c` replies when the machine stops, `s` after one instruction of the `Hc` hart |
| `vCont?` | `vCont;c;C;s;S` |
| `vCont;action[:thread]...` | a step (`s`/`S`) of its thread if there is one, else continue. Every hart runs either way: a step of one hart is a turn of each hart before it |
| `Z0`/`z0`, `Z1`/`z1 ,addr,kind` | sets/clears a breakpoint (software and hardware ones are the same thing here). Conditions after `;` are ignored |
| `Z2`-`Z4` / `z2`-`z4 ,addr,len` | sets/clears a write, read or access watchpoint on `len` bytes |
| `Hg thread`, `Hc thread` | selects the hart for registers and memory / for `c` and `s` |
| `T thread` | `OK` if the hart exists |
| `qfThreadInfo`, `qsThreadInfo`, `qC` | `m1,2,...`, `l`, `QC` and the selected thread |
| `qThreadExtraInfo,thread` | e.g. `hart 1, S-mode, in wfi`, hex encoded |
| `qAttached` | `1` |
| `qXfer:features:read:target.xml:off,len` | the target description: `riscv:rv32`, the `org.gnu.gdb.riscv.cpu` registers, every CSR Grail implements that has a name, and `priv` |
| `qRegisterInfoN` | LLDB's register description, for `x0`-`x31` and `pc` (LLDB builds without XML support, its Windows builds among them, ask for these instead of `target.xml`) |
| `qHostInfo`, `qProcessInfo` | LLDB's: triple `riscv32-unknown-unknown-elf`, little endian, 4-byte pointers |
| `k`, `vKill` | kills the machine, no reply |
| `D` | `OK`, then detaches |
| anything else | empty: not supported |

Stop replies:

| Stop | Reply |
|---|---|
| breakpoint or step | `T05thread:N;20:PC;` (the pc comes along, register 0x20) |
| watchpoint | `T05watch:ADDR;thread:N;20:PC;`, with `rwatch` or `awatch` for read and access watchpoints. ADDR is the first watched byte the access touched |
| interrupt, or a new client | `T02thread:N;20:PC;` |
| program exit | `W` and the exit code |

The stopping hart becomes the selected one.

Errors:

- A known packet with arguments it can't parse gets `E01`, and so does an unknown register or a CSR that can't be written.
- Memory that can't be read or written gets `E0e` (EFAULT, 14).
- A hart that doesn't exist, or an empty watch range, gets `E16` (EINVAL, 22).

Malformed packets never stop the stub. A bad checksum gets `-` and the client sends the packet again, and packets over 64 KiB are dropped as bad. `crates/grail/src/gdb` has unit tests for the framing and for every command, and `crates/grail/tests/gdb.rs` tests `grail --gdb` over TCP.

### With GDB or LLDB

GDB with RISC-V support (`gdb-multiarch`, `riscv64-unknown-elf-gdb`):

```
target remote 127.0.0.1:1234
```

LLDB from the official LLVM installer (`lldb`):

```
target create hello.elf
gdb-remote 127.0.0.1:1234
```

On Windows, LLDB needs the Python its build was made with on the PATH: `python310.dll` for LLVM 21. Without it, `lldb.exe` exits at once with "DLL not found". LLDB on Windows reads the general registers through `qRegisterInfo`, so it can't see the CSRs; GDB and merlin-dbg can. `cargo test -p grail --test gdb -- --ignored` runs LLDB against the stub (`LLDB=path` if it is not on the PATH) and checks it can break, step and read registers and memory.

## merlin-dbg

```
merlin-dbg [--connect ADDR] [-x FILE] [program.elf]
merlin-dbg [-x FILE] -- [grail options] program.elf
```

**`--connect ADDR`** connects to a running stub. ADDR is `HOST:PORT` or a port on 127.0.0.1, 127.0.0.1:1234 by default. `program.elf` is only read for its symbols. `quit` detaches and leaves the machine running.

**`-- ARGS`** starts Grail in merlin-dbg's own process, with the same options and program as the `grail` command. The machine is stopped at its first instruction:

- Its console output goes to merlin-dbg's stdout, mixed with the debugger's.
- It gets no console input, because stdin carries the commands.
- `quit` kills it.

**Commands** come from stdin, one per line. `-x FILE` runs a file of them first. Lines starting with `#` are comments. Commands that don't come from a terminal are echoed as `(merlin-dbg) command`, so a transcript reads like a session. The end of the input quits.

Ctrl-C stops a running machine and doesn't quit merlin-dbg. `quit` does.

| Command | What it does |
|---|---|
| `break ADDR`, `b` | breakpoint at ADDR |
| `watch [-r\|-a] ADDR [LEN]` | watchpoint on LEN bytes (4) at ADDR: writes, `-r` reads, `-a` either |
| `delete [N]`, `d` | deletes breakpoint or watchpoint N, or all of them |
| `info breakpoints`, `i b` | lists them |
| `continue`, `c` | runs every hart until something stops the machine |
| `stepi [N]`, `si`, `step`, `s` | N instructions (1) of the selected hart. At the machine level `step` is `stepi`: stepping by source line needs debug information |
| `nexti [N]`, `ni`, `next`, `n` | the same, but a call runs until it returns: a `jal`/`jalr` that links, or the `auipc ra` + `jalr ra` of a `call` |
| `finish` | runs until the current function returns to its caller (the backtrace's frame 1), and shows `a0` |
| `regs`, `info registers` | the selected hart's registers, pc and privilege mode |
| `x/N[w\|b] ADDR` | N words (default) or bytes of memory |
| `disas [ADDR] [N]` | N instructions (8) from ADDR (the pc), `=>` at the pc |
| `bt`, `backtrace` | the call stack |
| `hart`, `info harts` | every hart, its mode and pc. `*` marks the selected one |
| `hart N` | selects hart N |
| `quit`, `q` | leaves |

An ADDR is any of these, optionally followed by `+N` or `-N`:

- a number (`0x80000020`, `1234`);
- a symbol (`main`, `puts+8`);
- a register (`$sp`, `$a0`, `$x5`, `$pc`, `$mstatus`).

Symbols come from the ELF symbol table (through `elf32`). Constants and local labels are left out: absolute symbols such as `.equ` values, and `.L` labels. Addresses are shown as `0x80000024 <puts+4>`: the nearest symbol at or below the address, within the symbol's size when it has one.

Each stop shows its reason, then the instruction at the pc:

- `breakpoint N`;
- `watchpoint N (write) hit at ADDR`;
- `interrupted`;
- `program exited with code N`.

On a machine with several harts, the reason starts with `[hart N]`, and the stopping hart becomes the selected one.

`next` and `finish` set a temporary breakpoint at the return address and continue. A hit counts only if it is on the same hart, with a stack pointer no lower than it was. That way a recursive call returning to the same address doesn't end the run early. Any other stop on the way is reported and ends the command.

### Backtraces and the frame layout

merlin and clang (unless told to omit frame pointers) give every function that calls another the same kind of frame:

- `s0`, the frame pointer, holds the value `sp` had on entry: the canonical frame address (CFA).
- The prologue saves `ra` and the caller's `s0` just below the CFA.

merlin's prologue (`crates/merlin/src/codegen.rs`, "Stack frame"):

```
    addi  sp, sp, -16        sp = CFA - 16
    sw    ra, 12(sp)         ra        at CFA - 4
    sw    s0, 8(sp)          caller s0 at CFA - 8
    addi  s0, sp, 16         s0 = CFA
```

clang's prologue is the same with the frame's whole size in the first `addi`. In a variadic function, the saved argument registers come first, and merlin puts `ra` at CFA - 32 and `s0` at CFA - 36. So the unwinder doesn't assume where the slots are. It reads the prologue of the function holding each pc, from its symbol's address, for at most 16 instructions. It follows the `sp` adjustments to find where `ra` and `s0` are stored and what `s0` is set to.

From one frame's `s0`, the caller's pc and `s0` are the words in those slots. The walk follows the chain up and stops when any of these happens:

- the frame pointer is 0 (`_start` and the kernel's entry leave `s0` at its reset value 0), misaligned, not above the one before, or unreadable;
- a pc is 0;
- a function's prologue sets no `s0`;
- 64 frames have been shown.

The innermost frame is different, because its function may not have its frame:

- **Not built yet:** the pc is in the prologue, at or before `addi s0`.
- **Already taken down:** the pc is at the `ret`, or right after the `lw s0` that restores the caller's frame pointer.
- **No frame at all:** a leaf function.

In each case `ra` holds the return address and `s0` is still the caller's frame pointer. So frame 1 is `ra`, and the walk goes on from `s0`.

**Without frame pointers** (`-fomit-frame-pointer`, or hand-written assembly), `s0` is just another register. A function whose prologue doesn't set `s0` can't be unwound past, so the backtrace stops there. If it is the innermost frame, `ra` still gives frame 1.

**Trap handlers** are not calls either. A backtrace from inside one ends at the handler, or carries on with whatever `s0` the interrupted code had.

`crates/merlin-dbg/src/frames.rs` has the unwinder and its unit tests.

### Tests

`crates/merlin-dbg/tests/session.rs` drives the merlin-dbg binary against the stub, on `examples/hello`, `examples/smp` with 2 harts, and the kernel booted to its shell. The stub is `grail::gdb::serve`, what `grail --gdb` runs, in the test's process, so the test can capture the machine's console. The test checks:

- breakpoints by symbol, continues, registers, memory, steps, `next` over calls, `finish` and backtraces through the kernel's S-mode code with paging on;
- watchpoints and switching harts;
- errors that leave the session going;
- that each run's console output and exit code are those of the same program run without a debugger;
- `merlin-dbg -- ...` with a script (`-x`);
- interrupting a running machine.

## What's next

The next layers of merlin-dbg build on this one:

- source-level debugging with line tables (#119);
- variables and types from DWARF (#120);
- debugging Avalon VMs and individual Camelot processes (#121).
