# Excalibur ISA Spec v0 (draft)

Part of **Camelot**: the Excalibur ISA, the Grail emulator, the Merlin toolchain (merlin-as, merlin-ld, merlin), and the Camelot OS.

Status: draft. Base is stock RISC-V, plus the Xcal extensions in section 9.

## 1. Overview

- 32-bit, little-endian, load/store architecture.
- Base: **RV32I** (unprivileged spec 20191213), plus **Zicsr** and **Zifencei**.
- Planned extensions, in order: **M** (mul/div), **A** (atomics, needed once the kernel has locks), **C** optional and late.
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
| 0001111 MISC-MEM | `fence`, `fence.tso`, `fence.i` (reserved `fm`/rd/rs1 encodings execute as a plain `fence`; Grail is single-hart, so all are no-ops) |
| 1110011 SYSTEM | `ecall ebreak mret sret wfi sfence.vma csrrw csrrs csrrc csrrwi csrrsi csrrci` |
| 0001011 custom-0 | Xcal: `xcal.dbg`, `xcal.halt` (section 9) |
| 0101011 custom-1 | reserved for future Xcal extensions |

Misaligned loads/stores trap (address-misaligned exception) rather than being emulated. This keeps the emulator simple and is spec-legal. Consequence: riscv-tests `rv32ui-p-ma_data`, which assumes hardware misaligned support, is excluded.

## 5. Privilege and CSRs

Modes: U=0, S=1, M=3. Reset enters M-mode.

Minimum CSR set:

| CSR | Addr | Notes |
|---|---|---|
| mstatus | 0x300 | MIE, MPIE, MPP, SIE, SPIE, SPP, SUM, MXR |
| misa | 0x301 | RV32, I, M, S, U, X (read-only) |
| medeleg / mideleg | 0x302 / 0x303 | delegate traps to S-mode |
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

Exception causes implemented: 0 instr misaligned, 1 instr access fault, 2 illegal instruction, 3 breakpoint, 4 load misaligned, 5 load access fault, 6 store misaligned, 7 store access fault, 8 ecall from U, 9 ecall from S, 11 ecall from M, 12/13/15 page faults (with Sv32).

Interrupt causes: 1 SSI, 3 MSI, 5 STI, 7 MTI, 9 SEI, 11 MEI.

Interrupts are checked between instructions, priority MEI > MSI > MTI > SEI > SSI > STI.

Pending bits in `mip` come from the devices: MSIP and MTIP from the CLINT, MEIP from PLIC context 0, SEIP from PLIC context 1 (section 7.1). MEIP, MSIP and MTIP are read-only. SEIP is also software-writable: the value read is the OR of the software bit and the PLIC's signal, and `csrrs`/`csrrc` on `mip` modify the software bit only, so a read-modify-write never latches the PLIC's signal into it (as in the RISC-V privileged spec). STIP and SSIP are software-written.

## 7. Memory map

Layout follows the QEMU `virt` board's conventions so existing docs and examples apply, without needing QEMU.

| Range | Device |
|---|---|
| 0x0000_1000 - 0x0000_1FFF | Boot ROM (reset vector 0x1000, jumps to 0x8000_0000) |
| 0x0200_0000 - 0x0200_FFFF | CLINT: msip @+0x0, mtimecmp @+0x4000, mtime @+0xBFF8 |
| 0x0C00_0000 - 0x0FFF_FFFF | PLIC: external interrupts from the UART and keyboard (section 7.1) |
| 0x1000_0000 - 0x1000_00FF | UART, 16550-compatible subset (THR/RBR @+0, IER @+1, IIR @+2, LSR @+5) |
| 0x1000_1000 - 0x1000_1FFF | Reserved: virtio block device (later, for a filesystem) |
| 0x0010_0000 | Test/exit device: write 0x5555 = pass/exit, 0x3333 = fail (used by riscv-tests) |
| 0x3000_0000 - 0x3000_0FFF | Framebuffer control (Xcal, section 9.3) |
| 0x3000_1000 - 0x3000_1FFF | Keyboard (Xcal, section 9.4) |
| 0x3100_0000 - 0x3112_BFFF | Framebuffer VRAM |
| 0x8000_0000 - 0x87FF_FFFF | RAM, 128 MiB default (configurable) |

