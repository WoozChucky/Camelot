# The merlin-as encoder

How merlin-as turns one line of assembly into machine words, and how the
driver finishes the job once the whole file is read. The code is meant to be
read alongside this page:

- [`src/encode.rs`](src/encode.rs): the whole ISA. One function, `encode()`,
  plus the field packers it is built from.
- [`src/asm.rs`](src/asm.rs): the driver. Places the words in sections, turns
  fixups into relocations, and at the end resolves the ones clang resolves.
- [`src/parser.rs`](src/parser.rs): operands are already parsed when the
  encoder sees them (`Operand::Reg`, `Operand::Expr`, `Operand::Mem`).

```rust
pub fn encode(mnemonic: &str, ops: &[Operand], ctx: &Ctx) -> Result<Vec<Emit>, String>
```

It returns one `Emit` per 32-bit word. An operand that names a label leaves
its immediate bits zero and the `Emit` carries a fixup (`RelocKind` +
target). The number of words never depends on a label's address, so the
assembler can stay single pass; it may depend on a constant (`li`).

## Formats and field packers

Every instruction is one of six formats. They keep `opcode`, `rd`, `rs1`,
`rs2` and `funct3` in the same places, so the immediates get scattered
instead:

```
R: funct7[31:25] rs2[24:20] rs1[19:15] funct3[14:12] rd[11:7]  opcode
I: imm[11:0][31:20]         rs1        funct3        rd        opcode
S: imm[11:5]     rs2        rs1        funct3        imm[4:0]  opcode
B: imm[12|10:5]  rs2        rs1        funct3        imm[4:1|11] opcode
U: imm[31:12]                                        rd        opcode
J: imm[20|10:1|11|19:12]                             rd        opcode
```

`r_type`, `i_type`, `s_type`, `b_type`, `u_type` and `j_type` pack the fields.
Each uses an `x_imm` function (`i_imm`, `s_imm`, `b_imm`, `u_imm`, `j_imm`)
that returns only the bits the immediate occupies; they are the inverse of
`imm_i`/`imm_s`/`imm_b`/`imm_j` in `crates/excalibur/src/decode.rs`, and the
driver reuses them to patch resolved labels into words. Branch and jump
offsets are even, so bit 0 is never stored, and the sign is always bit 31.

### The hi/lo split

A 32-bit value is built in two instructions: `lui`/`auipc` supplies the upper
20 bits, and `addi`/a load/a store/`jalr` adds a 12-bit immediate. That
immediate is sign-extended, so when bit 11 of the value is set the low part is
negative and the upper part must be one more:

```
hi = (value + 0x800) >> 12      lo = value - (hi << 12)    (lo in -2048..=2047)
0x12345fff → hi = 0x12346, lo = -1
```

`hi_lo()` does this; `li`, `%hi`/`%lo` of a constant, and the resolution of
pc-relative pairs all use it (ld.lld does the same for `R_RISCV_HI20` and
friends).

## What it accepts

Mnemonics are lower case; registers by ABI name or `xN`; CSRs by name
(`excalibur::csr`) or number.

| Group | Mnemonics |
|---|---|
| RV32I | `lui auipc jal jalr beq bne blt bge bltu bgeu lb lh lw lbu lhu sb sh sw addi slti sltiu xori ori andi slli srli srai add sub sll slt sltu xor srl sra or and` |
| M | `mul mulh mulhsu mulhu div divu rem remu` (OP with funct7 = 1) |
| Zifencei, fences | `fence` (no operands = `iorw, iorw`, or two sets like `rw, w`), `fence.tso`, `fence.i` |
| Privileged | `ecall ebreak mret sret wfi sfence.vma [rs1[, rs2]]` |
| Zicsr | `csrrw csrrs csrrc csrrwi csrrsi csrrci` |
| Xcal (spec §9) | `xcal.dbg rs1`, `xcal.halt rs1` (custom-0, funct3 0 / 1) |

Pseudo-instructions, as in the RISC-V Assembly Programmer's Manual and LLVM:

| Pseudo | Expands to |
|---|---|
| `nop` | `addi zero, zero, 0` |
| `li rd, imm` | see below |
| `mv`, `not`, `neg`, `seqz`, `snez`, `sltz`, `sgtz`, `zext.b` | `addi rd, rs, 0`, `xori rd, rs, -1`, `sub rd, zero, rs`, `sltiu rd, rs, 1`, `sltu rd, zero, rs`, `slt rd, rs, zero`, `slt rd, zero, rs`, `andi rd, rs, 255` |
| `beqz bnez bltz bgez` / `blez bgtz` | the branch with `zero` as rs2 / as rs1 |
| `bgt ble bgtu bleu a, b, t` | `blt bge bltu bgeu b, a, t` (operands swapped) |
| `j t`, `jal t` | `jal zero, t`, `jal ra, t` |
| `jr rs`, `jalr rs`, `ret` | `jalr zero, 0(rs)`, `jalr ra, 0(rs)`, `jalr zero, 0(ra)` |
| `la rd, sym`, `lla rd, sym` | `auipc rd, %pcrel_hi(sym)` + `addi rd, rd, %pcrel_lo(.)` |
| `lw rd, sym` (any load) | `auipc rd, %pcrel_hi(sym)` + `lw rd, %pcrel_lo(.)(rd)` |
| `sw rs, sym, rt` (any store) | `auipc rt, %pcrel_hi(sym)` + `sw rs, %pcrel_lo(.)(rt)` |
| `call sym` / `tail sym` | `auipc ra/t1, 0` + `jalr ra/zero, 0(ra/t1)`, one `R_RISCV_CALL_PLT` on the auipc |
| `csrr rd, c` | `csrrs rd, c, zero` |
| `csrw csrs csrc c, rs` | `csrrw csrrs csrrc zero, c, rs` (with an immediate: the `i` form) |
| `csrwi csrsi csrci c, imm` | `csrrwi csrrsi csrrci zero, c, imm` |
| `rdcycle rdtime rdinstret` (and `h`) | `csrrs rd, cycle/time/instret, zero` |
| `unimp` | `csrrw zero, cycle, zero` |

`%pcrel_lo(.)` above stands for the hidden `.Lpcrel_hiN` label the driver puts
on the `auipc` (`Target::Auipc(0)`), as the psABI requires.

### `li`

`li rd, value` takes a 32-bit constant (signed or unsigned) and picks the same
sequence as LLVM, which the golden file comes from:

1. fits in 12 signed bits: `addi rd, zero, value`;
2. low 12 bits zero: `lui rd, value >> 12`;
3. even and `c << s` with `c` in -32..=31: `addi rd, zero, c` + `slli rd, rd, s`
   (LLVM prefers it because with the C extension both halves compress;
   `li a0, 2048` is `addi a0, zero, 1; slli a0, a0, 11`);
4. otherwise `lui rd, hi` + `addi rd, rd, lo`, split by `hi_lo()`.

### Relocation modifiers

| Operand | Allowed in | Relocation |
|---|---|---|
| `%hi(sym)` | `lui` | `R_RISCV_HI20` |
| `%lo(sym)` | I-type immediates (`addi`..., loads, `jalr`) / stores | `R_RISCV_LO12_I` / `_S` |
| `%pcrel_hi(sym)` | `auipc` | `R_RISCV_PCREL_HI20` |
| `%pcrel_lo(label)` | I-type immediates / stores; `label` is on the `auipc` | `R_RISCV_PCREL_LO12_I` / `_S` |
| a label | branches, `jal`, `j`, `la`, `call`, ... | `R_RISCV_BRANCH`, `R_RISCV_JAL`, ... |

`%hi`/`%lo` of a plain number are computed on the spot. A modifier anywhere
else (on a branch, a shift amount, `%hi` in an `addi`, `%pcrel_hi` in a `lui`)
and a label where only a number fits (`li a0, sym`) are errors.

### `.insn`

For encodings without a mnemonic. hello.S writes the Xcal instructions as
`.insn i 0x0b, 0, x0, a0, 0` so that clang can build it too. The parser turns
`.insn i ...` into the mnemonic `.insn i`. Same forms and encodings as LLVM:

```
.insn r opcode, funct3, funct7, rd, rs1, rs2
.insn i opcode, funct3, rd, rs1, simm12      .insn i opcode, funct3, rd, simm12(rs1)
.insn s opcode, funct3, rs2, simm12(rs1)
.insn b opcode, funct3, rs1, rs2, offset
.insn u opcode, rd, imm20
.insn value                                  .insn 4, value
```

`.insn j` is not supported: LLVM 21 packs its numeric offset without the
J-type scrambling, and matching that would be matching a bug.

## Errors

Everything is range checked and reported on the instruction's line:
immediates (`immediate 2048 out of range (-2048..=2047)`), shift amounts,
even branch/jump offsets, CSR numbers and 5-bit CSR immediates, `.insn`
fields, operand kinds (`expected a register, found an immediate`), operand
counts (`usage: add rd, rs1, rs2`) and unknown mnemonics
(`unknown instruction 'mulw'`). The CLI prints `file:line: error: message`.

## After the last line: resolving local labels

