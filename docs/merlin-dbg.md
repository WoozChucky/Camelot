# merlin-dbg and Grail's GDB stub

merlin-dbg is the Camelot debugger. This document covers its first two layers:

- **machine-level debugging** of any program running in Grail, RV32 or RV64, with no help from the compiler: breakpoints, watchpoints, single steps, registers, CSRs, memory, disassembly and backtraces by frame pointer, on one hart or several;
- **source-level debugging** of programs built with `merlin -g` (the kernel and its user programs are): breakpoints on a source line, stepping by line, the source shown at every stop, from the programs' DWARF line tables (see "Source lines" below).

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
(merlin-dbg) break *puts
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

An RV64 program (`examples/hello/hello-rv64.elf`) runs on an RV64 machine, and merlin-dbg shows the addresses of its frames and code with 16 digits, as GDB does, and the others without leading zeros:

```
(merlin-dbg) x/2g $a0
0x80000050 <message>:  0x7266206f6c6c6548  0x6c656d6143206d6f
(merlin-dbg) bt
#0  0x0000000080000024 <puts+4>
#1  0x0000000080000018 <_start+24>
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
- `Space::Virtual`: translated as the hart's instruction fetches are now. That means physical in M-mode or with `satp` off, through its page table otherwise (Sv39 or Sv48 on an RV64 hart, Sv32 on an RV32 one), with no permission checks, so a debugger can read a user page from S-mode.

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
| `g` / `G data` | `x0`-`x31` and `pc` of the selected hart, little-endian in hex, as wide as the machine's harts (4 bytes each on RV32, 8 on RV64) / writes as many of them as `data` holds |
| `p n` / `P n=value` | register `n`: 0-31 `x0`-`x31`, 32 `pc`, 65 + `a` CSR `a`, 4161 `priv` (0 U, 1 S, 3 M; writing it changes the mode), as wide as `g`'s (a value of another width is `E01`). GDB's own numbering, so a client agrees with it with or without the target description |
| `m addr,len` | the bytes that can be read from `addr` on, through the selected hart's translation, at most 4096. Addresses are up to 64 bits; on RV32, one past 4 GiB is unmapped |
| `M addr,len:hex`, `X addr,len:binary` | `OK` if all `len` bytes were written |
| `c [addr]`, `C sig[;addr]`, `s [addr]`, `S sig[;addr]` | resumes (at `addr` if given; signals are ignored). `c` replies when the machine stops, `s` after one instruction of the `Hc` hart |
| `vCont?` | `vCont;c;C;s;S` |
| `vCont;action[:thread]...` | a step (`s`/`S`) of its thread if there is one, else continue. Every hart runs either way: a step of one hart is a turn of each hart before it |
| `Z0`/`z0`, `Z1`/`z1 ,addr,kind` | sets/clears a breakpoint (software and hardware ones are the same thing here). Conditions after `;` are ignored. On RV32, a breakpoint at an address past 4 GiB, which no hart can reach, gets `E16` |
| `Z2`-`Z4` / `z2`-`z4 ,addr,len` | sets/clears a write, read or access watchpoint on `len` bytes (`E16` for an empty range or one past the end of the address space, 2^64 or, on RV32, 2^32) |
| `Hg thread`, `Hc thread` | selects the hart for registers and memory / for `c` and `s` |
| `T thread` | `OK` if the hart exists |
| `qfThreadInfo`, `qsThreadInfo`, `qC` | `m1,2,...`, `l`, `QC` and the selected thread |
| `qThreadExtraInfo,thread` | e.g. `hart 1, S-mode, in wfi`, hex encoded |
| `qAttached` | `1` |
| `qXfer:features:read:target.xml:off,len` | the target description: `riscv:rv32` or `riscv:rv64` with every register 32 or 64 bits wide (`bitsize`), the `org.gnu.gdb.riscv.cpu` registers, every CSR Grail implements that has a name for the machine's width (RV64 has no `cycleh`), and `priv` |
| `qRegisterInfoN` | LLDB's register description, for `x0`-`x31` and `pc`, as wide as `g`'s and at their offsets in it (LLDB builds without XML support, such as LLVM 21's Windows build, ask for these instead of `target.xml`) |
| `qHostInfo`, `qProcessInfo` | LLDB's: triple `riscv32-unknown-unknown-elf` and 4-byte pointers, or `riscv64-unknown-unknown-elf` and 8-byte pointers; little endian |
| `k`, `vKill` | `X09` (ended by SIGKILL), then kills the machine. GDB expects no reply to `k` and ignores it; LLDB waits for one |
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

On Windows, LLDB needs a Python runtime on the PATH, or `lldb.exe` exits at once:

- LLVM 23 loads `python3.dll`, which any Python 3 installation has.
- LLVM 21 wanted exactly `python310.dll`.

LLDB 23 reads the target description and sees the CSRs. LLDB builds without XML support (LLVM 21's Windows build) read the general registers through `qRegisterInfo` and can't see the CSRs. GDB and merlin-dbg always see them.

`cargo test -p grail --test gdb -- --ignored` runs LLDB against the stub (set `LLDB=path` if it is not on the PATH). It checks that LLDB can break, step, and read registers and memory. It passes with LLDB 21 and 23.

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

**The machine's width** (RV32 or RV64) comes from the stub, as GDB takes it: the target description's `<architecture>`, `riscv:rv32` or `riscv:rv64` (`qXfer:features:read:target.xml`). A stub without a description is asked for its registers instead, and the length of the `g` reply, 33 registers of 4 or 8 bytes, gives the width. The width decides how the code decodes and disassembles, how wide registers, addresses and saved frame slots are, and which CSR names exist (RV64 has no `cycleh`). A program must be of the machine's width, an ELF32 one for RV32 and ELF64 for RV64: the symbols of one of the other width would be wrong for the machine. With `--connect`, such a program is left out with a warning, and the session goes on without its symbols, as GDB warns and goes on (`warning: program.elf: an RV32 program (ELF32), but the machine is RV64: going on without its symbols`). Ending the session instead would drop the connection, which the stub takes as a detach, and a machine waiting for its debugger would run off on its own. `add-symbol-file` refuses one with the same error, and the session goes on. (With `--`, Grail makes the machine as wide as the program.) On RV32, address arithmetic wraps at 2^32 and an address past it is refused.

| Command | What it does |
|---|---|
| `break FUNC`, `b` | breakpoint on a function past its prologue, as GDB's `break FUNC`: see "Breakpoints on functions" |
| `break ADDR`, `break *ADDR` | breakpoint at ADDR exactly (a `*` before a function's name, as in GDB, means its first instruction) |
| `break FILE:LINE` | breakpoint on a source line (`kernel/main.c:273`, or `main.c:273`): see "Source lines" |
| `watch [-r\|-a] ADDR [LEN]` | watchpoint on LEN bytes at ADDR: writes, `-r` reads, `-a` either. Without LEN, as GDB watches as many bytes as an expression's type has: the size of the symbol ADDR names in the symbol table (a C variable's, `watch total` on a `long` watches 8 bytes), at most 8, or else a register's worth (4 bytes on RV32, 8 on RV64), for an assembly label or an address |
| `delete [N]`, `d` | deletes breakpoint or watchpoint N, or all of them |
| `info breakpoints`, `i b` | lists them |
| `continue`, `c` | runs every hart until something stops the machine |
| `step [N]`, `s` | N source lines (1) of the selected hart, into calls. Without line information at the pc, `stepi` |
| `next [N]`, `n` | the same, but a call runs until it returns. Without line information, `nexti` |
| `stepi [N]`, `si` | N instructions (1) of the selected hart |
| `nexti [N]`, `ni` | the same, but a call runs until it returns: a `jal`/`jalr` that links, or the `auipc ra` + `jalr ra` of a `call` |
| `finish` | runs until the current function returns to its caller (the backtrace's frame 1), and shows `a0` |
| `regs`, `info registers` | the selected hart's registers, pc and privilege mode, in GDB's `info registers` layout (see below) |
| `x/N[b\|h\|w\|g] ADDR` | N units of memory, in hex: GDB's unit letters, bytes (`b`, 8 to a line), halfwords (`h`, 8), words (`w`, 4, the default) or giants (`g`, 8 bytes, 2 to a line), at either width. An `x` (hex, the only format) may come before or after the letter |
| `disas [ADDR] [N]` | N instructions (8) from ADDR (the pc), `=>` at the pc |
| `bt`, `backtrace` | the call stack, with each frame's source line |
| `list [FILE:]LINE`, `list FUNC`, `l` | ten source lines around LINE (of the file listed last) or a function's first line; `list` alone goes on after the lines listed last, or shows those around the stop |
| `directory DIR` | looks for source files under DIR first |
| `add-symbol-file FILE` | adds the symbols and line tables of another program: a user program next to the kernel |
| `hart`, `info harts` | every hart, its mode and pc. `*` marks the selected one |
| `hart N` | selects hart N |
| `quit`, `q` | leaves |

**`regs`** shows the registers as GDB's `info registers` does for RISC-V, one to a line, so a line fits 80 columns at either width: the name, padded to 15 columns, the value in hex without leading zeros, a tab, and the value as its type in GDB's target description shows it. `ra` and `pc` are code pointers, shown with their symbol; `sp`, `gp`, `tp` and `fp` (`s0`, as GDB names it) are data pointers, in hex; the others are integers, signed at the register's width. `zero` is left out, as GDB leaves it out. `priv`, the privilege mode, comes last, as GDB's `info registers priv` shows it:

```
ra             0x80000018	0x80000018 <_start+24>
sp             0x80010070	0x80010070
...
t0             0x80000000	-2147483648
...
a0             0x80000050	-2147483568
...
pc             0x80000020	0x80000020 <puts>
priv           0x3	prv:3 [Machine]
```

On RV64 the same `t0` is `2147483648`, a positive 64-bit number.

An ADDR is any of these, optionally followed by `+N` or `-N`:

- a number (`0x80000020`, `1234`);
- a symbol (`main`, `puts+8`);
- a register (`$sp`, `$a0`, `$x5`, `$pc`, `$mstatus`).

Symbols come from the ELF symbol table (through `elf`). Constants and local labels are left out: absolute symbols such as `.equ` values, and `.L` labels. Addresses are shown as `0x80000024 <puts+4>`: the nearest symbol at or below the address, within the symbol's size when it has one, and within its section when it has none (an assembly label), as GDB's minimal symbols are: the stack past the program's last section has no name. Where GDB pads an address with zeros to the target's width, so does merlin-dbg: in backtraces (`bt`), disassembly (`disas` and the `=>` line of each stop), `finish`'s frame, the harts' pcs, and `info breakpoints`' breakpoint addresses (GDB's Address column). That is 8 digits on RV32 and 16 on RV64: `0x0000000080000024 <puts+4>`. Everywhere else, as GDB, an address has no leading zeros: `x`'s line prefixes, `breakpoint N at ADDR`, watchpoints as they are set, hit and listed (GDB shows a watchpoint's expression), `regs`' code pointers, and a read that fails (`(cannot read past 0x80400000)`). So on RV32 only addresses below 0x10000000 look different between the two, such as a user program's `breakpoint 1 at 0x3bc4 <main>`. A branch's target in an instruction is as merlin-objdump and llvm-objdump write it (`beqz t2, 0x80000048 <puts+40>`).

**Breakpoints on functions.** As in GDB, `break FUNC` stops past the function's prologue, where its frame is built and its arguments are in their places, and `break *FUNC` at its first instruction. FUNC is a symbol in code (a C function, or an assembly label in `.text`); any other ADDR, a data symbol, a number or `FUNC+N`, is an exact address. Past the prologue is where GDB puts it (`skip_prologue_sal`, then `riscv_skip_prologue`):

- with line information: at the function's row marked `prologue_end` (as GDB 13 and later), else at its second row, the end of its first line (`skip_prologue_using_sal`). kmain's breakpoint is at its first statement, `kernel/main.c:275`, not on line 273, its `{`;
- without (an assembly function): by GDB's RISC-V prologue analysis (`riscv_scan_prologue`), which counts as prologue `addi` and `add` with any registers, `addiw sp, sp` and `addw s0, sp, zero`, `sw` and `sd` to `sp` or `s0`, `lui`, `auipc`, `lw` and `ld`, and ends at the first instruction of another kind, or after 100 bytes. hello's `puts` (`li t0, UART_BASE`, `mv t1, a0`, then `lbu`) is `puts+8`, the top of its loop, which every character passes: `break *puts` stops once.

Each stop shows its reason, then the instruction at the pc, then its source line if the line tables have one (`kernel/main.c:275:     cpu_init();`, the text left out when the file cannot be found):

- `breakpoint N`;
- `watchpoint N (write) hit at ADDR`;
- `interrupted`;
- `program exited with code N`.

A `stepi` on the line shown last does not show it again.

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

On RV64 (LP64) the slots are doublewords, saved with `sd` and restored with `ld`, as clang lays them out too:

```
    addi  sp, sp, -16        sp = CFA - 16
    sd    ra, 8(sp)          ra        at CFA - 8
    sd    s0, 0(sp)          caller s0 at CFA - 16
    addi  s0, sp, 16         s0 = CFA
```

clang's prologue is the same with the frame's whole size in the first `addi`. In a variadic function, the saved argument registers come first. merlin saves the `8 - k` argument registers that the named parameters leave free (they take `k` argument words, at most 8) right below the CFA, and `ra` and `s0` below them. With `W` the bytes of a register (4 on RV32, 8 on RV64), `ra` is at CFA - W·(9 - k) and `s0` at CFA - W·(10 - k): with one named parameter, as kprintf, CFA - 32 and CFA - 36 on RV32, CFA - 64 and CFA - 72 on RV64 (`crates/merlin/src/codegen.rs`, "Variadic functions"). So the unwinder doesn't assume where the slots are. It reads the prologue of the function holding each pc, from its symbol's address, for at most 16 instructions. It follows the `sp` adjustments to find where `ra` and `s0` are stored, by a store as wide as a register (`sw` on RV32, `sd` on RV64: a `sw ra` on RV64 would keep half of it), and what `s0` is set to. Instructions are read a word at a time at both widths; the slots are read at the machine's width.

From one frame's `s0`, the caller's pc and `s0` are the values in those slots. The walk follows the chain up and stops when any of these happens:

- the frame pointer is 0 (`_start` and the kernel's entry leave `s0` at its reset value 0), not 16-byte aligned (a CFA is the `sp` of a call, which the psABI keeps 16-byte aligned on RV32 and RV64), not above the one before, or unreadable;
- a pc is 0;
- a function's prologue sets no `s0`;
- 64 frames have been shown.

The innermost frame is different, because its function may not have its frame:

- **Not built yet:** the pc is in the prologue, at or before `addi s0`.
- **Already taken down:** the pc is at the `ret`, or right after the `lw s0` (`ld s0`) that restores the caller's frame pointer.
- **No frame at all:** a leaf function.

In each case `ra` holds the return address and `s0` is still the caller's frame pointer. So frame 1 is `ra`, and the walk goes on from `s0`.

**Without frame pointers** (`-fomit-frame-pointer`, or hand-written assembly), `s0` is just another register. A function whose prologue doesn't set `s0` can't be unwound past, so the backtrace stops there. If it is the innermost frame, `ra` still gives frame 1.

**Trap handlers** are not calls either. A backtrace from inside one ends at the handler, or carries on with whatever `s0` the interrupted code had.

`crates/merlin-dbg/src/frames.rs` has the unwinder and its unit tests. They read prologues made of merlin's instruction words, and one of them builds `tests/rv64/prog.c` with merlin for RV32 and RV64 and reads the real `sum` (variadic) and `fact`, so that those words are merlin's own.

### Source lines

The line tables (`.debug_line`, the spec's section 10, "Debug information") map each address to a file and a line. merlin-dbg reads them from the program and from each `add-symbol-file` (`crates/merlin-dbg/src/source.rs`, through `elf::lines`). A row marked `is_stmt` starts a statement: merlin writes one before each statement, a function's first line before its prologue, a loop's condition and step, and the closing brace before the epilogue, and marks the first statement after the prologue `prologue_end`.

- **`break FILE:LINE`**: FILE is the path the line table has (`kernel/main.c`) or its end after a `/` (`main.c`). The breakpoint goes at the lowest address of a statement on that line in each function that has one (a `for` line has several statements: its initialization first). A line with no code, such as a comment or a declaration without an initializer, means the next line that has some, as in gdb, and merlin-dbg says so. A function's first line is its prologue, so a breakpoint there goes past the prologue, as `break FUNC` does (and as GDB's `skip_prologue_sal` does with a line in a function's prologue); a line further in stays where it is.
- **`step` and `next`** run instructions until the pc is at the start of a statement on another line. A call on the way runs to its return under `next`, a temporary breakpoint as `nexti` uses. `step` follows it into the function, and stops past its prologue, at its `prologue_end` row (or its second row). A function with no line information, an assembly one, is run to its return instead, as gdb does. A return stops in the caller, on the line of the call, wherever in it the pc is. Any other stop on the way (a breakpoint, a watchpoint, another hart's) ends the command, and Ctrl-C does too. A line that never ends (`for (;;);`) is given up after a million instructions.
- **Sources** are found by their paths in the line tables, under a `directory` first, then relative to the current directory. The kernel's are relative to the repository root (`kernel/main.c`, `kernel/user/sh.c`), so run merlin-dbg there.
- **User programs**: the kernel embeds its programs without their line tables, and `/bin` holds them the same way; `cargo xtask kernel` writes them with their line tables to `target/kernel/obj/user/<name>.elf`. Every program runs at the same virtual addresses, so a breakpoint in one stops whichever process runs that address; telling processes apart is #121.

```
$ cargo run -p merlin-dbg -- -- kernel/kernel.elf
(merlin-dbg) break kernel/main.c:273
breakpoint 1 at 0x8001ccb4 <kmain+16>: kernel/main.c:275
(merlin-dbg) continue
breakpoint 1
=> 0x8001ccb4 <kmain+16>:  auipc ra, 0x23
kernel/main.c:275:     cpu_init();
(merlin-dbg) next
=> 0x8001ccbc <kmain+24>:  auipc ra, 0x2a
kernel/main.c:276:     uart_init();
(merlin-dbg) step
...
(merlin-dbg) add-symbol-file target/kernel/obj/user/sh.elf
(merlin-dbg) break kernel/user/sh.c:502
```

LLDB reads the same tables: `image lookup -a ADDR` gives the line, `breakpoint set -f main.c -l LINE` the address (`cargo test -p merlin-dbg --test session -- --ignored`).

### Tests

`crates/merlin-dbg/tests/session.rs` drives the merlin-dbg binary against the stub, on `examples/hello`, `examples/smp` with 2 harts, and the kernel booted to its shell. The stub is `grail::gdb::serve`, what `grail --gdb` runs, in the test's process, so the test can capture the machine's console. The test checks:

- breakpoints by symbol, continues, registers, memory, steps, `next` over calls, `finish` and backtraces through the kernel's S-mode code with paging on;
- watchpoints and switching harts;
- errors that leave the session going;
- that each run's console output and exit code are those of the same program run without a debugger;
- `merlin-dbg -- ...` with a script (`-x`);
- interrupting a running machine;
- source lines: a breakpoint on `kmain`'s line of `kernel/main.c` and on a line with no code, `next` and `step` by line, `list`, the lines in a backtrace, and the same in the shell, a user program added with `add-symbol-file`; LLDB on the kernel's line tables (ignored).

The same on RV64 machines, against the stub's `riscv:rv64` description:

- `examples/hello/hello-rv64.elf`: frames and code addresses with 16 digits, the other addresses and `regs` in GDB's layout without leading zeros, `x/b`, `x/h`, `x/w` and `x/g`, steps, a backtrace, disassembly and `finish`;
- `examples/smp/smp-rv64.elf` with 2 harts: harts and watchpoints;
- `crates/merlin-dbg/tests/rv64/prog.c`, built by the test with `merlin -g` for RV64, merlin-as and merlin-ld: `break FILE:LINE`, `next` and `step` by line, `list`, a backtrace through a variadic function from inside it, past its prologue (its own frame read from memory: its `ra` and `s0` at CFA - 64 and CFA - 72), and five frames of a recursive one, `finish`, and a watchpoint on a 64-bit global;
- `merlin-dbg -- ...` on an RV64 program, and an RV32 program's symbols left out for an RV64 machine: the session goes on, the machine stopped at its first instruction until it is run to a breakpoint.

- `kernel/kernel-rv64.elf` (#154), the higher-half kernel: `break kmain` and `break kvminit` at their link addresses (0xffff_ffff_8...), stopped in S-mode with Sv48 on, `regs`, and a backtrace of three frames read through the page table, down to `supervisor_entry`.

`src/remote.rs`'s unit tests give it a stub with no target description, whose width comes from the `g` reply.

## What's next

The next layers of merlin-dbg build on this one:

- variables and types from DWARF (#120);
- debugging Avalon VMs and individual Camelot processes (#121).