Unmapped accesses raise access-fault exceptions. Instructions and page tables live only in RAM and the boot ROM: an instruction fetch from VRAM or from any other range raises an instruction access fault, and a Sv32 page-table walk that reads a PTE outside them raises an access fault of the original access type. So neither can trigger a device register's read side effect (such as a PLIC claim).

Timer: `mtime` increments once per executed instruction, so runs are deterministic and reproducible. During `wfi`, Grail jumps `mtime` forward to `mtimecmp` instead of spinning. (A real-time mode can be added later.) MTIP is pending while `mtime >= mtimecmp`.

UART: writing THR prints the byte to the host terminal. RBR is fed from host stdin. LSR bit 5 (THRE) always 1, bit 0 (DR) set when input is available. IER bit 0 (ERBFI) enables the "received data available" interrupt: the UART holds PLIC source 1 high while IER bit 0 and LSR.DR are both set, so reading RBR until DR clears lowers it. IIR (@+2, read) reports 0x04 while that interrupt is asserted, else 0x01 (none). The other IER bits are stored but raise nothing (no transmit, line-status or modem interrupts).

### 7.1 PLIC

A minimal RISC-V PLIC (riscv-plic-spec) with the QEMU `virt` register layout: one hart, two contexts, sources 1-31.

| Source | Device |
|---|---|
| 0 | none ("no interrupt" when claimed) |
| 1 | UART receive (section 7) |
| 2 | Keyboard (section 9.4) |
| 3-31 | unused, never asserted |

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

- Pass `riscv-tests` rv32ui-p-*, rv32mi-p-*, rv32si-p-* (and rv32um-p-* once M is in). Tests are loaded as ELF and report via the test device (or `tohost` symbol).
- Prebuilt test ELFs are committed to the repo so no cross toolchain is needed on Windows.

## 9. Excalibur extensions (Xcal)

Everything here lives in RISC-V's reserved custom space (custom-0 opcode, new MMIO ranges), so stock RV32 software keeps working. `misa` bit 23 (X, "non-standard extensions present") is set.

### 9.1 Xcaldbg: debug print

```
xcal.dbg rs1        custom-0 (0001011), funct3=000, rd=0, imm=0
encoding: 0000_0000_0000 | rs1 | 000 | 00000 | 0001011
```

Prints the register to the emulator's console (stderr), e.g. `[dbg] pc=80000010 a0 = 0x0000002a (42)`. No architectural side effects. Legal in every mode. Meant for the first days of the kernel, before the UART driver exists.

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
- Grail runs everything on one thread: the CPU runs in slices that end early right after a `FB_PRESENT` write; between slices it pumps the window's messages and redraws on a present, when Windows asks for a repaint, or when ~16 ms of host time have passed. The window is opened only with `--fb` (Windows only; elsewhere Grail warns and runs headless, the default). The image is stretched to the window's client area. Closing the window stops the emulator (exit code 0); if the program stops first, the window stays open on the last frame until it is closed, and Grail then exits with the program's code.
- `--fb-dump DIR` writes `DIR/frame-NNNN.ppm` (binary P6, 640×480, numbered from 0000) after every `FB_PRESENT` and once more when the machine stops. It works with or without the window.

### 9.4 Keyboard (MMIO)

| Address | Register | Notes |
|---|---|---|
| 0x3000_1000 | `KBD_STATUS` (RO) | bit 0 = event available |
| 0x3000_1004 | `KBD_DATA` (RO) | pops one event: bits [7:0] key code, bit 8 = 1 on release, bit 9 = shift, bit 10 = ctrl. 0 if empty |
| 0x3000_1008 | `KBD_CTRL` | bit 0 = interrupt enable (PLIC source 2, section 7.1) |

Key codes: printable ASCII for printable keys, then 0x80+ for Enter (0x0A uses ASCII), arrows (0x80-0x83 up/down/left/right), Esc (0x1B), Backspace (0x08), F1-F12 (0x90-0x9B). The key events come from the framebuffer window; without a window, the UART remains the text input.

## 10. Toolchain (all our own)

Everything that runs on the host is written by us, with no third-party libraries doing the heavy lifting (std only). The target side is code produced by our own tools.

