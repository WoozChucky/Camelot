# Excalibur ISA Spec v0 (draft)

Part of **Camelot**: the Excalibur ISA, the Grail emulator, the Merlin toolchain (merlin-as, merlin-ld, merlin), and the Camelot OS.

Status: draft. Base is stock RISC-V, plus the Xcal extensions in section 9.

## 1. Overview

- 32-bit, little-endian, load/store architecture.
- Base ISA: **RV32IMA + Zicsr + Zifencei**: RV32I (unprivileged spec 20191213), **M** (multiply/divide) and **A** (atomics: LR/SC and AMOs, section 4.1).
- Planned extensions: **C** optional and late.
- Privilege: **M-mode** and **S-mode**, plus **U-mode** for user programs. Follows the RISC-V privileged spec (20211203) subset described below.
- Goal: a stock RV32I program built with `riscv32i-unknown-none-elf` runs unmodified. Custom features (Excalibur extensions, prefixed `xcal`, e.g. `Xcaldbg`) are additive and never change the meaning of standard encodings.

## 2. Registers

| Name | ABI | Role |
|---|---|---|
| x0 | zero | always 0, writes ignored |
| x1 | ra | return address |
| x2 | sp | stack pointer |
| x3 | gp | global pointer |
| x4 | tp | thread pointer |
| x5-x7 | t0-t2 | temporaries |
| x8 | s0/fp | saved / frame pointer |
| x9 | s1 | saved |
| x10-x11 | a0-a1 | args / return values |
| x12-x17 | a2-a7 | args |
| x18-x27 | s2-s11 | saved |
| x28-x31 | t3-t6 | temporaries |

`pc` is a separate 32-bit register. Instruction fetch requires 4-byte alignment (2-byte if C is enabled later).

## 3. Instruction formats

All instructions are 32 bits. `opcode` is bits [6:0].

```
R: funct7[31:25] rs2[24:20] rs1[19:15] funct3[14:12] rd[11:7]  opcode
I: imm[11:0][31:20]         rs1        funct3        rd        opcode
S: imm[11:5]     rs2        rs1        funct3        imm[4:0]  opcode
B: imm[12|10:5]  rs2        rs1        funct3        imm[4:1|11] opcode
U: imm[31:12]                                        rd        opcode
J: imm[20|10:1|11|19:12]                             rd        opcode
```

Immediates are sign-extended from their top bit. B and J immediates are multiples of 2.

## 4. RV32I instruction list

| Opcode | Instructions |
|---|---|
| 0110111 LUI | `lui` |
| 0010111 AUIPC | `auipc` |
| 1101111 JAL | `jal` |
| 1100111 JALR | `jalr` (target low bit cleared) |
| 1100011 BRANCH | `beq bne blt bge bltu bgeu` |
| 0000011 LOAD | `lb lh lw lbu lhu` |
| 0100011 STORE | `sb sh sw` |
| 0010011 OP-IMM | `addi slti sltiu xori ori andi slli srli srai` |
| 0110011 OP | `add sub sll slt sltu xor srl sra or and` (+ M: `mul mulh mulhsu mulhu div divu rem remu`, funct7=0000001) |
| 0101111 AMO | A, funct3=010 (word): `lr.w sc.w amoswap.w amoadd.w amoxor.w amoand.w amoor.w amomin.w amomax.w amominu.w amomaxu.w`, each with `.aq`/`.rl`/`.aqrl` (section 4.1) |
| 0001111 MISC-MEM | `fence`, `fence.tso`, `fence.i` (reserved `fm`/rd/rs1 encodings execute as a plain `fence`; Grail is single-hart, so all are no-ops) |
| 1110011 SYSTEM | `ecall ebreak mret sret wfi sfence.vma csrrw csrrs csrrc csrrwi csrrsi csrrci` |
| 0001011 custom-0 | Xcal: `xcal.dbg`, `xcal.halt` (section 9) |
| 0101011 custom-1 | reserved for future Xcal extensions |

Misaligned loads/stores trap (address-misaligned exception) rather than being emulated. This keeps the emulator simple and is spec-legal. Consequence: riscv-tests `rv32ui-p-ma_data`, which assumes hardware misaligned support, is excluded.

### 4.1 A extension (atomics)

All A instructions are R-type on the AMO opcode with funct3 = 010 (32-bit word; the `.d` forms are RV64 only). funct7 is split: funct5 in bits [31:27] selects the instruction, bit 26 is `aq` and bit 25 is `rl`:

```
funct5[31:27] aq[26] rl[25] rs2[24:20] rs1[19:15] 010 rd[11:7] 0101111
```

| funct5 | Instruction | Effect (`M` = the word at address rs1) |
|---|---|---|
| 00010 | `lr.w rd, (rs1)` | rd = M; reserve M. rs2 must be 0 (else reserved: illegal instruction) |
| 00011 | `sc.w rd, rs2, (rs1)` | if M is reserved: M = rs2, rd = 0; else rd = 1. The reservation is gone either way |
| 00001 | `amoswap.w rd, rs2, (rs1)` | rd = M; M = rs2 |
| 00000 | `amoadd.w` | rd = M; M = M + rs2 |
| 00100 | `amoxor.w` | rd = M; M = M ^ rs2 |
| 01100 | `amoand.w` | rd = M; M = M & rs2 |
| 01000 | `amoor.w` | rd = M; M = M \| rs2 |
| 10000 / 10100 | `amomin.w` / `amomax.w` | rd = M; M = signed min / max(M, rs2) |
| 11000 / 11100 | `amominu.w` / `amomaxu.w` | rd = M; M = unsigned min / max(M, rs2) |

Other funct5 values (`amocas.w` of Zacas, ...) are reserved and raise illegal-instruction. Assembly writes the ordering bits as a suffix: none, `.aq`, `.rl` or `.aqrl` (`amoadd.w.aqrl`). The address is written `(rs1)`, or `0(rs1)`; there is no offset field. Grail runs one hart, one instruction at a time, so an AMO is atomic by construction and `aq`/`rl` change nothing; they are still encoded, decoded and disassembled.

Addresses (checked in this order):

- **Alignment**: the address must be 4-byte aligned. A misaligned `lr.w` raises load address misaligned (4); a misaligned `sc.w` (even one that would fail) or AMO raises store/AMO address misaligned (6). `xtval` = the address.
- **Translation** (Sv32): `lr.w` is checked and sets A like a load (load page fault, 13); `sc.w` and the AMOs like a store: the page must be writable, A and D are set, and a fault is a store/AMO page fault (15).
- **Memory only**: atomics work on RAM and VRAM. On any other physical address (device registers, the boot ROM, unmapped space) `lr.w` raises a load access fault (5), `sc.w` and the AMOs a store/AMO access fault (7). Device registers are not read or written.

**Reservations.** Each hart has one reservation set: the naturally aligned word `lr.w` read, by physical address (so two virtual mappings of a word share it). A new `lr.w` replaces it. `sc.w` succeeds only if the hart holds a reservation on the word it writes. A `sc.w` with no reservation at all fails without accessing memory: no translation, no page fault, no A/D update (it still traps if misaligned). With a reservation, the address is translated and checked first (faults as above, A/D set), then compared. The reservation is lost when:

- the hart executes any `sc.w`;
- the hart takes a trap (exception or interrupt);
- any write touches a byte of the reserved word: a store, an AMO, a successful `sc.w`, or a hardware A/D update of a PTE (so an LR/SC update of a PTE cannot overwrite a bit the page walker just set). On a multi-hart machine this includes writes by other harts;
- the hart stores to a device register: the store may start a DMA (the block device's, section 9.6) that writes RAM without a hart storing there.

The constrained LR/SC loop (an `lr.w`, a few ALU instructions and a `sc.w` to the same word) always succeeds on Grail unless an interrupt arrives in between, in which case the loop retries.

## 5. Privilege and CSRs

Modes: U=0, S=1, M=3. Reset enters M-mode.

Minimum CSR set:

| CSR | Addr | Notes |
|---|---|---|
| mstatus | 0x300 | MIE, MPIE, MPP, SIE, SPIE, SPP, SUM, MXR |
| misa | 0x301 | RV32, A, I, M, S, U, X: 0x4094_1101 (read-only) |
| medeleg / mideleg | 0x302 / 0x303 | delegate traps to S-mode (section 6, "Delegation") |
| mie / mip | 0x304 / 0x344 | MSIE/MTIE/MEIE, SSIE/STIE/SEIE |
| mtvec | 0x305 | direct mode required, vectored optional |
| mscratch | 0x340 | |
| mepc / mcause / mtval | 0x341 / 0x342 / 0x343 | |
| sstatus, sie, sip | 0x100, 0x104, 0x144 | restricted views of the M versions |
| stvec, sscratch, sepc, scause, stval | 0x105, 0x140, 0x141, 0x142, 0x143 | |
| satp | 0x180 | Sv32 paging, hardware-managed A/D bits, no TLB |
| mhartid | 0xF14 | always 0 (single hart for now) |
| cycle/time/instret (+h) | 0xC00-0xC02, 0xC80-0xC82 | read-only counters |

Accessing a CSR above the current privilege level, or writing a read-only CSR, raises illegal-instruction.

## 6. Traps

On a trap taken into mode X (M, or S if delegated and current mode is not M):

1. `xepc` = pc of the faulting instruction (or of the next instruction for interrupts).
2. `xcause` = cause code, top bit set for interrupts.
3. `xtval` = faulting address or instruction bits, else 0.
4. `xPIE` = `xIE`, `xIE` = 0, `xPP` = previous mode.
5. Mode = X, pc = `xtvec` base (vectored: base + 4*cause for interrupts).

`mret`/`sret` reverse this.

Exception causes implemented: 0 instr misaligned, 1 instr access fault, 2 illegal instruction, 3 breakpoint, 4 load misaligned, 5 load access fault, 6 store/AMO misaligned, 7 store/AMO access fault, 8 ecall from U, 9 ecall from S, 11 ecall from M, 12/13/15 instruction/load/store-AMO page faults (with Sv32).

Interrupt causes: 1 SSI, 3 MSI, 5 STI, 7 MTI, 9 SEI, 11 MEI.

Interrupts are checked between instructions, priority MEI > MSI > MTI > SEI > SSI > STI.

Delegation: a trap is taken in S-mode when its bit is set in `medeleg` (exceptions) or `mideleg` (interrupts) and the hart is in S- or U-mode; a trap in M-mode always stays in M-mode. The writable `medeleg` bits are those of every implemented exception except ecall from M (mask 0xB3FF: causes 0-9, 12, 13, 15); the writable `mideleg` bits are SSI, STI and SEI (mask 0x222). M-level interrupts (MSI, MTI, MEI) cannot be delegated. An interrupt is taken in M-mode when it is pending, enabled in `mie`, not delegated, and either the hart is below M-mode or `mstatus.MIE` is set; a delegated one when the hart is in U-mode, or in S-mode with `mstatus.SIE` set. So while S-mode runs, the machine timer interrupts it regardless of `mstatus.MIE`. There is no Sstc extension (`stimecmp`): an S-mode kernel gets timer ticks from M-mode software that takes MTI, rearms `mtimecmp` and raises SSIP or STIP in `mip` (the Camelot kernel does this, section 10).

Pending bits in `mip` come from the devices: MSIP and MTIP from the CLINT, MEIP from PLIC context 0, SEIP from PLIC context 1 (section 7.1). MEIP, MSIP and MTIP are read-only. SEIP is also software-writable: the value read is the OR of the software bit and the PLIC's signal, and `csrrs`/`csrrc` on `mip` modify the software bit only, so a read-modify-write never latches the PLIC's signal into it (as in the RISC-V privileged spec). STIP and SSIP are software-written.

## 7. Memory map

Layout follows the QEMU `virt` board's conventions so existing docs and examples apply, without needing QEMU.

| Range | Device |
|---|---|
| 0x0000_1000 - 0x0000_1FFF | Boot ROM (reset vector 0x1000, jumps to 0x8000_0000) |
| 0x0200_0000 - 0x0200_FFFF | CLINT: msip @+0x0, mtimecmp @+0x4000, mtime @+0xBFF8 |
| 0x0C00_0000 - 0x0FFF_FFFF | PLIC: external interrupts from the UART, keyboard, disks, pointer and network cards (section 7.1) |
| 0x1000_0000 - 0x1000_00FF | UART, 16550-compatible subset (THR/RBR @+0, IER @+1, IIR @+2, LSR @+5) |
| 0x1000_1000 - 0x1000_8FFF | Block devices: disk `i` at 0x1000_1000 + 0x1000·i, up to 8 (`--disk`; Xcal, section 9.6). Only the pages of attached disks are mapped |
| 0x1000_9000 - 0x1000_CFFF | Network cards: card `i` at 0x1000_9000 + 0x1000·i, up to 4 (`--net`; Xcal, section 9.8). Only the pages of attached cards are mapped |
| 0x0010_0000 | Test/exit device: write 0x5555 = pass/exit, 0x3333 = fail (used by riscv-tests) |
| 0x3000_0000 - 0x3000_0FFF | Framebuffer control (Xcal, section 9.3) |
| 0x3000_1000 - 0x3000_1FFF | Keyboard (Xcal, section 9.4) |
| 0x3000_2000 - 0x3000_2FFF | Sysinfo: RAM size, clock, device table (Xcal, section 9.5) |
| 0x3000_3000 - 0x3000_3FFF | Pointer: mouse/tablet (Xcal, section 9.7) |
| 0x3100_0000 - 0x3112_BFFF | Framebuffer VRAM |
| 0x8000_0000 - 0x87FF_FFFF | RAM, 128 MiB default (`--ram`, 1-2048 MiB; software reads the size from sysinfo) |

Unmapped accesses raise access-fault exceptions. Instructions and page tables live only in RAM and the boot ROM: an instruction fetch from VRAM or from any other range raises an instruction access fault, and a Sv32 page-table walk that reads a PTE outside them raises an access fault of the original access type. So neither can trigger a device register's read side effect (such as a PLIC claim).

RAM: `--ram MiB` takes a whole number from 1 to 2048, the most that fits between 0x8000_0000 and the top of the 32-bit address space; anything else is rejected with an error. (As a library, Grail accepts any non-zero multiple of 4 KiB up to 2 GiB.) Software must not assume a size: it reads `SYS_RAM_SIZE` (section 9.5).

Clock: one instruction is one cycle is one `mtime` tick, so the CPU clock and the timebase frequency are the same number. Both are reported in Hz by sysinfo (`SYS_CPU_HZ`, `SYS_TIMEBASE_HZ`), so software computes timer intervals from them (a 10 ms tick is `hz / 100` ticks) instead of hard-coding a rate. Two modes:

- **Default (no `--clock`)**: Grail runs as fast as it can and the run is deterministic and reproducible. It still reports a nominal clock of 10 MHz, so software computes intervals the same way in both modes.
- **Real time (`--clock FREQ`)**: Grail paces execution to FREQ instructions per wall-clock second, and `SYS_FLAGS` bit 0 is set. FREQ is a number with an optional unit, case-insensitive: `Hz`, `kHz` or `MHz` (`--clock 10MHz`, `--clock 500kHz`, `--clock 5Hz`); a bare number is Hz. Fractions are allowed when the result is a whole number of Hz (`--clock 1.5MHz`, not `--clock 1.5Hz`); 0 and values above 2^32 - 1 Hz are rejected. Pacing works in batches of about 1 ms of guest time, sleeping whenever the machine is ahead of the wall clock, not per instruction. If the host falls more than ~100 ms behind (it is slower than FREQ, or the machine was paused), the lag is dropped rather than caught up at full speed.

Timer: `mtime` increments once per step (an executed instruction, a taken interrupt, or a tick spent waiting in `wfi`). MTIP is pending while `mtime >= mtimecmp`. During `wfi` with nothing pending, the default mode jumps `mtime` forward to `mtimecmp` instead of spinning; in real-time mode the wait lasts the real time until `mtimecmp` (`mtime` keeps ticking at FREQ; Grail skips the idle ticks of each batch at once and sleeps), and device input such as a UART byte still wakes the hart within about one batch.

UART: writing THR sends the byte to the machine's console, which for the `grail` command is the host terminal (stdout); RBR is fed from the console's input (host stdin). Each machine has its own console, so a host can run several machines in one process. LSR bit 5 (THRE) always 1, bit 0 (DR) set when input is available. IER bit 0 (ERBFI) enables the "received data available" interrupt: the UART holds PLIC source 1 high while IER bit 0 and LSR.DR are both set, so reading RBR until DR clears lowers it. IIR (@+2, read) reports 0x04 while that interrupt is asserted, else 0x01 (none). The other IER bits are stored but raise nothing (no transmit, line-status or modem interrupts).

### 7.1 PLIC

A minimal RISC-V PLIC (riscv-plic-spec) with the QEMU `virt` register layout: one hart, two contexts, sources 1-31.

| Source | Device |
|---|---|
| 0 | none ("no interrupt" when claimed) |
| 1 | UART receive (section 7) |
| 2 | Keyboard (section 9.4) |
| 3-10 | Disks 0-7: disk `i` is source 3 + `i` (section 9.6) |
| 11 | Pointer (section 9.7) |
| 12-15 | Network cards 0-3: card `i` is source 12 + `i` (section 9.8) |
| 16-31 | unused, never asserted |

QEMU `virt` puts UART0 on source 10; Excalibur numbers its few devices from 1 instead, so drivers must take the source numbers from this table rather than from `virt` device trees.

| Offset | Register |
|---|---|
| +0x00_0000 + 4·src | Priority of source `src`, 0-7 (3 bits, WARL). 0 = never interrupts. Source 0's is hard-wired to 0 |
| +0x00_1000 | Pending bits, sources 0-31 (read-only) |
| +0x00_2000 + 0x80·ctx | Enable bits of context `ctx`, sources 0-31 (bit 0 hard-wired to 0) |
| +0x20_0000 + 0x1000·ctx | Priority threshold of context `ctx`, 0-7 |
| +0x20_0004 + 0x1000·ctx | Claim (read) / complete (write) of context `ctx` |

Context 0 is hart 0 in M-mode and drives `mip.MEIP`; context 1 is hart 0 in S-mode and drives `mip.SEIP`. Registers are 32-bit: other access widths raise access faults. Every other offset in the range is reserved (reads 0, writes ignored).

Behavior:

- **Gateways** are level-triggered. Grail samples every device's line once per instruction, before checking for interrupts. A high line makes its source pending unless the source is in flight (claimed, not completed). Pending stays set until claimed, even if the line drops first.
- **Notification**: a context's interrupt line is high while some source is pending, enabled for it, and has priority strictly above its threshold.
- **Claim** returns the highest-priority such source (ties go to the lowest source number), clears its pending bit and marks it in flight. With nothing to claim it returns 0.
- **Complete** (write the source number) ends the claim. Writes naming a source that is not enabled for that context are ignored. If the device still holds its line high at the next sample, the source is pending again, so a driver may handle one event per interrupt.

Typical handler: claim, service the device until it lowers its line (for the UART, read RBR until LSR.DR is clear, or read one byte and let it re-raise), complete, return.

## 8. Emulator conformance

- Pass `riscv-tests` rv32ui-p-*, rv32um-p-*, rv32ua-p-*, rv32mi-p-* and rv32si-p-* (all but `rv32ui-p-ma_data`, section 4). Tests are loaded as ELF and report via the test device (or `tohost` symbol).
- Prebuilt test ELFs are committed to the repo so no cross toolchain is needed on Windows.

## 9. Excalibur extensions (Xcal)

Everything here lives in RISC-V's reserved custom space (custom-0 opcode, new MMIO ranges), so stock RV32 software keeps working. `misa` bit 23 (X, "non-standard extensions present") is set.

### 9.1 Xcaldbg: debug print

```
xcal.dbg rs1        custom-0 (0001011), funct3=000, rd=0, imm=0
encoding: 0000_0000_0000 | rs1 | 000 | 00000 | 0001011
```

Prints the register to the machine's debug console (stderr for the `grail` command), e.g. `[dbg] pc=80000010 a0 = 0x0000002a (42)`. No architectural side effects. Legal in every mode. Meant for the first days of the kernel, before the UART driver exists.

### 9.2 Xcalhalt: stop the machine

```
xcal.halt rs1       custom-0 (0001011), funct3=001, rd=0, imm=0
encoding: 0000_0000_0000 | rs1 | 001 | 00000 | 0001011
```

Stops the emulator; the host process exits with the low 8 bits of `rs1` as its exit code (0 = success). M-mode only: in S/U it raises illegal-instruction, so user programs can't power off the machine. (The test device at 0x0010_0000 remains for compatibility with riscv-tests.)

Any other custom-0 encoding raises illegal-instruction.

### 9.3 Framebuffer (MMIO)

| Address | Register | Notes |
|---|---|---|
| 0x3000_0000 | `FB_WIDTH` (RO) | 640 |
| 0x3000_0004 | `FB_HEIGHT` (RO) | 480 |
| 0x3000_0008 | `FB_STRIDE` (RO) | bytes per row, 2560 |
| 0x3000_000C | `FB_FORMAT` (RO) | 0 = XRGB8888 (little-endian word: 0x00RRGGBB) |
| 0x3000_0010 | `FB_CTRL` | bit 0 = display enable |
| 0x3000_0014 | `FB_PRESENT` (WO) | write anything: host redraws the window now |
| 0x3100_0000 - 0x3112_BFFF | VRAM | 640×480×4 bytes, plain memory |

Grail also redraws at ~60 Hz on its own, so `FB_PRESENT` is optional. Host side: a window opened through raw Win32 (GDI) calls via `extern "system"` FFI, no crates. Headless runs can dump frames as PPM files.

Details:

- The registers are 32-bit only: byte or halfword accesses in 0x3000_0000 - 0x3000_0FFF raise access faults. Unused offsets in that page read as 0 and ignore writes, as do writes to the read-only registers. `FB_PRESENT` reads as 0.
- `FB_CTRL` bits other than bit 0 are reserved, read as 0. Both registers reset to 0, so the display starts disabled.
- While the display is disabled it shows black; VRAM keeps its contents and can be drawn into before enabling.
- VRAM is plain memory: byte, halfword and word accesses, same alignment rules as RAM (section 4). It resets to 0. The top byte of each pixel is ignored. Byte order in memory is B, G, R, X, which is also a 32-bpp Windows DIB, so Grail blits it without conversion.
- Grail runs a machine and its window on one thread: the CPU runs in slices that end early right after a `FB_PRESENT` write (or, with `--clock`, when it is ahead of the wall clock; Grail then sleeps at most ~10 ms at a time so the window stays responsive); between slices it pumps the window's messages and redraws on a present, when Windows asks for a repaint, or when ~16 ms of host time have passed. The window is opened only with `--fb` (Windows only; elsewhere Grail warns and runs headless, the default). The image is stretched to the window's client area. Closing the window stops the emulator (exit code 0); if the program stops first, the window stays open on the last frame until it is closed, and Grail then exits with the program's code.
- `--fb-dump DIR` writes `DIR/frame-NNNN.ppm` (binary P6, 640×480, numbered from 0000) after every `FB_PRESENT` and once more when the machine stops. It works with or without the window.

### 9.4 Keyboard (MMIO)

| Address | Register | Notes |
|---|---|---|
| 0x3000_1000 | `KBD_STATUS` (RO) | bit 0 = event available |
| 0x3000_1004 | `KBD_DATA` (RO) | pops one event: bits [7:0] key code, bit 8 = 1 on release, bit 9 = shift, bit 10 = ctrl. 0 if empty |
| 0x3000_1008 | `KBD_CTRL` | bit 0 = interrupt enable (PLIC source 2, section 7.1) |

Key codes:

| Key | Code |
|---|---|
| Printable keys | the ASCII character typed (0x20-0x7E) |
| Backspace, Tab, Enter, Esc | 0x08, 0x09, 0x0A, 0x1B (ASCII) |
| Up, Down, Left, Right | 0x80, 0x81, 0x82, 0x83 |
| F1-F12 | 0x90-0x9B |

Code 0 never occurs, so a `KBD_DATA` of 0 always means "empty". Bits 31:11 read as 0.

Details:

- The registers are 32-bit only: byte or halfword accesses in 0x3000_1000 - 0x3000_1FFF raise access faults (and pop nothing). Unused offsets read as 0 and ignore writes, as do writes to `KBD_STATUS` and `KBD_DATA`. `KBD_CTRL` bits other than bit 0 are reserved, read as 0; it resets to 0.
- Events wait in a FIFO of 64. An event that arrives while it is full is dropped (the older ones are kept).
- **Interrupt**: the keyboard holds PLIC source 2 high while `KBD_CTRL.0` and `KBD_STATUS.0` are both set, so reading `KBD_DATA` until `KBD_STATUS.0` clears lowers it. As with the UART, a handler may pop one event per interrupt and let the line raise the next one.
- **Printable keys** report the character the host keyboard layout types with the current Shift and Caps Lock state, so Shift+a gives 'A' (0x41) with bit 9 set and Shift+1 gives '!' on a US layout. Ctrl does not change the code: Ctrl+C is 'c' with bit 10 set, not 0x03. Keys that type no ASCII character (dead keys, accented letters, Insert, Home, ...) send no events.
- Shift and Ctrl (and Alt) send no events of their own; they only set bits 9 and 10 of other keys' events, as held at that moment. Ctrl+Alt is AltGr on Windows: it types the layout's AltGr character and does not set bit 10.
- A release carries the same code as its press, even if Shift changed in between. A held key auto-repeats as more press events. When the window loses focus, every key it saw go down gets a release event, so none stays stuck.
- Grail delivers the events typed into the framebuffer window between instruction slices (section 9.3). Without a window (headless) nothing produces events and the device stays empty; the UART remains the text input. Hosts that embed Grail as a library inject events with `Machine::key_event`, the same entry point the window uses.

### 9.5 Sysinfo (MMIO)

Read-only registers that describe the machine, so software discovers its hardware instead of hard-coding it: the kernel takes the RAM size for its page allocator and the timebase for its timer tick from here. A hypervisor host running several Grail machines gives each its own values.

| Address | Register | Notes |
|---|---|---|
| 0x3000_2000 | `SYS_MAGIC` | 0x5359_5349 ("SYSI"): the block is present |
| 0x3000_2004 | `SYS_VERSION` | 1. Changes only if an existing register changes meaning; new registers and device types do not need a new version |
| 0x3000_2008 | `SYS_RAM_BASE` | 0x8000_0000 |
| 0x3000_200C | `SYS_RAM_SIZE` | RAM size in bytes (a multiple of 4 KiB; 0x0800_0000 by default, 0x8000_0000 at most) |
| 0x3000_2010 | `SYS_TIMEBASE_HZ` | `mtime` frequency in Hz |
| 0x3000_2014 | `SYS_CPU_HZ` | CPU clock in Hz; equal to the timebase (one instruction per tick) |
| 0x3000_2018 | `SYS_FLAGS` | bit 0 = the clock is real time (`--clock`); clear = nominal (default 10 MHz, runs at full speed). Other bits are reserved and read as 0 |
| 0x3000_201C | `SYS_DEV_COUNT` | number of device table entries |
| 0x3000_2020 - 0x3000_203F | reserved | read as 0 |
| 0x3000_2040 + 16·i | device table entry `i` | four words, below |

Frequencies are plain 32-bit Hz values, like the device-tree `timebase-frequency` property, so a 10 ms tick is `SYS_TIMEBASE_HZ / 100` ticks.

Device table entry `i` (for `i` < `SYS_DEV_COUNT`; at most 252 fit in the page):

| Offset | Field | Notes |
|---|---|---|
| +0x0 | `DEV_TYPE` | device type code, below |
| +0x4 | `DEV_BASE` | physical address of its MMIO range |
| +0x8 | `DEV_SIZE` | size of that range in bytes |
| +0xC | `DEV_IRQ` | its PLIC source (section 7.1), 0 = none |

| Type | Device |
|---|---|
| 0 | none (never listed) |
| 1 | CLINT (section 7) |
| 2 | PLIC (section 7.1) |
| 3 | UART, 16550 subset (section 7) |
| 4 | Test/exit device (section 7) |
| 5 | Framebuffer control (section 9.3) |
| 6 | Framebuffer VRAM (section 9.3) |
| 7 | Sysinfo (this block) |
| 8 | Keyboard (section 9.4) |
| 9 | Block device, one entry per disk (section 9.6) |
| 10 | Pointer (section 9.7) |
| 11 | Network card, one entry per card (section 9.8) |

Entries are in address order. Software looks devices up by type, not by index. Type codes are never reused: a new device gets the next code and an entry, with no new registers and no change to this layout (the block device, type 9, the pointer, type 10, and the network card, type 11, were added this way). Grail's current machine lists the test device, CLINT, PLIC, UART (irq 1), one block device per disk (disk `i` at 0x1000_1000 + 0x1000·`i`, irq 3 + `i`; none without `--disk`), one network card per `--net` (card `i` at 0x1000_9000 + 0x1000·`i`, irq 12 + `i`; none without `--net`), framebuffer control, keyboard (irq 2), sysinfo, pointer (irq 11) and VRAM; `examples/sysinfo` prints the table.

The registers are 32-bit only: byte or halfword accesses raise access faults. Word writes are ignored, as for the framebuffer's read-only registers. Offsets past the last entry read as 0.

### 9.6 Block device (MMIO)

Disks of 512-byte sectors. A machine has 0 to 8 of them, in the order they are given (`grail --disk IMG`, repeatable; `MachineConfig::disks` for a host using Grail as a library). Disk `i` has a 4 KiB page of registers at 0x1000_1000 + 0x1000·`i` and interrupts on PLIC source 3 + `i`, and it has an entry of type 9 in the sysinfo device table (section 9.5), which is how software finds it. The pages of disks the machine does not have are unmapped.

The interface is one command at a time with DMA: software writes the first sector, the buffer's address in RAM and the sector count, then a command; the device copies the sectors between the disk and RAM itself, sets `BLK_STATUS.DONE` and, if enabled, raises its interrupt. The memory map used to reserve this slot for a virtio-blk device. Virtio was set aside because a driver must negotiate features and build a virtqueue in guest memory (descriptor table, available ring, used ring) behind the virtio-mmio transport registers before the first sector moves, while a driver for this device is about twenty instructions (`examples/disk`) that a first kernel driver can copy. Having only one request in flight costs nothing in an emulator that finishes each request at once. A queued device can still be added later as a new type code.

| Offset | Register | Notes |
|---|---|---|
| +0x00 | `BLK_CAPACITY` (RO) | disk size in sectors (at most 2^32 - 1, just under 2 TiB) |
| +0x04 | `BLK_FLAGS` (RO) | bit 0 = the disk is read-only. Other bits read as 0 |
| +0x08 | `BLK_SECTOR` | first sector of the transfer |
| +0x0C | `BLK_DMA` | physical address of the buffer in RAM |
| +0x10 | `BLK_COUNT` | number of sectors to transfer |
| +0x14 | `BLK_CMD` (WO) | writing a command runs it: 1 = read (disk to RAM), 2 = write (RAM to disk), 3 = flush. Reads as 0 |
| +0x18 | `BLK_STATUS` | bit 0 = BUSY (read-only); bit 1 = DONE: a command finished and has not been acknowledged. Writing 1 to bit 1 clears it; other bits ignore writes |
| +0x1C | `BLK_ERROR` (RO) | outcome of the last command, below |
| +0x20 | `BLK_CTRL` | bit 0 = completion interrupt enable. Other bits read as 0 |

| `BLK_ERROR` | Meaning |
|---|---|
| 0 | none: the command worked |
| 1 | unknown command |
| 2 | out of range: `BLK_SECTOR + BLK_COUNT` is past `BLK_CAPACITY` |
| 3 | bad buffer: the `BLK_COUNT` × 512 bytes at `BLK_DMA` are not all in RAM |
| 4 | write to a read-only disk |
| 5 | I/O error on the host side (the backend failed) |

Details:

- The registers are 32-bit only: byte or halfword accesses in a disk's page raise access faults and run nothing. Unused offsets read as 0 and ignore writes, as do writes to the read-only registers. Everything writable resets to 0. `BLK_SECTOR`, `BLK_DMA` and `BLK_COUNT` keep their values after a command.
- **Commands complete at once.** Grail runs a command to completion during the store to `BLK_CMD`, so BUSY never reads 1 and DONE is already set when the next instruction runs. BUSY is defined anyway, so that a driver which waits for BUSY to clear or for DONE (by polling or by interrupt) also works with a disk that takes time. A command written while BUSY is set is ignored. Every command written sets DONE and `BLK_ERROR`, failed ones included.
- **Checks come first**, in the order of the error codes (1 to 4), before any data moves: a command that fails one transfers nothing. The range check is done without overflow, so a huge `BLK_SECTOR` or `BLK_COUNT` fails with 2 instead of wrapping. The buffer must lie in RAM (not VRAM, the boot ROM or device registers); it may start at any byte address. `BLK_DMA` is a physical address: the device does not use the MMU. With `BLK_COUNT` 0 a read or write transfers nothing, and succeeds if the checks pass (`BLK_SECTOR` at most `BLK_CAPACITY`, `BLK_DMA` in RAM or just past its end). Only error 5 can leave a transfer half done. Nothing a program writes makes the host panic or touches memory outside the buffer.
- **Flush** ignores `BLK_SECTOR`, `BLK_DMA` and `BLK_COUNT`, and completes once every earlier write is durable on the host (for an image file, the host OS has written it to its own disk). It succeeds on a read-only disk.
- **Interrupt**: a disk holds its PLIC source high while `BLK_CTRL.0` and `BLK_STATUS.DONE` are both set. A handler claims the source, reads `BLK_ERROR`, writes 2 to `BLK_STATUS` (which lowers the line) and completes. If it completes without acknowledging, the source is pending again at once (section 7.1).
- **Backends.** Where a disk's sectors live is up to the host: Grail gives each disk of each machine its own backend (`BlockBackend` in the grail library: capacity, read, write, flush). `--disk IMG` uses an image file on the host, `--disk-ro IMG` the same file opened read-only (`BLK_FLAGS.0` set); tests use in-memory disks, and a host can hand Grail backends of its own (`Machine::with_backends`). `--disk lake://host:port/name` is a network disk: image `name` on a Lake storage server (Avalon protocol, `docs/avalon-protocol.md` section 6.5), attached when the machine is built and detached when it is dropped; every command is a request to Lake, answered before the store to `BLK_CMD` completes, and any failure (Lake's error, a server gone or silent for 10 s) is error 5. An image is raw: sector `n` is bytes 512·`n` to 512·`n` + 511 of the file. Its size must be a whole number of sectors, or Grail refuses to start; it never grows. Writes go to the file as they happen. Grail does not lock image files, so two machines must not attach the same file read-write; Lake does enforce it for its images (one writer, or any number of read-only attachments).

