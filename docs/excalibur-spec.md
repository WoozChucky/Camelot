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
| 0001111 MISC-MEM | `fence`, `fence.i` |
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

## 7. Memory map

Layout follows the QEMU `virt` board's conventions so existing docs and examples apply, without needing QEMU.

| Range | Device |
|---|---|
| 0x0000_1000 - 0x0000_1FFF | Boot ROM (reset vector 0x1000, jumps to 0x8000_0000) |
| 0x0200_0000 - 0x0200_FFFF | CLINT: msip @+0x0, mtimecmp @+0x4000, mtime @+0xBFF8 |
| 0x0C00_0000 - 0x0FFF_FFFF | PLIC (phase 2, for UART interrupts) |
| 0x1000_0000 - 0x1000_00FF | UART, 16550-compatible subset (THR/RBR @+0, IER @+1, LSR @+5) |
| 0x1000_1000 - 0x1000_1FFF | Reserved: virtio block device (later, for a filesystem) |
| 0x0010_0000 | Test/exit device: write 0x5555 = pass/exit, 0x3333 = fail (used by riscv-tests) |
| 0x3000_0000 - 0x3000_0FFF | Framebuffer control (Xcal, section 9.3) |
| 0x3000_1000 - 0x3000_1FFF | Keyboard (Xcal, section 9.4) |
| 0x3100_0000 - 0x3112_BFFF | Framebuffer VRAM |
| 0x8000_0000 - 0x87FF_FFFF | RAM, 128 MiB default (configurable) |

Unmapped accesses raise access-fault exceptions.

Timer: `mtime` increments once per executed instruction, so runs are deterministic and reproducible. During `wfi`, Grail jumps `mtime` forward to `mtimecmp` instead of spinning. (A real-time mode can be added later.) MTIP is pending while `mtime >= mtimecmp`.

UART: writing THR prints the byte to the host terminal. RBR is fed from host stdin. LSR bit 5 (THRE) always 1, bit 0 (DR) set when input is available.

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

### 9.4 Keyboard (MMIO)

| Address | Register | Notes |
|---|---|---|
| 0x3000_1000 | `KBD_STATUS` (RO) | bit 0 = event available |
| 0x3000_1004 | `KBD_DATA` (RO) | pops one event: bits [7:0] key code, bit 8 = 1 on release, bit 9 = shift, bit 10 = ctrl. 0 if empty |
| 0x3000_1008 | `KBD_CTRL` | bit 0 = interrupt enable (PLIC source 2, once the PLIC exists) |

Key codes: printable ASCII for printable keys, then 0x80+ for Enter (0x0A uses ASCII), arrows (0x80-0x83 up/down/left/right), Esc (0x1B), Backspace (0x08), F1-F12 (0x90-0x9B). The key events come from the framebuffer window; without a window, the UART remains the text input.

## 10. Toolchain (all our own)

Everything that runs on the host is written by us, with no third-party libraries doing the heavy lifting (std only). The target side is code produced by our own tools.

| Tool | Input → output | Notes |
|---|---|---|
| Assembler (`merlin-as`) | `.s` → `.o` | Full RV32I(+M) mnemonics, standard pseudo-instructions (`li`, `la`, `call`, `ret`, `mv`, `j`, `beqz`...), directives (`.text .data .bss .section .globl .word .byte .ascii .align .equ`), `%hi/%lo/%pcrel_hi/%pcrel_lo` |
| Linker (`merlin-ld`) | `.o` files → executable | Symbol resolution, relocation, section merging, simple linker script (or fixed layout: `.text` at 0x8000_0000) |
| Emulator (`grail`) | executable → runs it | Loads ELF directly, no boot loader needed early on |
| Disassembler (`merlin-objdump`) | `.o`/exe → text | Shares the decoder with the emulator; essential for debugging |
| Compiler (`merlin`) | C subset → `.s` | Emits assembly for our assembler. Starts small (int, pointers, structs, functions), grows toward enough C for the kernel |

### Merlin C subset

`merlin input.c -o output.s` compiles one translation unit; `--dump-ast` prints the checked syntax tree instead. Errors are reported as `file.c:line:col: error: message` (columns count characters from 1); lexing and parsing stop at the first error, name resolution reports all of them.

**v0** (a strict subset of C99: every v0 program means the same thing to clang):

- Types: `int` only (32-bit two's complement). No globals, pointers, arrays or structs yet; no preprocessor.
- Functions: `int f(int a, ...)` with at most 8 parameters, passed in `a0-a7` (ILP32 calling convention, result in `a0`). `int f(void)` and `int f()` both mean no parameters. Prototypes (`int f(int a);`) declare functions defined later or in another file.
- Names must be declared before use, as in C99: a call to a function not yet declared is an error (no implicit declarations), and so is a wrong argument count. Block scoping follows C: inner blocks and `for` initializers may shadow outer names; parameters share the scope of the function body's outermost block.
- Statements: blocks, `int` declarations with optional initializers (`int a = 1, b;`), expression statements, `if`/`else`, `while`, `for`, `return expr`, `break`, `continue`.
- Expressions, loosest first: `=` (right-associative, value is the assigned value), `||`, `&&`, `== !=`, `< <= > >=`, `+ -`, `* / %`, unary `- !`, then literals, variables, calls and parentheses. Literals are decimal, hex (`0x`) or octal (`0`), up to `0xFFFFFFFF`.
- Where C leaves behavior undefined, merlin defines it the way the hardware does: arithmetic wraps, `/` and `%` round toward zero (`div`/`rem`, so `INT_MIN / -1` wraps and division by zero gives RISC-V's results), and falling off the end of a function returns 0. Argument evaluation order is unspecified; tests must not depend on it.

Test programs live in `crates/merlin/tests/cases/*.c`, each starting with `// expect: N` (the exit code). The harness links them with a small `crt0.s` (set `sp` to the top of RAM, `call main`, `xcal.halt a0`) and runs them in Grail, so the exit code is the low 8 bits of `main`'s return value.

### Object and executable format

**ELF32 (little-endian, EM_RISCV)**, a subset:

- Relocatable `.o`: sections `.text .data .rodata .bss`, `.symtab`, `.strtab`, `.rela.*`.
- Relocations needed: `R_RISCV_32`, `R_RISCV_BRANCH`, `R_RISCV_JAL`, `R_RISCV_HI20`, `R_RISCV_LO12_I`, `R_RISCV_LO12_S`, `R_RISCV_PCREL_HI20`, `R_RISCV_PCREL_LO12_I/S`, `R_RISCV_CALL`. No linker relaxation.
- Executable: `ET_EXEC`, program headers only for `PT_LOAD`, entry point in `e_entry`.

Why ELF rather than a custom format: it costs little more to write, and it means clang/lld, `llvm-objdump` and `llvm-readelf` can check our output and produce reference files, and riscv-tests ELFs load directly. A custom format can still be added later as an exercise.

### Bootstrapping

Until our compiler can build the kernel, clang (`--target=riscv32 -march=rv32im -nostdlib`, native Windows LLVM build) is the reference compiler. Each of our tools is validated by swapping it in for the LLVM one and diffing behavior:

1. Our assembler output linked by `ld.lld` → must run the same.
2. clang `.o` files linked by our linker → must run the same.
3. Our compiler's `.s` → our assembler → our linker → emulator.

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