| Tool | Input → output | Notes |
|---|---|---|
| Assembler (`merlin-as`) | `.s` → `.o` | Full RV32I(+M, Zicsr, Zifencei, privileged, Xcal) mnemonics, standard pseudo-instructions (`li`, `la`, `call`, `ret`, `mv`, `j`, `beqz`...), `.insn`, directives (`.text .data .bss .section .globl .word .byte .ascii .align .equ`), `%hi/%lo/%pcrel_hi/%pcrel_lo`. See "Assembler" below |
| Linker (`merlin-ld`) | `.o` files → executable | Symbol resolution, relocation, section merging, simple linker script (or fixed layout: `.text` at 0x8000_0000) |
| Emulator (`grail`) | executable → runs it | Loads ELF directly, no boot loader needed early on |
| Disassembler (`merlin-objdump`) | `.o`/exe → text | Shares the decoder with the emulator; essential for debugging |
| Compiler (`merlin`) | C subset → `.s` | Emits assembly for our assembler. Starts small (int, pointers, structs, functions), grows toward enough C for the kernel |

### Merlin C subset

`merlin input.c -o output.s` compiles one translation unit; `--dump-ast` prints the checked syntax tree instead. Errors are reported as `file.c:line:col: error: message` (columns count characters from 1); lexing and parsing stop at the first error, name resolution reports all of them.

**v0** (a strict subset of C99: a v0 program whose behavior C defines means the same thing to clang; anything outside the subset is an error, never a different meaning):