`examples/disk` finds the first disk in the device table, prints its sector 0 as text, checks that a read past the end fails with error 2, and runs a write/read-back test on the last sector (saving and restoring it), waiting for the completion interrupt after every command: `grail --disk examples/disk/disk.img examples/disk/disk.elf`. The Camelot kernel's driver and buffer cache are in section 10, "Kernel disks".

### 9.7 Pointer (MMIO)

A mouse for graphical guests, and for remote viewers that forward the host's mouse. It has a 4 KiB page of registers at 0x3000_3000, PLIC source 11 and an entry of type 10 in the sysinfo device table.

**Absolute mode** (the default) reports where the pointer is, in framebuffer pixels: `x` from 0 to `FB_WIDTH` - 1 and `y` from 0 to `FB_HEIGHT` - 1 (section 9.3), like a USB tablet. The guest's cursor is then exactly where the host's is: no drift between the two, and a viewer never has to grab ("capture") the host mouse. **Relative mode** (`PTR_CTRL` bit 1) reports the motion since the previous event (`dx`, `dy`) instead, for software that wants deltas (a game, a guest with its own acceleration).

| Offset | Register | Notes |
|---|---|---|
| +0x00 | `PTR_STATUS` | bit 0 = an event is waiting (read-only); bit 1 = OVERFLOW: an event was dropped because the FIFO was full. OVERFLOW stays set until software writes 1 to bit 1. Other bits read as 0 and ignore writes |
| +0x04 | `PTR_EVENT` (RO) | reading it pops the oldest event and returns its first word (below), and latches its position into `PTR_EVENT_XY`. Reads 0 when the FIFO is empty |
| +0x08 | `PTR_EVENT_XY` (RO) | position word of the event last popped through `PTR_EVENT` (below). Reading it pops nothing. 0 at reset and after a pop that found the FIFO empty |
| +0x0C | `PTR_CTRL` | bit 0 = interrupt enable (PLIC source 11); bit 1 = relative mode (0 = absolute). Other bits read as 0 |
| +0x10 | `PTR_POS` (RO) | where the pointer is now, absolute in both modes: `x` in bits 15:0, `y` in bits 31:16 |
| +0x14 | `PTR_BUTTONS` (RO) | the buttons held now, in the bits of `PTR_EVENT` 2:0 |

An event is two words. `PTR_EVENT`:

| Bits | Field |
|---|---|
| 0 | left button held |
| 1 | right button held |
| 2 | middle button held |
| 3 | RELATIVE: the position word holds `dx`, `dy` (the event was made in relative mode) |
| 7:4 | reserved, 0 |
| 15:8 | wheel: notches turned, signed (two's complement), positive away from the user ("up") |
| 30:16 | reserved, 0 |
| 31 | VALID: 1 for every event, so a `PTR_EVENT` of 0 means "empty" |

`PTR_EVENT_XY`: `x` in bits 15:0 and `y` in bits 31:16. In absolute mode they are the pixel the pointer is on (unsigned, within the framebuffer). In relative mode they are `dx` and `dy`, signed 16-bit (saturated at -32768 and 32767): right and down are positive, as in framebuffer coordinates. The buttons in an event are the ones held after it, so a click is an event with the bit set followed by one with it clear, each with the position where it happened.

Reading an event: read `PTR_EVENT` (it pops), then `PTR_EVENT_XY`. Software that only wants to know where the pointer is now can read `PTR_POS` and `PTR_BUTTONS` instead, without touching the FIFO.

Details:

- The registers are 32-bit only: byte or halfword accesses in 0x3000_3000 - 0x3000_3FFF raise access faults (and pop nothing). Unused offsets read as 0 and ignore writes, as do writes to the read-only registers. Everything resets to 0: absolute mode, interrupts off, the pointer at (0, 0) with no buttons held, the FIFO empty.
- **Events** wait in a FIFO of 64. Each change the host reports makes one event, in the mode `PTR_CTRL` selects at that moment; switching modes does not change the events already queued (their RELATIVE bit says how to read them). A change that the program could not see makes no event: no motion (in relative mode, a zero `dx`, `dy`; in absolute mode, the same pixel), the same buttons and no wheel.
- **Clamping**: absolute positions are clamped to the framebuffer, so a drag that leaves the host's window pins the pointer to the edge. In relative mode a host that reports motion as deltas has them passed on as they are, even against an edge (the absolute position, `PTR_POS`, still stops there); a host that reports positions has its deltas computed between clamped positions.
- **Overflow**: an event that arrives while the FIFO is full is dropped (the older ones are kept) and sets OVERFLOW. `PTR_POS` and `PTR_BUTTONS` follow every change, dropped ones included, so after an overflow a program catches up by reading them, then writes 2 to `PTR_STATUS`.
- **Interrupt**: the pointer holds PLIC source 11 high while `PTR_CTRL.0` and `PTR_STATUS.0` are both set, so reading `PTR_EVENT` until `PTR_STATUS.0` clears lowers it. OVERFLOW alone does not interrupt. A handler may read every waiting event at once, or one per interrupt and let the line raise the next.
- **Host side**: Grail delivers the mouse over the framebuffer window between instruction slices, like keys (sections 9.3, 9.4): motion (`WM_MOUSEMOVE`), the left, right and middle buttons and the wheel (`WM_MOUSEWHEEL`; fractions of a notch from precise wheels add up until they make one). Window coordinates are mapped to framebuffer pixels through the stretch, so the mapping holds at any window size. While a button is held the window captures the mouse, so a drag keeps reporting (clamped) positions outside it and its release is not lost; if the window loses the mouse or the focus with buttons held, they are released. Without a window nothing produces events. Each machine has its own pointer, and hosts that embed Grail as a library (a remote viewer, a test) inject input with `Machine::pointer_event`: a new position (in framebuffer pixels) or a motion in pixels, the buttons held, and the wheel notches; the device turns it into an event in the guest's mode.

`examples/pointer` draws an arrow cursor that follows the pointer and changes colour while buttons are held, reading the events on the pointer's interrupt; Esc on the keyboard ends it: `grail --fb examples/pointer/pointer.elf`.

### 9.8 Network card (MMIO)

An Ethernet card, so machines can network. It sends and receives raw Ethernet II frames (destination, source, EtherType, payload; no FCS, no padding required) of 14 to 1514 bytes, through two rings of descriptors in RAM, like a minimal virtio-net or e1000. A machine has 0 to 4 cards, in the order they are given (`grail --net hub`, repeatable; `MachineConfig::nets` for a host using Grail as a library). Card `i` has a 4 KiB page of registers at 0x1000_9000 + 0x1000·`i`, right after the disks' pages, interrupts on PLIC source 12 + `i`, and has an entry of type 11 in the sysinfo device table (section 9.5). The pages of cards the machine does not have are unmapped.

| Offset | Register | Notes |
|---|---|---|
| +0x00 | `NET_MAC_LO` (RO) | bytes 0-3 of the card's MAC address, byte 0 (the first on the wire) in bits 7:0, so a word store of it puts the bytes in wire order |
| +0x04 | `NET_MAC_HI` (RO) | bytes 4-5 in bits 15:0; bits 31:16 read 0 |
| +0x08 | `NET_STATUS` (RO) | bit 0 = link up (the backend can carry frames). Other bits read 0 |
| +0x0C | `NET_ERROR` | the latest ring error, below; 0 = none. Any write clears it |
| +0x10 | `NET_IRQ_ENABLE` | bit 0 = RX, bit 1 = TX, bit 2 = ERROR, as in `NET_IRQ_STATUS`. Other bits read 0 |
| +0x14 | `NET_IRQ_STATUS` | bit 0 = RX: the card handed back receive descriptors; bit 1 = TX: it handed back transmit descriptors; bit 2 = ERROR: it recorded a ring error. Writing 1 to a bit clears it |
| +0x18 | `NET_RX_DROPPED` | frames lost on the way in (a 32-bit counter that wraps). Any write sets it to 0 |
| +0x1C | reserved | reads 0 |
| +0x20 | `NET_TX_BASE` | physical address of the transmit ring |
| +0x24 | `NET_TX_SIZE` | its size in descriptors: 0 (no ring) or a power of two from 1 to 1024 |
| +0x28 | `NET_TX_HEAD` (RO) | counter of the next descriptor the card will send |
| +0x2C | `NET_TX_TAIL` | counter one past the last descriptor the driver has filled. Writing it sends |
| +0x30 - +0x3C | `NET_RX_BASE`, `NET_RX_SIZE`, `NET_RX_HEAD` (RO), `NET_RX_TAIL` | the same for the receive ring |

**Descriptors** are 8 bytes, little-endian like everything else:

| Offset | Field | Notes |
|---|---|---|
| +0 | `addr` (u32) | physical address of the buffer, in RAM; any byte address |
| +4 | `len` (u16) | transmit: the frame's length, set by the driver. Receive: the buffer's size, set by the driver; the card overwrites it with the length of the frame it received |
| +6 | `status` (u16) | written by the card when it hands the descriptor back, below |

**Rings.** A ring is `SIZE` descriptors at `BASE`, and two free-running 32-bit counters, `HEAD` and `TAIL`; counter `n` is slot `n % SIZE` (`SIZE` is a power of two, so this holds across the wrap at 2^32). The descriptors from `HEAD` up to, not including, `TAIL` belong to the card, the rest to the driver: the driver hands descriptors over by advancing `TAIL`, the card hands them back by advancing `HEAD`. The ring is empty when `TAIL == HEAD` and full when `TAIL - HEAD == SIZE` (mod 2^32), so every slot can be used, with no slot kept free to tell the two apart. Writing `BASE` or `SIZE` starts the ring over with `HEAD = TAIL = 0`, which is how a driver sets one up (and recovers from a ring error). A `TAIL` more than `SIZE` ahead of `HEAD` is refused: the write is ignored and `NET_ERROR` becomes 2.

- **Transmit**: the driver fills the descriptor at slot `TX_TAIL % SIZE` (address and length of a frame in RAM), then writes `TX_TAIL + 1` (or several at once). Grail sends every descriptor from `TX_HEAD` to the new `TX_TAIL` during that store, in order, writing each one's status and advancing `TX_HEAD` past it, so `TX_HEAD == TX_TAIL` when the next instruction runs. A driver should still wait for `TX_HEAD` (or the TX interrupt) before reusing a descriptor or its buffer, so that it also works with a card that takes time. Then `NET_IRQ_STATUS.TX` is set.
- **Receive**: the driver gives the card empty buffers by filling descriptors (address, size) and advancing `RX_TAIL`; at start, typically all `SIZE` of them. Each frame that arrives goes into the buffer of the descriptor at `RX_HEAD`: the card copies it, writes its length and status, advances `RX_HEAD` and sets `NET_IRQ_STATUS.RX`. The driver reads the descriptors from its own counter up to `RX_HEAD`, then gives each buffer back by writing its size into `len` again (the card overwrote it) and advancing `RX_TAIL`.
- **Flow control**: a frame that arrives while the card owns no receive descriptor (`RX_HEAD == RX_TAIL`, a full ring or none set up) is dropped and counted in `NET_RX_DROPPED`. Frames never wait in the card: what the guest cannot take is lost, as on a real network, and nothing queues without bound. Frames shorter than 14 or longer than 1514 bytes from a backend are dropped and counted too. Grail takes received frames from the backend every 256 steps (instructions, or ticks spent in `wfi`, section 7; in real-time mode the idle ticks skipped in a batch count too), at most 64 at a time, so a frame lands in RAM between two instructions and wakes a hart waiting in `wfi` for the interrupt; a card the guest has not set up still drains its backend, dropping everything.

| `status` | Meaning |
|---|---|
| 0 | ok: the frame was sent, or received into the buffer |
| 1 | transmit: the length is not 14 to 1514; nothing was sent |
| 2 | the buffer (`addr`, `len` bytes) is not entirely in RAM; nothing was sent or copied |
| 3 | receive: the frame is longer than the buffer; nothing was copied, and `len` holds the frame's length |
| 4 | transmit: the backend failed to send the frame |

A received frame whose descriptor gets status 2 or 3 is lost, and counted in `NET_RX_DROPPED` as well.

| `NET_ERROR` | Meaning |
|---|---|
| 0 | none |
| 1 | a `SIZE` that is neither 0 nor a power of two up to 1024 was written; the ring's size became 0 |
| 2 | a `TAIL` more than `SIZE` descriptors ahead of `HEAD` was written; it was ignored |
| 3 | a descriptor the card needed is not in RAM (`BASE` is wrong): transmit stops there (`TX_HEAD` stays on it until the ring is set up again); a received frame is dropped and counted |

Details:

- The registers are 32-bit only: byte or halfword accesses in a card's page raise access faults and do nothing. Unused offsets read as 0 and ignore writes, as do writes to the read-only registers. Everything writable resets to 0: no rings, interrupts off.
- **DMA rules** are the block device's (section 9.6): addresses are physical (the card does not use the MMU), descriptors and buffers must lie entirely in RAM (not VRAM, the boot ROM or device registers), checked without overflow, and they may start at any byte address. A bad descriptor gets an error status and the card moves on; a bad ring is a `NET_ERROR`. Nothing a program writes makes the host panic or touches memory outside the descriptors and buffers.
- **Interrupt**: a card holds its PLIC source high while `NET_IRQ_STATUS & NET_IRQ_ENABLE` is non-zero. A handler claims the source, handles the receive ring up to `RX_HEAD` (and reclaims transmit descriptors up to `TX_HEAD`), writes the bits it saw to `NET_IRQ_STATUS` (which lowers the line) and completes. Dropped frames do not interrupt.
- **MAC address**: set by the host, read-only for the guest, always an individual (not a group) address. A configured card gets the one in its config (`--net hub,mac=02:00:00:00:00:42`); otherwise a hub port gives `02:ca:00:00:00:NN`, `NN` being the port's number on its hub in the order cards attach (from 1), which is locally administered (bit 1 of the first byte), so it never clashes with a manufacturer's.
- **Backends.** Where frames go is up to the host: Grail gives each card of each machine its own backend (`NetBackend` in the grail library: send a frame, poll for a received one). Both calls are made from the machine's run loop and never block, and backends are `Send`, so frames can cross to other threads: to another machine, a switch or a socket. `--net hub` puts the card on a port of an in-process **hub** (`grail::net::Hub`), shared by every `--net hub` of the process: a frame one port sends is queued for every other port (a repeater, not a switch: no address learning), up to 256 frames per port, beyond which that port's frames are dropped. Tests connect two machines in one process by giving their configs the same hub. A host can hand Grail backends of its own with their MAC addresses (`Machine::with_devices`), after the configured cards.
- **Capture**: `--pcap FILE` (after a `--net`) writes every frame that card sends or receives to FILE, in the classic libpcap format: a 24-byte header (magic 0xa1b2c3d4 written little-endian, version 2.4, snap length 65535, link type 1, `LINKTYPE_ETHERNET`), then per frame a 16-byte record header (wall-clock time stamp in seconds and microseconds, captured and original length) and the frame. Wireshark and tcpdump read it. The capture wraps the backend (`grail::net::Pcap`), so it sees received frames as the card takes them, including ones the card then drops; it is flushed after every frame, so the file is complete whenever the machine stops.

The card's design follows the block device's choice (section 9.6) of something a first driver can copy over a general queue interface: no feature negotiation, one descriptor format for both directions, and every slot of a ring usable thanks to free-running counters. A driver is about forty instructions (`examples/net/netlib.S`).

`examples/net` is a pair of programs sharing that driver: `net-pong` waits for a frame on its receive interrupt (in `wfi`), prints it and answers its sender; `net-ping` broadcasts a frame (EtherType 0x88b5, "local experimental") and polls the receive ring for the answer. `crates/grail/tests/net.rs` runs them on two machines on one hub, and checks a capture of the exchange byte for byte against `crates/grail/tests/golden/net-ping.pcap`. One `grail` process runs one machine, so there `net-ping` gets no answer, but its frame is in the capture: `grail --net hub --pcap ping.pcap examples/net/net-ping.elf`.

## 10. Toolchain (all our own)

Everything that runs on the host is written by us, with no third-party libraries doing the heavy lifting (std only). The target side is code produced by our own tools.

| Tool | Input → output | Notes |
|---|---|---|
| Assembler (`merlin-as`) | `.s` → `.o` | Full RV32I(+M, Zicsr, Zifencei, privileged, Xcal) mnemonics, standard pseudo-instructions (`li`, `la`, `call`, `ret`, `mv`, `j`, `beqz`...), `.insn`, directives (`.text .data .bss .section .globl .word .byte .ascii .align .equ`), `%hi/%lo/%pcrel_hi/%pcrel_lo`. See "Assembler" below |
| Linker (`merlin-ld`) | `.o` files → executable | Symbol resolution, relocation, section merging, simple linker script (or fixed layout: `.text` at 0x8000_0000) |
| Emulator (`grail`) | executable → runs it | Loads ELF directly, no boot loader needed early on |
| Disassembler (`merlin-objdump`) | `.o`/exe → text | Shares the decoder with the emulator; essential for debugging |
| Compiler (`merlin`) | C subset → `.s` | Emits assembly for our assembler. Integer types, pointers, arrays, globals and strings so far; grows toward enough C for the kernel (see "Merlin C subset") |

### Merlin C subset

`merlin input.c -o output.s` compiles one translation unit; `--dump-ast` prints the checked syntax tree instead. Errors are reported as `file.c:line:col: error: message` (columns count characters from 1); lexing and parsing stop at the first error, name resolution reports all of them.

The language is a strict subset of C99: a program whose behavior C defines means the same thing to clang (`--target=riscv32`, `-std=c99`); anything outside the subset is an error, never a different meaning. It grows toward what the kernel needs (tracking issue #15). v0 was `int` only; the current subset adds the integer types, pointers, arrays, globals and strings:

- **Types**: `char`, `signed char`, `unsigned char`, `short`, `unsigned short`, `int`, `unsigned` (`unsigned int`), `long`, `unsigned long` (specifiers in any order, `signed`/`short int`/`long int` spellings included), `void` as a function result and behind pointers, pointers to any of these, and arrays of them (any number of dimensions). Sizes are ILP32: `char` 1, `short` 2, `int`/`long`/pointers 4, each aligned to its size. Plain `char` is **unsigned**, as on every RISC-V target. `long long`, `_Bool`, floating point, `struct`/`union`/`enum`, `typedef`, `const`/`volatile` and function pointers are errors for now ("... is not supported yet").
- **Declarations**: one specifier list, then declarators made of `*`s, a name and `[N]`s, with an optional initializer (`int a = 1, *p = &a, v[4];`). Array sizes are integer constant expressions made of literals, operators, casts and `sizeof(type)` (`sizeof expr` in an array size is an error for now); `int a[] = {...}` and `char s[] = "..."` take their size from the initializer, unless an earlier declaration gave one (`int a[3]; int a[] = {1};` is an `int[3]`). A name is in scope inside its own initializer, as in C, for globals and `static` locals too (`static void *head[2] = {head, head};`). Parenthesized declarators are errors.
- **Functions**: `T f(T1 a, ...)` with at most 8 parameters, passed in `a0-a7` (ILP32 calling convention, result in `a0`); `void` functions and `return;`. `f(void)` and `f()` both mean no parameters. Array parameters are pointers (`int m[3][3]` is `int (*m)[3]`). Prototypes may leave parameter names out; definitions may not. Arguments are converted to the parameter types as by assignment. `static` functions get no `.globl`.
- **Globals**: file-scope variables, `static` or not, and `extern` declarations; repeated declarations must agree on the type, and a tentative definition (`int x;`) counts once. Initializers must be constant: integer constant expressions, `0` for a null pointer, the address of a global or of an element of one (`&table[2]`, `table + 4`), a string literal, or a brace list of those, with each inner array in its own braces (or a string literal, for a `char` array): C's brace elision (`int m[2][2] = {1, 2, 3, 4};`) is an error for now. Missing elements are zero. A variable with an initializer goes to `.data`, one without to `.bss` (zero, as C requires). A `static` local is a global too, named `func.name` (`func.name.1` for a second one with the same name), initialized once before the program starts.
- **Literals**: decimal, hex (`0x`) and octal (`0`) integers with `u`/`l` suffixes, typed by C's rules for the types merlin has: a decimal literal is `int` or `long`, a hex or octal one may also be `unsigned int`/`unsigned long` (`0xffffffff` is `unsigned int`), and a literal that would need `long long` (`2147483648`, `4294967296u`) is an error (write `INT_MIN` as `-2147483647 - 1`). Character constants (`'a'`, type `int`) and string literals (`char[N]`, in `.rodata`, adjacent ones joined) with the escapes `\n \t \r \0 \\ \' \" \a \b \f \v \?`, octal `\ooo` and hex `\xhh`. String literals are read-only: storing into one is undefined. Trigraphs (`??=`, `??/`...) and line splices (a backslash at the end of a line), which C rewrites before reading tokens, are errors wherever they would change the meaning (in a `//` comment, a splice would continue the comment); write `\?` for a `?` that would start a trigraph.
- **Statements**: blocks, declarations (anywhere in a block, and in a `for` initializer), expression statements, `if`/`else`, `while`, `do`/`while`, `for`, `return`, `break`, `continue`.
- **Expressions**: every C operator except `.`, `->` and function calls through pointers: `= += -= *= /= %= <<= >>= &= ^= |=`, `?:`, `,`, `|| &&`, `| ^ &`, `== != < <= > >=`, `<< >>`, `+ - * / %`, unary `- + ! ~ & * ++ --` (prefix and postfix), casts, `sizeof` (of a type or an expression, which is not evaluated; its result is `unsigned int`, C's `size_t`), indexing (`a[i]` is `*(a + i)`, so `i[a]` works too) and calls. Operators C lexes as one token that merlin lacks (`->`, `...`) are rejected rather than split.
- **Conversions**: integer promotion and the usual arithmetic conversions as in C (an `unsigned int` operand makes the operation unsigned; `long` and `int` are both 32 bits, so `long` with `unsigned int` is `unsigned long`). Arrays decay to pointers. Pointer arithmetic counts in elements, and the difference of two pointers is an `int`. Implicit conversions between integers and pointers (other than a null pointer constant) and between different pointer types (other than to and from `void *`) are errors where clang only warns; casts allow them. Converting an out-of-range value to a signed type keeps the low bytes, as clang does.
- Names must be declared before use, as in C99: a call to a function not yet declared is an error (no implicit declarations), and so is a wrong argument count. Block scoping follows C: inner blocks and `for` initializers may shadow outer names; parameters share the scope of the function body's outermost block.
- Statements and expressions nest at most 256 levels deep (each nested statement, `=`, unary operator, parenthesis, call argument or chained binary operator is a level); deeper input is an error. So is a declarator with more than 256 `*`s or 256 array dimensions, and a function whose locals need a frame over 2 GiB.
- Where C leaves behavior undefined, merlin defines it the way the hardware does: signed arithmetic wraps, `/` and `%` round toward zero (`div`/`rem`, so `INT_MIN / -1` wraps and division by zero gives RISC-V's results), a shift uses the low 5 bits of its amount (`sll`/`srl`/`sra`), and falling off the end of a non-`void` function returns 0. Argument evaluation order is unspecified; tests must not depend on it.
- There is no inline `asm()`, on purpose: code that needs particular instructions (CSRs, `sfence.vma`, Xcal) lives in `.s` files and is called like any function, which the kernel already does. No preprocessor yet.

Still to come, in the order the kernel needs them (issue #15): `struct`/`union` (with the ILP32 layout and by-value passing), `typedef`, `enum`, `const`/`volatile`, `switch`, `goto`, function pointers, a preprocessor (`#include`, `#define`, `#if`), `__builtin_va_*` for `kprintf`, `inline`, and `_Bool`; then building the kernel with merlin.

Code generation (`crates/merlin/src/codegen.rs`, whose module docs walk through it) is a stack machine: every expression leaves its value in `a0`, operands waiting for the other side are pushed on the stack, every variable lives in memory. The output is plain merlin-as source (`.text`/`.data`/`.bss`/`.section .rodata`, `.globl f`, `f:`, local labels `.L.<what>.<n>`, string literals `.L.str.<n>`) that clang assembles to the same bytes and relocations in every section. Values in registers are 32 bits: `char` and `short` are widened when loaded (`lbu`/`lb`, `lhu`/`lh`) and narrowed when stored (`sb`, `sh`); unsigned operations use `divu`, `remu`, `sltu` and `srl`. Functions follow the ILP32 convention:

- Frame: `s0` is the frame pointer, equal to `sp` on entry. `ra` is saved at `-4(s0)`, the caller's `s0` at `-8(s0)`, then the locals (parameters first, as numbered by `--dump-ast`) downward from there, each at the next offset aligned to its type (arrays to at least 4), so with only `int`s local `#i` is at `-(12 + 4*i)(s0)`. The frame is rounded up to 16 bytes; a comment above each function lists every local's offset. Prologue and epilogue use `t0` for frames or offsets beyond a 12-bit immediate.
- Parameters arrive in `a0-a7`, widened by their type, and are stored to their slots in the prologue; the result returns in `a0`. Arguments are evaluated left to right (C leaves the order unspecified) before any argument register is loaded, and `sp` is padded to a multiple of 16 around a `call` made while temporaries are pushed.
- A local declared without an initializer is not cleared (C: indeterminate). A local array with an initializer is cleared first (word stores, or a loop for more than 16 words), then its elements are stored.
- A function or global may not be named like a register (`sp`, `a0`, `x5`, `fp`...), since merlin-as never reads those as symbols: it is a compile error. Names of instructions, pseudo-instructions or CSRs (`add`, `call`, `mstatus`) are fine.
- merlin-as does not relax branches, so codegen does: before printing a function it estimates every instruction's address (pseudo-instructions at their largest) and writes a conditional branch that may not reach its label (±4 KiB) as the opposite branch over a `j` (±1 MiB).

Test programs live in `crates/merlin/tests/cases/*.c`, each starting with `// expect: N` (the exit code); the ones in `crates/merlin/tests/cases-merlin/` rely on the definitions above for what C leaves undefined, so they are not checked against clang. The harness links them with a small `crt0.s` (set `sp` to the top of RAM, `call main`, `xcal.halt a0`; plus a `memset`, which clang calls to clear large local arrays) and runs them in Grail, so the exit code is the low 8 bits of `main`'s return value. A reference interpreter in the harness (`tests/common/interp.rs`, byte-addressed memory with every object bounds-checked) runs the checked syntax tree too, and must agree.

### Object and executable format

**ELF32 (little-endian, EM_RISCV)**, a subset:

- Relocatable `.o`: sections `.text .data .rodata .bss`, `.symtab`, `.strtab`, `.rela.*`.
- Relocations needed: `R_RISCV_32`, `R_RISCV_BRANCH`, `R_RISCV_JAL`, `R_RISCV_HI20`, `R_RISCV_LO12_I`, `R_RISCV_LO12_S`, `R_RISCV_PCREL_HI20`, `R_RISCV_PCREL_LO12_I/S`, `R_RISCV_CALL` and `R_RISCV_CALL_PLT` (what clang and merlin-as emit for `call`; without shared libraries it means the same). No linker relaxation: `R_RISCV_RELAX` hints are ignored, `R_RISCV_ALIGN` is an error (build with `-mno-relax`).
- Executable: `ET_EXEC`, program headers only for `PT_LOAD`, entry point in `e_entry`.

Why ELF rather than a custom format: it costs little more to write, and it means clang/lld, `llvm-objdump` and `llvm-readelf` can check our output and produce reference files, and riscv-tests ELFs load directly. A custom format can still be added later as an exercise.

All reading goes through one crate, `elf32` (header, program headers, sections, symbols, `.rela`/`.rel` entries, RISC-V relocation names). Grail's loader and merlin-objdump use it; merlin-ld still reads its input objects with its own `src/object.rs` and should move to it. Writers stay in each tool.

### Assembler (`merlin-as`)

`merlin-as [-o out.o] input.s`. The reference is clang's integrated assembler with `-march=rv32ima_zicsr_zifencei -mno-relax` (the examples use no A instructions and are compared with `rv32im_zicsr_zifencei` builds): for the same source, `llvm-objdump -dr` must print the same text for both objects (checked for every source in `examples/`, which clang assembles with `-include examples/xcal.inc`), and every encoding matches LLVM's (`crates/merlin-as/tests/golden/encode.golden`). The encoder, the pseudo-instruction expansions and `li`'s choice of sequence are documented in `crates/merlin-as/ENCODER.md`.

- **Single pass.** No instruction's size depends on a label's address; `.equ`/`.set` constants must be defined before use. Every label reference is first recorded as a fixup with its immediate left zero.
- **Local labels are resolved by the assembler**, as clang does with `-mno-relax`. When the file has been read, a pc-relative reference to a label that is local (not `.globl`, also when `.globl` comes later in the file) and defined in the same section is patched in and gets no relocation: branches, `jal`/`j`, `call`/`tail` (both words), and `auipc` + `%pcrel_lo` pairs (`la`, `lla`, `lw rd, sym`, `sw rs, sym, rt`, explicit `%pcrel_hi`/`%pcrel_lo`). A branch or jump out of range is an error on its line. Everything else stays a relocation for the linker: references to global or undefined symbols, to labels in other sections, and absolute ones (`%hi`, `%lo`, `.word sym`).
- **`.`** is the location counter: the address of the instruction (its first word, for a pseudo-instruction) or data word it appears in. Each use becomes a hidden local label there, so `j .`, `beq a0, a1, .-8` and `la a0, .` resolve like any local label, and `.word .` gets an `R_RISCV_32` against that label (clang's `.L0`).
- **Symbols**: `.L` labels (numeric labels like `1:`, hidden `.Lpcrel_hiN` labels on `auipc`s, `.Ldot$N` labels for `.`) are written to `.symtab` only when a relocation refers to them, as in clang.
- **`.insn`** spells out an instruction by fields, with LLVM's forms and encodings: `.insn r opcode, funct3, funct7, rd, rs1, rs2`, `.insn i opcode, funct3, rd, rs1, imm` (or `rd, imm(rs1)`), `.insn s opcode, funct3, rs2, imm(rs1)`, `.insn b opcode, funct3, rs1, rs2, target`, `.insn u opcode, rd, imm20`, and `.insn value` / `.insn 4, value`. Opcodes must be 32-bit ones (low bits `11`). The examples write the Xcal instructions as mnemonics (`xcal.dbg a0`); clang does not know them, so `examples/xcal.inc` defines them as assembler macros that expand to the `.insn` with the same encoding, and clang reference builds pass `-include examples/xcal.inc`.
- **`.riscv.attributes`**: every object carries clang's `Tag_RISCV_arch` for `rv32ima_zicsr_zifencei` (`rv32i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0`), so llvm-objdump decodes M and A instructions without `--mattr`.
- **Differences from clang**, on purpose:
  - No branch relaxation. clang rewrites a conditional branch it cannot resolve or reach (to a global or undefined symbol, another section, or beyond ±4 KiB) into the inverted branch over a `jal`, even with `-mno-relax`; merlin-as keeps one instruction with `R_RISCV_BRANCH`, or reports the branch out of range. (llvm-mc 18's `-riscv-asm-relax-branches=0` behaves like merlin-as, and made the golden lines for such branches; LLVM 21 no longer has the option, so `scripts/gen-encode-golden.sh` keeps those lines and assembles the rest with clang.)
  - A numeric label that a relocation refers to is named `.L<n>$<k>` (the k-th definition of `n:`); clang names it `.Ltmp<m>`. No `$x`/`$d` mapping symbols are written.
  - `.insn j` is not accepted: LLVM 21 packs its numeric offset without the J-type bit scrambling.
  - Register names are never symbols: `lw a0, a1` is an error, where LLVM reads `a1` as a symbol.
  - Some operand forms LLVM accepts are not: `jalr rd, rs` and `jalr off(rs)` (write `jalr rd, 0(rs)`, `jalr ra, off(rs)`), `jr off(rs)` and `jr rs, off` (write `jalr zero, off(rs)`), `call rd, sym`, `jump sym, rt`, `%hi(sym)` in `.insn u`, named opcodes in `.insn` (`.insn r OP, ...`), upper-case CSR names (`MSTATUS`) and standard CSRs Excalibur doesn't have (`fflags`). The other way round, `call`/`tail` accept `sym+offset` and `.`, where clang wants a bare symbol name.

### Linker (`merlin-ld`)

`merlin-ld a.o b.o [-T link.ld] [-e entry] [-o out.elf] [--Map out.map]`. The reference is ld.lld: for the same objects and script, the allocated sections (names, types, addresses, sizes, alignments, A/W/X flags), `PT_LOAD`s and symbol values must match, and so must every loaded byte, except for the differences listed at the end of this subsection. This is checked against ld.lld links committed with their `llvm-readelf` and `llvm-objdump -d` output (`crates/merlin-ld/tests/fixtures`, regenerated with `cargo xtask ld-fixtures`), including layout edge cases (empty and assignment-only output sections, gaps, `NOBITS` before `PROGBITS`) and every relocation type at the edges of its range, and by relinking every riscv-tests program from its object (`tests/riscv-tests/obj`) and comparing with the committed ld.lld executable.

- **Inputs**: `ET_REL`, `EM_RISCV`, ELFCLASS32, little-endian, `e_flags` 0 (ilp32 soft-float, no RVC). Allocated sections must be `PROGBITS` or `NOBITS`. Rejected with an error: `SHT_REL`, section groups/COMDAT, TLS, common symbols (`-fno-common`), extended section indices. Non-allocated sections (`.riscv.attributes`, `.comment`, debug info) are dropped. Mergeable sections (`SHF_MERGE`, e.g. clang's `.rodata.str1.1` for C string literals) are placed like any other section: see the differences below. Build clang objects with `-mno-relax`: there is no linker relaxation.
- **Symbols**: locals stay per object. Two strong definitions of a global are an error (`b.o: duplicate symbol 'foo' (first defined in a.o)`); a strong definition beats a weak one; an undefined weak symbol is 0. An undefined global nobody defines is an error (`a.o: undefined reference to 'foo'`). Symbols assigned in the linker script override object definitions (as in ld.lld).
- **Default layout** (no `-T`): `.text` at `0x8000_0000` (`.text.start` and `.text.init` first), then `.rodata` (+ `.srodata`), `.data` (+ `.sdata`), `.bss` (+ `.sbss`), each aligned to its most-aligned input. Other allocated sections need a script.
- **Linker scripts**, a subset (anything else is an error naming what it found): `OUTPUT_ARCH(riscv)`, `OUTPUT_FORMAT(elf32-littleriscv)`, `ENTRY(sym)`, `MEMORY` with one region, and `SECTIONS` containing `. = expr;`, `. += expr;`, `sym = expr;` and output sections `name : { *(pattern ...) ... } [> REGION]`. Expressions: numbers (`0x..`, decimal, `K`/`M` suffixes), `.`, symbols already placed, `ALIGN(n)`, `ORIGIN(r)`, `LENGTH(r)`, `+`, `-`, parentheses. Inside an output section `.` may only move forward with `ALIGN`, `+=` or an expression using `.`.
- **Placement**: in script order, the first `*(pattern)` that matches an input section takes it; within one `*(...)`, inputs keep command-line order and file order. An allocated input section no rule matches is an error if it is non-empty or defines a global (an empty one is left out). Each input is aligned to its `sh_addralign`, also when it is empty; an output section starts aligned to its largest input alignment. As in ld.lld, an output section is dropped only when no input section matched it (not even an empty one) and it contains no assignments; one that holds only assignments (`. += n`, `sym = .`) is `PROGBITS` with the flags of the previous output section that had inputs, so `.stack : { . += 4K; }` takes file space (a top-level `. += 4K;` reserves memory without it). The location counter starts at 0, also with `MEMORY` (a symbol assigned before the first output section gets 0, as in ld.lld). With `MEMORY`, every output section goes at the region's next free address, as in ld.lld, which means a top-level `. = ...` before a section would be ignored: merlin-ld reports it as an error instead (put the `ALIGN` inside the section). Overflowing the region is an error.
- **Entry**: `-e` (symbol or number), else `ENTRY(...)`, else `_start`. A missing `-e` symbol is an error; otherwise a warning, and the entry falls back to the first executable section.
- **Output**: `ET_EXEC`, one `PT_LOAD` per run of consecutive output sections with the same permissions (R, RX, RW), as ld.lld does with a `SECTIONS` command: address gaps and `NOBITS` sections inside a run do not split it but are written as zeros, so only `NOBITS` sections at the end of a run (`.bss`) give `p_memsz > p_filesz`. A run of empty sections has no `PT_LOAD`. Segment file offsets are congruent to their addresses mod 4 KiB (`p_align` 0x1000). Section headers and a `.symtab` (locals, then globals) are included for merlin-objdump and for Grail's `tohost` lookup.
- **Relocations**: after layout, every entry of every `.rela.*` section that patches a loaded section goes through one function, `reloc::apply(type, P, S, A, bytes, ctx)` (`crates/merlin-ld/src/reloc.rs`, which explains each formula and field), where `bytes` runs from the patched place to the end of its input section and `ctx` maps each `R_RISCV_PCREL_HI20` place to its S and A. The values follow the psABI:
  - `R_RISCV_32`: S + A, which must fit 32 bits (signed or unsigned).
  - `R_RISCV_BRANCH` / `R_RISCV_JAL`: S + A - P, even, within -4096..=4094 / -1 MiB..=1 MiB - 2.
  - `R_RISCV_HI20` / `R_RISCV_LO12_I` / `R_RISCV_LO12_S`: S + A split into `hi = (v + 0x800) >> 12` (for `lui`) and the low 12 bits, which `addi`/loads/stores sign-extend: the `+ 0x800` rounds `hi` up when bit 11 is set so the pair adds back to `v`.
  - `R_RISCV_PCREL_HI20`: S + A - P, split the same way (for `auipc`). `R_RISCV_PCREL_LO12_I/S`: its symbol is the label on the matching `auipc`, which must be in the same section; the low part is that of the `PCREL_HI20` found at the label's address (its S + A - P, not the low instruction's). Its own addend must be 0.
  - `R_RISCV_CALL` / `R_RISCV_CALL_PLT`: S + A - P split over the `auipc` and the `jalr` after it.
  - The `hi`/`lo` pairs are computed modulo 2^32, as RV32 executes them, so they reach every address and never overflow.
  - A weak reference that another object defines is that definition. A branch, jump or call to an undefined weak symbol targets itself (as in ld.lld: address 0 is usually out of reach); every other reference to it sees 0.
  - Errors name the file, section, offset and symbol: `a.o: relocation R_RISCV_JAL out of range for 'foo' at .text+0x10: 1048580 is not in [-1048576, 1048574]`. Misaligned targets, a field running past its section, a `PCREL_LO12` without its `PCREL_HI20`, `R_RISCV_ALIGN` and types outside this list (GOT, TLS, compressed, `ADD`/`SUB`) are errors too.
- **Differences from ld.lld**, on purpose:
  - `SHF_MERGE` sections are not merged. ld.lld keeps one copy of identical strings and constants (two C files using the same string literal share it) and orders the pieces by hash; merlin-ld concatenates the input sections in order. When an output section holds duplicates it is larger than ld.lld's and every address after it differs, and merlin-ld warns (`.rodata: mergeable (SHF_MERGE) input sections hold N bytes of duplicate strings or constants; ...`). Without duplicates the sizes match, but a symbol inside a merged string section can still have a different address.
  - With `MEMORY`, a top-level `. = ...` that moves `.` before an output section is an error (ld.lld ignores it), see Placement.
  - Script-assigned symbols are `SHN_ABS` in `.symtab`; ld.lld makes them relative to a nearby output section. Their values are the same.
  - Output section flags are only `SHF_ALLOC`, `SHF_WRITE` and `SHF_EXECINSTR` (ld.lld also carries `SHF_MERGE`/`SHF_STRINGS` over from merged inputs).
  - Unmatched allocated sections are errors, not orphans placed by heuristics. `/DISCARD/`, `NOLOAD`, `AT`, and output section addresses or attributes (`.sec ALIGN(16) : { }`) are not supported.
  - Only `PT_LOAD` program headers (no `PT_GNU_STACK` or `PT_RISCV_ATTRIBUTES`), and no `.riscv.attributes` or `.comment` in the output. Without `.riscv.attributes`, llvm-objdump does not know the target has the M extension and prints `mul`/`div`/... in a merlin-ld executable as `<unknown>`: compare the two disassemblies with `llvm-objdump -d --mattr=+m` (the bytes are the same). File offsets of empty and `NOBITS` sections may differ. Undefined weak symbols are left out of `.symtab`.
  - A `PCREL_LO12` with a non-zero addend is an error (ld.lld warns and ignores the addend), and an `R_RISCV_32` value that does not fit 32 bits is an error (ld.lld truncates it).

### Disassembler output

`merlin-objdump` takes llvm-objdump's options (`-d -r -t -h -x -f -p -s`, `-M no-aliases`, `--no-show-raw-insn`) and prints the same text as llvm-objdump 21, so the two can be diffed: same layout, pseudo-instructions where LLVM prints them (`mv`, `ret`, `beqz`, `csrr`, ...), hex immediates, branch targets as `0x80000044 <puts+0x28>`, `jalr` after `auipc` resolved to `<symbol>`, and relocations under their instruction with `-dr`. The deliberate differences:

- Xcal instructions print as `xcal.dbg`/`xcal.halt` and undecodable words as `.word 0x...` (trailing odd bytes as `.byte`), where LLVM prints `<unknown>`.
- Reserved fence encodings (non-zero `fm`, `rs1` or `rd`, and `fence.i` with operand bits set) print as the `fence`/`fence.i` Grail executes. Like LLVM, the disassembler still forgets the `auipc` values it tracks at them.
- `dret` (debug mode, which Excalibur does not have) prints as `.word 0x7b200073`.
- A branch or jump target below the symbol it is named after (a negative target that wraps around in a `.o`) gets a 32-bit displacement, `<f+0xffffff04>`, where LLVM prints a 64-bit one, `<f+0xffffffffffffff04>`.
- CSR names come from `excalibur::csr`; CSRs not listed there print as numbers.

The instruction formatting itself (`Inst::format` in `excalibur`) is shared with Grail's `--trace`, which uses the plain form: no pseudo-instructions, decimal immediates, relative branch offsets.

### Bootstrapping

Until our compiler can build the kernel, clang (`--target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -ffreestanding -nostdlib`, native Windows LLVM build) is the reference compiler. Each of our tools is validated by swapping it in for the LLVM one and diffing behavior:

1. Our assembler output linked by `ld.lld` → must run the same.
2. clang `.o` files linked by our linker → must run the same.
3. Our compiler's `.s` → our assembler → our linker → emulator.

Steps 1 and 2 together, our assembler and our linker with no LLVM at all, is how every example is built (below).

### Examples

The programs in `examples/` are built by `cargo xtask examples` (`crates/xtask/src/examples.rs`): each source is assembled by merlin-as and the objects are linked by merlin-ld, with the example's `link.ld` (or the default layout for `muldiv`). Both tools run as libraries inside xtask, the same code as their command lines, which the comment at the top of each source spells out. The linked `examples/<dir>/<name>.elf` files are committed so Grail can run them without building anything, and `cargo test` (`crates/xtask/tests/examples.rs`) rebuilds every example from source, requires the result to equal the committed file byte for byte, and runs it in Grail with its input (UART bytes, key events, pointer input, a disk image, another machine on a hub), checking the output and the exit code. Nothing in that needs LLVM. `examples/multiobj` exists to exercise multi-object linking: two sources with calls, pc-relative addresses and `.word` pointers between them, `.data` written at run time, `.bss` in the other object, and a non-zero exit code (42).

clang and ld.lld are only the reference. Each source carries the command that builds the same program with them:

```
clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
      -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
      -T examples/hello/link.ld examples/hello/hello.S -o hello.elf
```

`-mno-relax` and `--no-relax` because our tools do no relaxation; `--discard-none` because the clang driver passes `-X` to ld.lld, which drops the `.L` labels (`.Lpcrel_hi1`) that merlin-ld keeps; `-include examples/xcal.inc` for the Xcal mnemonics (see `.insn` above). For `hello` and `multiobj` this link is committed (`crates/merlin-ld/tests/fixtures/{hello,multiobj}`, from `cargo xtask ld-fixtures`), and the merlin-built executables are compared with it: the entry point, every `PT_LOAD` (addresses, sizes, every byte), the allocated section headers and the symbols (names and values) are identical, and merlin-objdump's disassembly is llvm-objdump's text for ld.lld's file. The rest of the file differs, as listed under "Differences from ld.lld" above:

- ld.lld writes `.riscv.attributes` and `.comment` sections and `PT_GNU_STACK` and `PT_RISCV_ATTRIBUTES` program headers; merlin-ld writes only `PT_LOAD`s and no non-allocated sections besides `.symtab`, `.strtab` and `.shstrtab`.
- clang's objects carry `$x`/`$d` mapping symbols, which ld.lld copies to the executable; merlin-as writes none.
- `__stack_top` (assigned by the script) is `SHN_ABS` in merlin-ld's `.symtab` and relative to an output section in ld.lld's. The value is the same.
- The non-allocated sections come in another order (`.symtab .strtab .shstrtab` against ld.lld's `.symtab .shstrtab .strtab`), so the section header table and those sections sit at other file offsets and the file sizes differ.

With relaxation on (clang's default, which is how the examples used to be built), ld.lld also shrinks every `call` whose target is in reach of a single `jal` from 8 bytes to 4, so everything after the first call moves: in `hello`, `puts` is at 0x8000001c instead of 0x80000020 and `xcal.dbg` reports pc 0x80000014 instead of 0x80000018. That build is kept as a merlin-objdump test fixture (`crates/merlin-objdump/tests/fixtures/hello.elf`).

### Kernel build

The kernel (`kernel/`) is built by `cargo xtask kernel`, a Rust build task (`crates/xtask`), not a Makefile or shell script: clang compiles each `.c`, clang or merlin-as (`--as merlin`, step 1 above) assembles each `.s`, and `ld.lld` links them with `kernel/kernel.ld`. The full command lines are in `kernel/README.md`. The linked `kernel/kernel.elf` is committed, so running and testing the kernel needs no LLVM; the build is reproducible, and when the installed clang matches the one recorded in `kernel/kernel.elf.clang-version`, `cargo test` checks the committed image is up to date. The user programs the kernel embeds (`kernel/user/`, see "Kernel processes") are built first, by our own tools: `cargo xtask kernel` assembles each with merlin-as, links it with merlin-ld and writes the executables as data into `kernel/userprogs.s`, a generated file that is committed like `kernel.elf` (and that `cargo test` checks is current, without LLVM).

Boot contract: Grail's boot ROM jumps to `_start` at 0x8000_0000 in M-mode. `_start` points `mtvec` at an early trap handler, sets `sp` to the top of a 16 KiB stack, zeroes `.bss` (`__bss_start`..`__bss_end`, both 4-byte aligned), and calls `start()`, the M-mode setup below, then executes `mret`, which enters S-mode at `supervisor_entry`. That calls `int kmain(void)` in S-mode; when `kmain` returns, the machine call `MCALL_HALT` stops the machine with its return value as the exit code. Until `start()` installs the M-mode trap handler, any trap prints `mcause`, `mepc` and `mtval` with `xcal.dbg` and halts with exit code 0xFF. `.text` starts at `__text_start` (= `_start`), and `.rodata` (`__rodata_start`) and `.data` (`__data_start`) each start on a page boundary, so the kernel page table can map them with different permissions. `__kernel_end` (page aligned) marks the first free byte of RAM after the image.

### Kernel traps

The kernel runs in S-mode on top of a small M-mode layer (the xv6-riscv design).

`start()` (M-mode, `kernel/start.c`):

- `medeleg` = every exception except ecall from S-mode (0xB1FF), so all faults go to the kernel and ecall from S-mode is left for machine calls. `mideleg` = SSI, STI, SEI (0x222).
- `stvec` = `kernel_vec`, set before S-mode starts, so a fault in the kernel's very first instruction is handled.
- `mtvec` = `machine_vec`; `mscratch` points at `machine_scratch`, a few words of scratch space (the M-mode handler has no stack) plus the timer interval.
- `mtimecmp` = `mtime` + interval; `mie` = MTIE only.
- `mstatus.MPP` = S, `mepc` = `supervisor_entry`.

`machine_vec` (M-mode, `kernel/machine.s`) handles the only traps left in M-mode:

- Machine timer interrupt: `mtimecmp += interval` (so ticks don't drift), set `mip.SSIP`, `mret`. The kernel sees each tick as a supervisor software interrupt.
- Machine call (ecall from S-mode): function number in a7, arguments in a0-a6, result in a0; `mepc` advances past the `ecall`. Function 0 (`MCALL_HALT`) stops the machine with exit code a0, since `xcal.halt` is M-mode only. Any other function returns -1.
- Anything else is a kernel bug: print `mcause`/`mepc`/`mtval` and halt with 0xFF, like the early trap handler.

Timer: the tick rate is 10 Hz, an interval of `timebase / 10` `mtime` ticks (at least 1). The timebase comes from sysinfo's `SYS_TIMEBASE_HZ` (section 9.5) through one function, `timebase_hz()`, which reads it on its first call (`timer_init`, in M-mode) and keeps it, so S-mode code never touches the register again (it stays valid once the kernel turns paging on). If `SYS_MAGIC` does not read "SYSI", or `SYS_TIMEBASE_HZ` reads 0, it falls back to the nominal 10 MHz (section 7).

`kernel_vec` (S-mode, `kernel/kernelvec.s`) saves x1-x31 and `sepc`, `scause`, `stval`, `sstatus` in a `struct trap_frame` on the current stack (`regs[32]` with x*n* at offset 4*n*, then the four CSRs at 128-140; 144 bytes), calls `kernel_trap(frame)`, restores `sepc`, `sstatus` and the registers from the frame, and returns with `sret`. `stvec` points at `kernel_vec` only while the hart runs in S-mode; traps from U-mode go to `user_vec`, which uses `sscratch` to find the process's kernel stack ("Kernel processes").

`kernel_trap` (`kernel/trap.c`): a supervisor software interrupt clears `sip.SSIP` and increments `ticks`. A supervisor external interrupt (cause 9) claims the source from PLIC context 1, services it (the UART's receive handler, see "Kernel console") and completes it; `sip.SEIP` follows the PLIC's line, so there is nothing to clear. An exception at one of the memory probes' load or store (see "Kernel memory") resumes the probe instead. Any other trap is a panic: it prints

```
panic: <cause name>
  scause 0x........  sepc 0x........  stval 0x........  sstatus 0x........
```

then, for a page fault (cause 12, 13 or 15) with paging on, how the active page table maps `stval` (the kernel's mappings are the same in the kernel page table and in every process's): `  va 0x........ is not mapped` or `  va 0x........ maps to pa 0x........, pte 0x........ DAGUXWRV`, the flags printed bit 7 to bit 0 with `-` for a clear bit. Then x1-x31 four per line (`   ra 01010101   sp 80007400 ...`), and it halts with exit code 0xFE. After its self-tests the kernel sets up the PLIC and enables its interrupts (`sie.SSIE`, `sie.SEIE`, `sstatus.SIE`), lists the disks (`disk: none` without any, "Kernel disks" below), idles in `wfi` printing `tick 1` ... `tick 10`, runs the disk self-test if there is a disk, then runs its shell ("Kernel console" below) and returns when told to `halt`.

For testing the panic path, the image has a word `boot_fault` in `.data`, normally 1. Patching it to 2 makes the kernel execute an illegal instruction after the ticks (with every register but sp set to its number in each byte, x5 = 0x05050505); 3 makes it load from the unmapped address 0x100, a load page fault since paging is on; 4 makes it store to `kmain`'s first instruction, a store page fault since kernel text is read and execute only. Values 5 to 11 misuse the memory API after the ticks, and each must end in a plain `panic: <message>` from the check that catches it: 5 maps the first kernel text page again (`map_pages: remap`), 6 unmaps page 0 (`unmap_pages: not mapped`), 7 maps a page with W but not R (`map_pages: bad permissions`), 8 plants a 4 MiB leaf in the root table and walks it (`walk: unexpected megapage`), and 9, 10 and 11 `kfree` a page pointer plus 4, the kernel's first page and the page after the end of RAM (`kfree`, after a `kfree: bad page <pa>` line).

Kernel C stays inside what Merlin will compile: no libc headers (`-nostdinc`), no floating point, no inline `asm()` (CSR access and Xcal instructions live in `.s` files, with Xcal instructions written as `.word`), and `.s` files use only merlin-as syntax. The one exception is varargs: `kprintf` uses `__builtin_va_list`/`__builtin_va_start`/`__builtin_va_arg`/`__builtin_va_end` (via `kernel/include/varargs.h`), so Merlin must provide those builtins with the ILP32 convention (variadic arguments travel in a0-a7 and then on the stack like named ones; the callee spills a0-a7 next to its stack arguments and `va_list` is a pointer walking that area).

### Kernel memory

The kernel manages physical memory in 4 KiB pages and runs with Sv32 paging on (`kernel/kalloc.c`, `kernel/vm.c`), the xv6-riscv design.

**RAM** comes from sysinfo (section 9.5), not from a fixed size or a boot argument: `kinit()` checks `SYS_MAGIC` and `SYS_VERSION` (1), reads `SYS_RAM_BASE` and `SYS_RAM_SIZE`, and panics unless the image starts RAM (`__text_start` = `SYS_RAM_BASE`) and RAM is a whole number of pages. So the allocator follows `--ram`. With 2 GiB, RAM ends at 2^32, which wraps to 0 in 32 bits: the kernel counts pages and compares `pa - ram_base < ram_size` instead of computing an end address.

**Page allocator**: one free list threaded through the free pages (each free page's first word points to the next), holding every page from `__kernel_end` to the end of RAM. `kalloc()` pops a page (NULL when none is left), `kfree(pa)` pushes one and panics unless `pa` is a page-aligned address in that range, `kzalloc()` is `kalloc()` plus zeroing, and `kfree_pages()` is the free count. Unlike xv6, pages are not filled with junk on free or alloc: at about a thousand stores a page it would dominate the boot.

**Page tables**: `pagetable_t` is a page of 1024 PTEs. `walk(pt, va, alloc)` returns the level-0 PTE for `va`, creating the second-level table (a zeroed page) when `alloc` is set. `map_pages(pt, va, pa, size, perm)` maps a page-aligned range with permissions R/W/X/U (W without R and remapping a valid PTE panic), returning -1 if a table can't be allocated. `unmap_pages(pt, va, size, free)` clears mapped PTEs (panics on an unmapped page) and optionally frees the pages. `pagetable_free(pt)` frees the second-level tables and the root, and panics if a leaf is still mapped. The kernel only creates 4 KiB leaves: no megapages, so a 4 MiB leaf in a root table is treated as corruption. PTEs are created with A and D clear; Grail sets them (section 5), so the kernel never has to.

**Kernel address space**: identity mapped (virtual = physical), so physical addresses (page tables, pages from `kalloc`) stay usable as pointers. `kvminit()` maps

| Range | Permissions |
|---|---|
| `__text_start` .. `__rodata_start`: kernel text | R+X |
| `__rodata_start` .. `__data_start`: read-only data | R |
| `__data_start` .. end of RAM: data, `.bss`, boot stack, free pages | R+W |
| every device in the sysinfo device table, rounded out to whole pages; a page that devices share is mapped once | R+W |

and nothing else: no U, no G, nothing executable outside the kernel text, and the boot ROM, the gaps and address 0 unmapped, so a stray access is a page fault. The devices come from the table rather than from constants, so a new Grail device is mapped without a kernel change. `kvminithart()` then executes `sfence.vma`, writes `satp` (MODE = Sv32, PPN = root), and `sfence.vma` again. Grail has no TLB, so the fences cost nothing there, but the ISA requires them. M-mode code (`machine_vec`) ignores `satp` and keeps using physical addresses.

Boot order in `kmain`: UART banner and `kprintf` self-test, `kinit()`, `kvminit()`, `kvminithart()`, then the memory self-tests, then `trap_init()`, `blk_init()` and `binit()` ("Kernel disks"), the ticks, the disk self-test and the shell, which run with paging on. Setting up 128 MiB takes about 4.5 million instructions, nearly all of them freeing, mapping and (in the self-test) allocating one page at a time; a few machine timer ticks fire meanwhile and pile up in `sip.SSIP`. `trap_init()` clears that bit before it enables the interrupt, so the first tick the kernel counts is a whole interval long, like the others.

**Memory probes** (`kernel/probe.s`): `probe_load(addr, &val, &stval)` and `probe_store(addr, val, &stval)` make one word access and return 0, or the trap cause with `stval` when it faults. `kernel_trap` recognises an exception whose `sepc` is a probe's access instruction, sets a0 = `scause` and a1 = `stval` in the frame and resumes at `probe_fault`, which stores `stval` and returns (an exception table, as in Linux). The self-tests use them to expect faults. Any other page fault in S-mode panics; a fault in U-mode kills the process instead ("Kernel processes"). The kernel reads user memory through the process's page table rather than through probes (`copyin`).

**Self-tests** (`kernel/vmtest.c`), printed as `vm self-test: N/M passed` and counted in `kmain`'s exit code:

- allocate every free page (each aligned and after the kernel) until `kalloc()` returns NULL, check the count, check that `pagetable_create()` and a `map_pages()` needing a new table fail while memory is exhausted, then free every page and check the count again;
- fill a page with all ones (every word a valid-looking PTE) and free it, then check that `kzalloc()` hands it back zeroed; the same for `pagetable_create()`, which `walk()` also uses. RAM starts out zero on Grail, so only a reused page shows the zeroing;
- map a page a second time at 0x4000_0000, write through each address and read through the other, check the hardware set A and D in the PTE, unmap it (freeing the page) and check a load and a store there are page faults 13 and 15 with `stval` = the address;
- a store to the first word of the kernel text and one to `.rodata` are store page faults (cause 15) caught by the probe, the text is unchanged and readable, and a store to `.data` works;
- build a separate page table, map two pages across a 4 MiB boundary and a three-page range, check the PTEs with `walk()`, unmap, free the table, and check every page came back.

The alias test leaves its second-level table in the kernel page table; everything else ends as it started.

### Kernel console

Console input is interrupt-driven; nothing in the kernel waits on RBR.

- **PLIC** (`kernel/plic.c`): `plic_init()` sets context 1's (hart 0, S-mode) threshold to 0 and calls `plic_enable(1)` for the UART's source (section 7.1): `plic_enable(src)` gives a source priority 1 and sets its bit in context 1's enable register, keeping the others (the disk driver enables each disk's source the same way). Context 0 (M-mode) is left alone and `mie.MEIE` stays clear: every device interrupt goes to the kernel. A claimed source other than the UART's goes to the disk driver's handler.
- **UART** (`kernel/uart.c`): `uart_init()` sets 8N1 and IER.ERBFI, so the UART holds its line high while a byte waits. `uart_intr()`, called for source 1, reads RBR until LSR.DR clears into a 128-byte ring buffer. The ring has one writer per index (the handler advances the head, the reader the tail), so it needs no lock. If it fills up, the handler leaves the remaining bytes in the UART and clears IER instead of dropping them (a byte left in RBR would keep the line high and the interrupt firing). `uart_rx_pop()` sets IER again once the ring is half empty. Transmit still polls LSR.THRE, which Grail always reports as set.
- **Console** (`kernel/console.c`): `console_getc()` pops a byte, or sleeps in `wfi` with interrupts off from the check to the `wfi` (the interrupt that wakes it is taken right after). `console_readline(buf, size)` echoes printable characters (0x20-0x7E) and keeps at most `size - 1` of them. BS (0x08) and DEL (0x7F) erase one, echoing BS, space, BS. CR, LF or CR LF ends the line and echoes a LF; a LF right after a CR is skipped, so both raw consoles (CR) and piped text (LF, CR LF) work. Other control characters are ignored.
- **Shell** (`kmain`, until there is a user-space one): prints `Type help for the list of commands.`, then reads lines after the prompt `camelot> `. It knows `help`, `ticks` (prints `ticks: N`, the timer ticks since boot), `run` (runs user programs, "Kernel processes" below) and `halt` (returns from `kmain`, so the exit code is the self-test failure count, 0 normally). Blank lines are skipped, surrounding blanks ignored, and anything else prints `unknown command: <line> (try help)`. Lines are up to 63 characters.

Host side: the `grail` command feeds its stdin to the UART. When stdin is an interactive terminal (a Windows console, or a Linux terminal), Grail switches it to raw mode while it runs: each key is passed on as typed, the terminal does not echo it (the kernel does), and Ctrl-C still stops Grail. The terminal is restored at exit and on Ctrl-C. With a pipe or file nothing changes, so `printf 'ticks\nhalt\n' | grail kernel/kernel.elf` runs the shell non-interactively. Such input must end with `halt` (or the run needs `--max-steps`): when it runs out, the kernel just waits at the prompt for more, taking timer ticks, and Grail never exits. In a raw Windows console a lone Ctrl-Z is passed on as 0x1A like any other key (the shell ignores it), not taken as the end of the input. Without `--clock`, a kernel waiting for input still takes every timer tick (`wfi` skips to the next one), so the tick count grows by far more than 10 a second; `--clock 10MHz` makes it real time.

### Kernel processes

User programs run in U-mode, each in its own address space, and take turns on the CPU, one timer tick at a time (`kernel/proc.c`, `kernel/uservec.s`, `kernel/swtch.s`, `kernel/elf.c`, `kernel/syscall.c`). The design follows xv6-riscv for one hart, with one difference in the address space, below.

**Address space.** User space is the low 4 MiB, `0` to `USER_TOP` = 0x0040_0000: exactly root entry 0 of an Sv32 page table. Page 0 is never mapped, so a null pointer faults; programs are linked at 0x1000 (`kernel/user/user.ld`); the stack is the last page, 0x003F_F000-0x003F_FFFF, with `sp` = `USER_TOP` at the start, and the page under it is an unmapped guard page, so program segments must end at or below 0x003F_E000. Every other root entry of a process's page table is a copy of the kernel page table's: the process shares the kernel's second-level tables, so the kernel's mappings (text, data, all of RAM and the devices, none with U) are present in every address space without being copied or freed per process. The kernel's own root entry 0 (only the test device lives there, which the kernel doesn't use) is replaced by the process's. This is why the kernel must not add root entries once processes exist.

Why a shared kernel half rather than xv6's trampoline page: the kernel is identity mapped at 0x8000_0000 and up, far from user space, so the two never collide, and with the kernel mapped in every table a trap from U-mode needs no `satp` switch and no trampoline; the scheduler switches `satp` once per context switch. U-mode still can't touch the kernel, since no kernel PTE has U: a load from 0x8000_0000 in U-mode is a load page fault. `sstatus.SUM` stays clear, so S-mode can't touch user pages through user addresses either: the kernel reads user memory through the process's page table instead (`copyin(pt, dst, va, len)` walks it and copies from the physical pages, which the kernel's identity map makes reachable). `uvm_range_ok(pt, va, len, perm)` checks that every byte of a range is below `USER_TOP` and on a page mapped with V, U and `perm`; `copyin` copies only if the whole range passes, so a bad buffer copies nothing.

**Processes** (`struct proc`, `kernel/include/proc.h`): a slot in a table of 8 with a state (unused, runnable, running, sleeping, zombie), a pid (counting from 1), a name, the page table, a kernel stack (one page), a trap frame, a saved kernel context and, while sleeping, the channel it sleeps on. The trap frame is the same `struct trap_frame` `kernel_vec` uses, at the top of the kernel stack page; the process's kernel code runs on the rest of the page, below it.

**Kernel processes.** `kproc_create(name, fn, &err)` makes a process with only a kernel thread: it runs the C function `fn` in S-mode on its own kernel stack (the whole page: there is no trap frame), with interrupts off like any process's kernel code, and ends quietly (no `exited` line) when `fn` returns. It has no user space: no page table of its own (the scheduler runs it on the kernel page table) and nothing to free but its stack. It takes a slot and a pid like a user process. The kernel uses them for work that has to sleep, such as disk requests made outside a system call (the disk self-test, "Kernel disks").

**Trap entry and exit.** While a process runs in U-mode, `stvec` = `user_vec` and `sscratch` points at its trap frame. `user_vec` swaps `sscratch` into a0, saves every register (the user's a0 from `sscratch`) and `sepc`, `scause`, `stval`, `sstatus` in the frame, sets `sp` to the frame (so each trap from U-mode starts on an empty kernel stack) and calls `user_trap(frame)` in C. That first points `stvec` back at `kernel_vec`, and keeps interrupts off (the trap cleared `sstatus.SIE`) until the process is back in U-mode, so the kernel never takes an interrupt on a process's kernel stack. Then:

- supervisor software interrupt (the tick): `ticks` + 1 and `yield()`, the end of the time slice;
- supervisor external interrupt: serviced as in `kernel_trap`, and the process continues;
- ecall from U-mode (cause 8): `sepc` + 4, then `syscall(frame)`;
- any other exception: the process is killed (below).

`user_trap_return(frame)` goes back: interrupts off, `stvec` = `user_vec`, the frame's `sstatus` gets `SPP` = U (0) and `SPIE` = 1, then `user_return(frame)` (assembly) sets `sscratch` = frame, `sepc` and `sstatus` from the frame, loads every register (a0 last) and executes `sret`, which enters U-mode with interrupts on.

**Context switch.** `swtch(old, new)` (`kernel/swtch.s`) saves `ra`, `sp` and `s0`-`s11` in `*old` (`struct context`: ra at 0, sp at 4, s0-s11 from 8) and loads them from `*new`; the caller-saved registers are already assumed clobbered by any call. The scheduler runs on `kmain`'s boot stack. A new process's context has `ra` = `proc_start` and `sp` = its frame, so its first `swtch` lands in `proc_start`, which calls `user_trap_return` with a frame that holds only `sepc` = the ELF entry point and `sp` = `USER_TOP`.

**Scheduler.** `scheduler()` loops: turn interrupts on and off again (pending interrupts are taken in between), pick the next runnable process round robin starting from the slot after the last one that ran (from slot 0 at each call, so processes start in the order they were created), mark it running, write its page table to `satp` (with `sfence.vma`), and `swtch` to it. When the process gives the CPU back, it restores the kernel page table, and frees the process if it became a zombie. When no process is runnable but some sleep, it idles in `wfi` (interrupts off from the search to the `wfi`, which still wakes on a pending interrupt, taken at the top of the loop) until an interrupt wakes one; it returns, with interrupts on, when no process is runnable or sleeping. A process gives up the CPU only from its kernel thread: `yield()` on a tick (it stays runnable), `sleep()`, or when it exits or is killed (it becomes a zombie). Kernel threads are never preempted: a tick that arrives while one runs is taken by the scheduler afterwards.

**Sleep and wakeup** (xv6's, without locks). `sleep(chan)` marks the current process sleeping on `chan`, any address that stands for the event (the disk, a buffer), and switches to the scheduler; `wakeup(chan)` makes every process sleeping on `chan` runnable, and may be called anywhere, interrupt handlers included. A process's kernel code runs with interrupts off and is never preempted, so checking a condition and sleeping on it is atomic: the interrupt that changes it can only be taken once the process is asleep, and no wakeup is lost. Callers check their condition in a loop, since a wakeup only says it may have changed.

**Ending.** `exit` (the ecall) prints `pid N (name) exited: STATUS`. A fault in U-mode (any exception but ecall: a page fault, an illegal instruction such as `xcal.halt`, which is M-mode only, a misaligned access...) kills the process and prints

```
pid N (name) killed: <cause name>
  scause 0x........  sepc 0x........  stval 0x........
```

and, for a page fault, how the process's page table maps `stval` (the same line as a panic's). The kernel is unaffected. Freeing a process (`uvm_free`) frees every page mapped in user space, the user-space second-level table and the root, clears the copied kernel entries first so their tables stay, and frees the kernel stack: after a `run`, every page is back on the free list.

**Loading programs** (`elf_load`, `kernel/elf.c`). User programs are ELF32 executables embedded in the kernel's `.rodata` (`kernel/userprogs.s`, generated, see "Kernel build"); the table `user_programs` holds `{ name, image, size }` for each and `user_program_count` says how many. The loader treats an image as untrusted: it must be `\x7fELF`, 32-bit, little-endian, RISC-V, `ET_EXEC`, with 32-byte program headers (at most 16) inside the file; each `PT_LOAD` with a non-zero `p_memsz` must have `p_filesz <= p_memsz` and its file bytes inside the image, must be readable (`PF_R`), must lie in 0x1000..0x003F_E000, and must not share a page with another segment. Each page gets a fresh zeroed page from the allocator, the segment's file bytes copied in, mapped with U, R, plus W for `PF_W` and X for `PF_X`. The entry point must be 4-byte aligned (`sepc` would silently clear its low bits, starting the program somewhere other than its header says) and on a page mapped with U and X. A failure gives a reason (`not an ELF file`, `not a 32-bit little-endian RISC-V file`, `not an executable`, `bad program headers`, `segment outside the file`, `segment not readable`, `segment outside user space`, `segments share a page`, `misaligned entry point`, `entry point not in an executable segment`, or `out of memory`), and whatever was allocated is freed. The stack page is mapped after the segments, R+W+U and zeroed.

**Provisional ecalls** (`kernel/include/syscall.h`), until the system call ABI (#21): number in a7, arguments in a0-a1, result in a0. 1 = exit (status in a0; never returns). 2 = debug write: print the a1 bytes at user address a0 on the console and return a1, or return -1 without printing anything if any of them isn't readable user memory. Any other number returns -1. `xcal.dbg` works in U-mode too (section 9.1).

**User programs** (`kernel/user/`, assembled by merlin-as and linked by merlin-ld with `user.ld`; `lib.s` is linked into each and has `exit`, `write`, `puts`, `strlen` and a delay loop):

| Program | What it does |
|---|---|
| `hello` | prints `hello from user mode`, exits 0 |
| `ping`, `pong` | print `ping 1` ... `ping 4` (`pong ...`), half a tick (500 000 instructions) apart, exit 0; run together they alternate |
| `regs` | sets every register but t6 to its number in each byte, spins for about three ticks, exits 0 if all still hold their values, else with the first one's number: preemption must not change a register |
| `data` | checks that its whole stack page and its `.bss` (two pages, the second with no file bytes) read zero, that its `.data` word holds its initial value, and that `.data` and `.bss` take stores; then fills its stack and `.bss` with non-zero bytes, so the pages it frees are dirty for the next program; exits 0 or the failing check's number |
| `badptr` | writes from kernel memory, page 0, across `USER_TOP`, with a length that wraps, from the guard page, and makes an unknown ecall: each must return -1; then a good write must return its length; exits 0 or the failing case's number |
| `badload` | loads from 0x8000_0000: killed by a load page fault |
| `badstore` | stores to its own `.rodata` (mapped without W): killed by a store page fault |
| `badhalt` | executes `xcal.halt`: killed by an illegal instruction |

**The `run` command.** `run PROG...` looks every name up (an unknown one prints `run: no program called NAME` and starts nothing), creates a process for each in order (one that can't be created prints `run: cannot start NAME: REASON`, e.g. `too many processes` for the ninth), and calls `scheduler()`; the prompt returns when all have exited or been killed. `run` alone prints `programs:` and the names. Typed input that arrives meanwhile is kept for the shell (the UART interrupt is serviced from `user_trap`). For example:

```
camelot> run ping pong badload
ping 1
ping 2
pong 1
pong 2
pid 3 (badload) killed: load page fault
  scause 0x0000000d  sepc 0x00001004  stval 0x80000000
  va 0x80000000 maps to pa 0x80000000, pte 0x2000004b -A--X-RV
ping 3
ping 4
pong 3
pong 4
pid 1 (ping) exited: 0
pid 2 (pong) exited: 0
camelot>
```

### Kernel disks

The kernel drives Grail's disks (section 9.6) with an interrupt-driven driver (`kernel/blk.c`) and keeps recently used blocks in a buffer cache (`kernel/bio.c`), the layer a filesystem builds on. Both follow xv6-riscv (`virtio_disk.c`, `bio.c`), simplified for a device that takes one request at a time.

**Finding disks.** `blk_init()`, called after `trap_init()`, walks the sysinfo device table (section 9.5) and takes every entry of type 9 in table order: the first is disk 0 (with Grail, the first `--disk`). Base address, capacity and PLIC source all come from the table and the disk's registers, none from constants. An entry without a source (0) or with one above 31 is skipped, since the driver doesn't poll. For each disk it reads `BLK_CAPACITY` and `BLK_FLAGS`, acknowledges a leftover `BLK_STATUS.DONE` (so the line starts low), sets `BLK_CTRL.0` and enables the source with `plic_enable()`. The disks' pages are already mapped, as `kvminit()` maps every device in the table. `kmain` then prints a line per disk, `disk N: S sectors (K KiB), irq I`, with `, read-only` for a read-only disk, or `disk: none` when there is none, in which case the kernel boots as before.

**Requests.** `blk_read(dev, sector, buf, count)`, `blk_write(dev, sector, buf, count)` and `blk_flush(dev)` each run one command and return its `BLK_ERROR` code: 0 (`BLK_OK`) or one of the codes of section 9.6 (`BLK_ERR_BAD_COMMAND` to `BLK_ERR_IO` in `kernel/include/blk.h`); a refused command moves no data. `buf` is a kernel address, which the kernel's identity map makes the physical address `BLK_DMA` wants. Each disk has at most one request in flight: a process that finds the disk busy sleeps on the disk until the request in flight has been taken. The process then writes `BLK_SECTOR`, `BLK_DMA`, `BLK_COUNT` and `BLK_CMD`, and sleeps on the disk until the completion interrupt; nothing reads `BLK_STATUS`. Requests sleep, so they must come from a process (a system call, or a kernel process, "Kernel processes"): a request made outside one panics, as does a disk number that doesn't exist. Grail runs a command during the store to `BLK_CMD`, so its interrupt is already pending when the process goes to sleep and is taken as soon as the scheduler lets interrupts in; with a slower disk the scheduler runs other processes, or idles in `wfi`, until it comes.

**Interrupt.** A claimed PLIC source other than the UART's goes to `blk_intr(irq)`, which finds the disk with that source, reads `BLK_ERROR`, writes 2 to `BLK_STATUS` (clearing DONE lowers the line, so the source isn't pending again when the trap code completes it), marks the request done and wakes the processes sleeping on the disk. The process that made the request takes the error code, marks the disk free and wakes the next one waiting for it.

**Buffer cache.** A block is 1 KiB (`BSIZE`), two sectors: block `n` of a disk is its sectors 2`n` and 2`n` + 1. 1 KiB rather than one 512-byte sector, as in xv6: half the requests and buffers for the same data, and the filesystem's bitmap and inode blocks hold twice as much. The cache has 30 buffers (`NBUF`, xv6's number), each a `struct buf` (`kernel/include/bio.h`) holding a block's data, its disk and block number, a valid flag, a lock flag, a reference count and its place in a doubly linked list ordered by when it was last released. They live in `.bss`, so they are part of the kernel image's memory, not taken from the page allocator (`_start` spends about 35 000 instructions zeroing `.bss` since). The interface:

- `bread(dev, blockno)` returns the block's buffer, locked. A block is cached when a buffer holds it with valid data, or with users (its read is under way): that buffer is taken, waiting for its lock if another process holds it. Otherwise the least recently released buffer without users is taken over for the block. If the buffer's data isn't valid, `bread` reads the block with `blk_read`; `b->error` is then `BLK_OK` or the read's error code, and the data of a failed read stays invalid, so a failed read is never cached. Blocks from 2^31 on (whose sectors would pass 2^32) fail with `BLK_ERR_OUT_OF_RANGE` without a request. The counters `bio_hits` and `bio_misses` count the calls served from the cache and the ones that read the disk.
- `bwrite(b)` writes a locked buffer's data to its block at once and returns the error code. The cache is write-through, so it never holds data the disk doesn't: after a failed write the cached copy is marked invalid and the next `bread` reads what the disk holds.
- `brelse(b)` unlocks the buffer, wakes the processes waiting for it and, once nobody uses it, moves it to the front of the list, or to the back if its data isn't valid (its read or write failed): a buffer that holds nothing is reused before any cached block.

The lock is a sleep lock, `locked` plus `sleep()`/`wakeup()` on the buffer, held from `bread` to `brelse` and across the sleep in the driver, so each block has one copy in the cache and only its holder reads or changes it. `bread` panics when every buffer is in use, and `bwrite` and `brelse` panic on a buffer that isn't locked.

**Self-test** (`kernel/blktest.c`). With a disk attached, after the ticks (and the deliberate fault, if there is one), `kmain` tests disk 0 in kernel processes and prints `blk self-test: N/M passed`; a failed check prints `blk self-test FAILED: <check>` and counts in `kmain`'s exit code. A disk 0 with fewer than two blocks (4 sectors) is not tested (`blk self-test: disk 0 is too small, skipped`): one reader needs block 1, and the scratch block must not be block 0, which the readers check. The disk ends as it started:

- three kernel processes at once, two reading block 0 and one block 1. The first reader's request is in flight when the block-1 reader asks for the disk, so that one waits for it; the second block-0 reader finds the buffer locked, waits for the first to release it and gets the same bytes from the cache. Two disk reads in all;
- one kernel process: block 0 in the cache matches the disk (read past the cache with `blk_read`); the scratch block, the disk's last whole block, is read, written with a pattern (byte `i` = `i`·7 + 3), read back from the cache, read past the cache to check the disk holds the pattern, written back with its old contents and checked, and the disk is flushed. On a read-only disk the write must fail with `BLK_ERR_READ_ONLY`, the next `bread` must read the disk again, and the disk must be unchanged. Then the errors: the block just past the end fails with `BLK_ERR_OUT_OF_RANGE`, twice, since a failed read isn't cached, and a read into the boot ROM's address (not RAM) fails with `BLK_ERR_BAD_DMA`. Last, if the disk has at least `NBUF` + 1 blocks, LRU reuse: blocks 0 to `NBUF` - 1 are read in turn (filling every buffer), then block 0 again, then block `NBUF`, which must take block 1's buffer, the least recently used: blocks 0 and `NBUF` are then hits and block 1 a miss. (Evicting the most recently used buffer instead would drop block 0.)

On a 64 KiB read-write disk that is 16 checks (14 when it is read-only). `crates/xtask/tests/kernel.rs` runs it on in-memory disks and checks, instruction by instruction, that each command is written only once the previous result has been acknowledged in `blk_intr`, that `BLK_STATUS` is never read, which sectors were written with what, the sleeps on the disk and on a buffer, the cache counters and which buffer each miss took, and that a held-back interrupt leaves the kernel idle in `wfi` instead of spinning.

## 11. Roadmap

1. **Grail core**: RV32IM fetch/decode/execute, ELF loader, UART, test device, `xcal.dbg`/`xcal.halt`. Pass rv32ui/rv32um.
2. **Disassembler** (shared decoder) and an emulator trace mode.
3. **Privilege + traps**: CSRs, M/S/U, CLINT timer, Sv32. Pass rv32mi/rv32si. (Landed together with step 1.)
4. **merlin-as** emitting ELF `.o`. Validate with `llvm-objdump` and ld.lld.
5. **merlin-ld**. Validate against clang-produced objects.
6. **Bare-metal hello world** written in our assembly, built entirely with our tools.
7. **M extension** in merlin-as (Grail already passes rv32um), then the **A extension** (LR/SC and AMOs, section 4.1) in Grail and the toolchain. **Xcal devices** in Grail: framebuffer window, keyboard and pointer via Win32 FFI, sysinfo, block devices and network cards with pluggable backends, and a configurable machine (RAM size, real-time clock, disks, network cards) that a host can run several of.
8. **Merlin v0**: C subset → assembly. Grow it test by test (integer types, pointers, arrays, globals and strings done; structs, preprocessor and the rest tracked in #15).
9. **Camelot kernel** in C (clang first, our compiler as it catches up): trap handling, timer, UART driver, then paging (Sv32), processes, a disk driver and buffer cache, syscalls.

   **Phase 9c, networking** (Ethernet and IPv4; issues #80 to #85): the network card in Grail with pluggable backends (an in-process hub, pcap capture; section 9.8), virtual Ethernet switches in Avalon connecting its VMs, a user-mode NAT gateway (DHCP, DNS, internet access and host port forwarding, in std Rust with no host driver), and in the kernel a NIC driver, ARP, IPv4, ICMP, UDP, then TCP, then socket syscalls and network tools (`ping`, `ifconfig`, `nc`, `fetch`, `httpd`).
10. **Self-hosting stretch goal**: our tools running on our OS.