Like clang with `-mno-relax`, `asm.rs` resolves pc-relative references to
labels that are local (not `.globl`) and defined in the same section as the
instruction, once the whole file is read: the distance can't change at link
time. That covers branches, `jal`/`j`, `call`/`tail` (both words of the pair)
and `auipc` + `%pcrel_lo` pairs (`la`, `lw rd, sym`, `sw rs, sym, rt`, explicit
`%pcrel_hi`/`%pcrel_lo`, whose low half gets the low 12 bits of its own
auipc's distance). The immediate is patched with the same `x_imm` functions
and no relocation is written; an out-of-range branch or jump is an error on
its line. Everything else stays a relocation: global or undefined symbols, labels in other sections, and absolute references (`%hi`, `%lo`,
`.word sym`).

`.`, the location counter, is the address of the instruction (its first
word) or data word it appears in: each use becomes a hidden `.Ldot$N` label
there, so `j .` and `beq a0, a1, .-8` are resolved like any local label.

`.L` labels (numeric labels like `1:`, the hidden `.Lpcrel_hiN` and `.Ldot$N`) go into
the symbol table only when a relocation still uses them, and the object
carries the same `.riscv.attributes` as clang's
(`rv32i2p1_m2p0_zicsr2p0_zifencei2p0_zmmul1p0`), so llvm-objdump decodes the M
instructions without extra flags.

For `examples/hello/hello.S` and `examples/muldiv/muldiv.S`, `llvm-objdump -dr`
prints exactly the same text for our object as for clang's. The differences
that remain on other input (spec section 10):

- clang rewrites a branch it can't resolve or reach (to a global, an
  undefined symbol, another section, or too far) into an inverted branch over
  a `jal`. merlin-as keeps it one instruction with `R_RISCV_BRANCH`, or
  reports it out of range.
- A numeric label a relocation refers to is called `.L1$0` (`1:`, first
  definition) where clang says `.Ltmp0`.
- No `$x`/`$d` mapping symbols in the symbol table.
- Operand forms LLVM accepts and merlin-as doesn't: `jalr rd, rs`,
  `jalr off(rs)`, `jr off(rs)`, `jr rs, off`, `call rd, sym`, `jump sym, rt`,
  `%hi(sym)` in `.insn u`, named `.insn` opcodes (`.insn r OP, ...`),
  upper-case CSR names and CSRs Excalibur doesn't have (`fflags`). And
  `call`/`tail` take `sym+offset` or `.`, where clang wants a bare symbol.

## Tests

```
cargo test -p merlin-as
```

- `encode_golden`: 118 lines assembled by llvm-mc (`tests/golden/encode.s` →
  `encode.golden`: RV32I, M, CSRs, pseudo-instructions, relocations). Words and
  relocations must be identical.
- `encode_roundtrip`: `excalibur::decode` of our words gives back the
  operation and operands of a hand-written table (Xcal, CSRs by name, `li`
  splits, explicit `%pcrel` pairs, and the relocation on each word);
  every llvm-mc word, disassembled by `excalibur`, assembles back to itself;
  and so does every decodable word of a 400 000-word pseudo-random sample.
- `encode_errors`: bad lines are rejected on their own line, with the
  wording checked for unknown mnemonics and out-of-range local branches.
- `encode_hello`: hello.S against clang's object (`tests/golden/hello.ref`,
  `tests/golden/hello.dr.txt`), word for word and as `llvm-objdump -dr` text
  (printed by merlin-objdump).
- Unit tests in `encode.rs` (packers against the decoder, `hi_lo`, `li`),
  `asm.rs` (local resolution, against words from clang) and `elf.rs`.

Elsewhere: `crates/merlin-ld/tests/merlin_as_programs.rs` builds
`examples/muldiv/muldiv.S` with merlin-as and merlin-ld and runs it in Grail;
`crates/xtask/tests/hello.rs` runs the committed merlin-as + ld.lld build of
hello (`tests/golden/hello.elf`) in Grail and, when LLVM is installed,
rebuilds it and compares `llvm-objdump -dr` with clang's;
`crates/xtask/tests/kernel.rs` boots the kernel with `kernel/entry.s`
assembled by merlin-as.

### Regenerating the references

- `tests/golden/encode.golden`: add lines to `encode.s` and run
  `scripts/gen-encode-golden.sh` (llvm-mc, llvm-objcopy, llvm-readelf, bash).
- `tests/golden/hello.ref` and `hello.dr.txt`: `scripts/gen-hello-ref.sh`
  (clang and llvm-objdump 21). Rerun when `examples/hello/hello.S` changes.
- `tests/golden/hello.elf`: `cargo xtask hello` (merlin-as and ld.lld).