- Types: `int` only (32-bit two's complement). No globals, pointers, arrays or structs yet; no preprocessor.
- Functions: `int f(int a, ...)` with at most 8 parameters, passed in `a0-a7` (ILP32 calling convention, result in `a0`). `int f(void)` and `int f()` both mean no parameters. Prototypes (`int f(int a);`) declare functions defined later or in another file.
- Names must be declared before use, as in C99: a call to a function not yet declared is an error (no implicit declarations), and so is a wrong argument count. Block scoping follows C: inner blocks and `for` initializers may shadow outer names; parameters share the scope of the function body's outermost block.
- Statements: blocks, `int` declarations with optional initializers (`int a = 1, b;`), expression statements, `if`/`else`, `while`, `for`, `return expr`, `break`, `continue`.
- Expressions, loosest first: `=` (right-associative, value is the assigned value), `||`, `&&`, `== !=`, `< <= > >=`, `+ -`, `* / %`, unary `- !`, then literals, variables, calls and parentheses. Literals are decimal, hex (`0x`) or octal (`0`), up to `2147483647` (`0x7FFFFFFF`): larger ones would be `unsigned int` or `long long` in C, so they are rejected until those types exist (write `INT_MIN` as `-2147483647 - 1`). Operators C lexes as one token, such as `++ -- += -> <<`, are rejected rather than split, so `--x` is never read as `-(-x)`.
- Statements and expressions nest at most 256 levels deep (each nested statement, `=`, unary operator, parenthesis, call argument or chained binary operator is a level); deeper input is an error.
- Where C leaves behavior undefined, merlin defines it the way the hardware does: arithmetic wraps, `/` and `%` round toward zero (`div`/`rem`, so `INT_MIN / -1` wraps and division by zero gives RISC-V's results), and falling off the end of a function returns 0. Argument evaluation order is unspecified; tests must not depend on it.

Test programs live in `crates/merlin/tests/cases/*.c`, each starting with `// expect: N` (the exit code); the ones in `crates/merlin/tests/cases-merlin/` rely on the definitions above for what C leaves undefined, so they are not checked against clang. The harness links them with a small `crt0.s` (set `sp` to the top of RAM, `call main`, `xcal.halt a0`) and runs them in Grail, so the exit code is the low 8 bits of `main`'s return value.

### Object and executable format

**ELF32 (little-endian, EM_RISCV)**, a subset:

- Relocatable `.o`: sections `.text .data .rodata .bss`, `.symtab`, `.strtab`, `.rela.*`.
- Relocations needed: `R_RISCV_32`, `R_RISCV_BRANCH`, `R_RISCV_JAL`, `R_RISCV_HI20`, `R_RISCV_LO12_I`, `R_RISCV_LO12_S`, `R_RISCV_PCREL_HI20`, `R_RISCV_PCREL_LO12_I/S`, `R_RISCV_CALL` / `R_RISCV_CALL_PLT` (the same computation; clang and merlin-as emit `_PLT` for `call`). No linker relaxation.
- Executable: `ET_EXEC`, program headers only for `PT_LOAD`, entry point in `e_entry`.

Why ELF rather than a custom format: it costs little more to write, and it means clang/lld, `llvm-objdump` and `llvm-readelf` can check our output and produce reference files, and riscv-tests ELFs load directly. A custom format can still be added later as an exercise.

All reading goes through one crate, `elf32` (header, program headers, sections, symbols, `.rela`/`.rel` entries, RISC-V relocation names). Grail's loader and merlin-objdump use it; merlin-ld still reads its input objects with its own `src/object.rs` and should move to it. Writers stay in each tool.

### Assembler (`merlin-as`)

`merlin-as [-o out.o] input.s`. The reference is clang's integrated assembler with `-march=rv32im_zicsr_zifencei -mno-relax`: for the same source, `llvm-objdump -dr` must print the same text for both objects (checked for `examples/hello/hello.S` and `examples/muldiv/muldiv.S`), and every encoding matches llvm-mc (`crates/merlin-as/tests/golden/encode.golden`). The encoder, the pseudo-instruction expansions and `li`'s choice of sequence are documented in `crates/merlin-as/ENCODER.md`.

- **Single pass.** No instruction's size depends on a label's address; `.equ`/`.set` constants must be defined before use. Every label reference is first recorded as a fixup with its immediate left zero.
- **Local labels are resolved by the assembler**, as clang does with `-mno-relax`. When the file has been read, a pc-relative reference to a label that is local (not `.globl`, also when `.globl` comes later in the file) and defined in the same section is patched in and gets no relocation: branches, `jal`/`j`, `call`/`tail` (both words), and `auipc` + `%pcrel_lo` pairs (`la`, `lla`, `lw rd, sym`, `sw rs, sym, rt`, explicit `%pcrel_hi`/`%pcrel_lo`). A branch or jump out of range is an error on its line. Everything else stays a relocation for the linker: references to global or undefined symbols, to labels in other sections, and absolute ones (`%hi`, `%lo`, `.word sym`).
- **Symbols**: `.L` labels (numeric labels like `1:`, hidden `.Lpcrel_hiN` labels on `auipc`s) are written to `.symtab` only when a relocation refers to them, as in clang.
- **`.insn`** spells out an instruction by fields, with LLVM's forms and encodings: `.insn r opcode, funct3, funct7, rd, rs1, rs2`, `.insn i opcode, funct3, rd, rs1, imm` (or `rd, imm(rs1)`), `.insn s opcode, funct3, rs2, imm(rs1)`, `.insn b opcode, funct3, rs1, rs2, target`, `.insn u opcode, rd, imm20`, and `.insn value` / `.insn 4, value`. Opcodes must be 32-bit ones (low bits `11`). This lets sources that use Xcal instructions (`examples/hello/hello.S`) build with both assemblers.
- **`.riscv.attributes`**: every object carries clang's `Tag_RISCV_arch` for `rv32im_zicsr_zifencei` (`rv32i2p1_m2p0_zicsr2p0_zifencei2p0_zmmul1p0`), so llvm-objdump decodes M instructions without `--mattr`.
- **Differences from clang**, on purpose:
  - No branch relaxation. clang rewrites a conditional branch it cannot resolve or reach (to a global or undefined symbol, another section, or beyond ±4 KiB) into the inverted branch over a `jal`, even with `-mno-relax`; merlin-as keeps one instruction with `R_RISCV_BRANCH`, or reports the branch out of range. (llvm-mc's `-riscv-asm-relax-branches=0` behaves like merlin-as; the golden encodings are made with it.)
  - A numeric label that a relocation refers to is named `.L<n>$<k>` (the k-th definition of `n:`); clang names it `.Ltmp<m>`. No `$x`/`$d` mapping symbols are written.
  - `.insn j` is not accepted: LLVM 21 packs its numeric offset without the J-type bit scrambling.
  - Register names are never symbols: `lw a0, a1` is an error, where LLVM reads `a1` as a symbol.

### Linker (`merlin-ld`)

`merlin-ld a.o b.o [-T link.ld] [-e entry] [-o out.elf] [--Map out.map]`. The reference is ld.lld: for the same objects and script, the allocated sections (names, types, addresses, sizes, alignments, A/W/X flags), `PT_LOAD`s and symbol values must match, except for the differences listed at the end of this subsection. This is checked against committed `llvm-readelf` output of ld.lld links (`crates/merlin-ld/tests/fixtures`, regenerated with `cargo xtask ld-fixtures`), including layout edge cases (empty and assignment-only output sections, gaps, `NOBITS` before `PROGBITS`).

- **Inputs**: `ET_REL`, `EM_RISCV`, ELFCLASS32, little-endian, `e_flags` 0 (ilp32 soft-float, no RVC). Allocated sections must be `PROGBITS` or `NOBITS`. Rejected with an error: `SHT_REL`, section groups/COMDAT, TLS, common symbols (`-fno-common`), extended section indices. Non-allocated sections (`.riscv.attributes`, `.comment`, debug info) are dropped. Mergeable sections (`SHF_MERGE`, e.g. clang's `.rodata.str1.1` for C string literals) are placed like any other section: see the differences below. Build clang objects with `-mno-relax`: there is no linker relaxation.
- **Symbols**: locals stay per object. Two strong definitions of a global are an error (`b.o: duplicate symbol 'foo' (first defined in a.o)`); a strong definition beats a weak one; an undefined weak symbol is 0. An undefined global nobody defines is an error (`a.o: undefined reference to 'foo'`). Symbols assigned in the linker script override object definitions (as in ld.lld).
- **Default layout** (no `-T`): `.text` at `0x8000_0000` (`.text.start` and `.text.init` first), then `.rodata` (+ `.srodata`), `.data` (+ `.sdata`), `.bss` (+ `.sbss`), each aligned to its most-aligned input. Other allocated sections need a script.
- **Linker scripts**, a subset (anything else is an error naming what it found): `OUTPUT_ARCH(riscv)`, `OUTPUT_FORMAT(elf32-littleriscv)`, `ENTRY(sym)`, `MEMORY` with one region, and `SECTIONS` containing `. = expr;`, `. += expr;`, `sym = expr;` and output sections `name : { *(pattern ...) ... } [> REGION]`. Expressions: numbers (`0x..`, decimal, `K`/`M` suffixes), `.`, symbols already placed, `ALIGN(n)`, `ORIGIN(r)`, `LENGTH(r)`, `+`, `-`, parentheses. Inside an output section `.` may only move forward with `ALIGN`, `+=` or an expression using `.`.
- **Placement**: in script order, the first `*(pattern)` that matches an input section takes it; within one `*(...)`, inputs keep command-line order and file order. An allocated input section no rule matches is an error if it is non-empty or defines a global (an empty one is left out). Each input is aligned to its `sh_addralign`, also when it is empty; an output section starts aligned to its largest input alignment. As in ld.lld, an output section is dropped only when no input section matched it (not even an empty one) and it contains no assignments; one that holds only assignments (`. += n`, `sym = .`) is `PROGBITS` with the flags of the previous output section that had inputs, so `.stack : { . += 4K; }` takes file space (a top-level `. += 4K;` reserves memory without it). The location counter starts at 0, also with `MEMORY` (a symbol assigned before the first output section gets 0, as in ld.lld). With `MEMORY`, every output section goes at the region's next free address, as in ld.lld, which means a top-level `. = ...` before a section would be ignored: merlin-ld reports it as an error instead (put the `ALIGN` inside the section). Overflowing the region is an error.
- **Entry**: `-e` (symbol or number), else `ENTRY(...)`, else `_start`. A missing `-e` symbol is an error; otherwise a warning, and the entry falls back to the first executable section.
- **Output**: `ET_EXEC`, one `PT_LOAD` per run of consecutive output sections with the same permissions (R, RX, RW), as ld.lld does with a `SECTIONS` command: address gaps and `NOBITS` sections inside a run do not split it but are written as zeros, so only `NOBITS` sections at the end of a run (`.bss`) give `p_memsz > p_filesz`. A run of empty sections has no `PT_LOAD`. Segment file offsets are congruent to their addresses mod 4 KiB (`p_align` 0x1000). Section headers and a `.symtab` (locals, then globals) are included for merlin-objdump and for Grail's `tohost` lookup.
- **Relocations**: after layout, every entry of every `.rela.*` section that patches a loaded section goes through one hook, `reloc::apply(type, P, S, A, bytes, ctx)`, where `bytes` starts at the patched place and `ctx` maps each `R_RISCV_PCREL_HI20` place to its S and A for the `PCREL_LO12` relocations. Implementing the hook is issue #7; until then it rejects every relocation with an explicit "not implemented yet" error, so only relocation-free objects link to an executable.
- **Differences from ld.lld**, on purpose:
  - `SHF_MERGE` sections are not merged. ld.lld keeps one copy of identical strings and constants (two C files using the same string literal share it) and orders the pieces by hash; merlin-ld concatenates the input sections in order. When an output section holds duplicates it is larger than ld.lld's and every address after it differs, and merlin-ld warns (`.rodata: mergeable (SHF_MERGE) input sections hold N bytes of duplicate strings or constants; ...`). Without duplicates the sizes match, but a symbol inside a merged string section can still have a different address.
  - With `MEMORY`, a top-level `. = ...` that moves `.` before an output section is an error (ld.lld ignores it), see Placement.
  - Script-assigned symbols are `SHN_ABS` in `.symtab`; ld.lld makes them relative to a nearby output section. Their values are the same.
  - Output section flags are only `SHF_ALLOC`, `SHF_WRITE` and `SHF_EXECINSTR` (ld.lld also carries `SHF_MERGE`/`SHF_STRINGS` over from merged inputs).
  - Unmatched allocated sections are errors, not orphans placed by heuristics. `/DISCARD/`, `NOLOAD`, `AT`, and output section addresses or attributes (`.sec ALIGN(16) : { }`) are not supported.
  - Only `PT_LOAD` program headers (no `PT_GNU_STACK` or `PT_RISCV_ATTRIBUTES`), and no `.riscv.attributes` or `.comment` in the output. File offsets of empty and `NOBITS` sections may differ.

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

### Kernel build

The kernel (`kernel/`) is built by `cargo xtask kernel`, a Rust build task (`crates/xtask`), not a Makefile or shell script: clang compiles each `.c`, clang or merlin-as (`--as merlin`, step 1 above) assembles each `.s`, and `ld.lld` links them with `kernel/kernel.ld`. The full command lines are in `kernel/README.md`. The linked `kernel/kernel.elf` is committed, so running and testing the kernel needs no LLVM; the build is reproducible, and when the installed clang matches the one recorded in `kernel/kernel.elf.clang-version`, `cargo test` checks the committed image is up to date.

Boot contract: Grail's boot ROM jumps to `_start` at 0x8000_0000 in M-mode. `_start` points `mtvec` at an early trap handler, sets `sp` to the top of a 16 KiB stack, zeroes `.bss` (`__bss_start`..`__bss_end`, both 4-byte aligned), and calls `int kmain(void)`; when `kmain` returns, `xcal.halt` stops the machine with its return value as the exit code. Until the kernel installs its real trap handlers, any trap prints `mcause`, `mepc` and `mtval` with `xcal.dbg` and halts with exit code 0xFF. `__kernel_end` (page aligned) marks the first free byte of RAM after the image.

Kernel C stays inside what Merlin will compile: no libc headers (`-nostdinc`), no floating point, no inline `asm()` (CSR access and Xcal instructions live in `.s` files, with Xcal instructions written as `.word`), and `.s` files use only merlin-as syntax. The one exception is varargs: `kprintf` uses `__builtin_va_list`/`__builtin_va_start`/`__builtin_va_arg`/`__builtin_va_end` (via `kernel/include/varargs.h`), so Merlin must provide those builtins with the ILP32 convention (variadic arguments travel in a0-a7 and then on the stack like named ones; the callee spills a0-a7 next to its stack arguments and `va_list` is a pointer walking that area).

## 11. Roadmap

1. **Grail core**: RV32IM fetch/decode/execute, ELF loader, UART, test device, `xcal.dbg`/`xcal.halt`. Pass rv32ui/rv32um.
2. **Disassembler** (shared decoder) and an emulator trace mode.
3. **Privilege + traps**: CSRs, M/S/U, CLINT timer, Sv32. Pass rv32mi/rv32si. (Landed together with step 1.)
4. **merlin-as** emitting ELF `.o`. Validate with `llvm-objdump` and ld.lld.
5. **merlin-ld**. Validate against clang-produced objects.
6. **Bare-metal hello world** written in our assembly, built entirely with our tools.
7. **M extension** in merlin-as (Grail already passes rv32um). **Xcal devices** in Grail: framebuffer window and keyboard via Win32 FFI.
8. **Merlin v0**: C subset → assembly. Grow it test by test.
9. **Camelot kernel** in C (clang first, our compiler as it catches up): trap handling, timer, UART driver, then paging (Sv32), processes, syscalls.
10. **Self-hosting stretch goal**: our tools running on our OS.
