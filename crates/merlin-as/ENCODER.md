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

The ISA is RV64's, `-march=rv64ima_zicsr_zifencei`, the default and the
only one it takes (RV32's, `rv32ima_zicsr_zifencei`, is refused as any other
is). Every instruction word is 32 bits; RV64 has a few instructions more than
RV32, 6-bit shift amounts and 64-bit `li` constants, and not RV32's counter
halves (see below). `.option` and `.attribute` never change the ISA.

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

`hi_lo()` does this for a word; the resolution of pc-relative pairs uses it
(ld.lld does the same for `R_RISCV_HI20` and friends). `hi_lo64()` is the
same split of a 64-bit value, as LLVM folds `%hi(c)` and `%lo(c)`, and `li`
and `%hi`/`%lo` of a constant use it: `hi = ((c + 0x800) >> 12) & 0xfffff`,
so the bits above 31 are dropped (`%hi(0x123456789)` is `0x23456`); on a word
it gives what `hi_lo()` does. `%hi`/`%lo` take any 64-bit constant that way.

`lui` sign-extends its result from bit 31: `lui a0, 0x80000` is
0xffff_ffff_8000_0000. So `lui` + `addi` builds only the sign-extended 32-bit
values, and `li` needs other sequences for the rest (below).

## What it accepts

Mnemonics are lower case; registers by ABI name or `xN`; CSRs by name
(`excalibur::csr`) or number.

| Group | Mnemonics |
|---|---|
| RV64I's 32-bit base (RV32I) | `lui auipc jal jalr beq bne blt bge bltu bgeu lb lh lw lbu lhu sb sh sw addi slti sltiu xori ori andi slli srli srai add sub sll slt sltu xor srl sra or and` |
| M | `mul mulh mulhsu mulhu div divu rem remu` (OP with funct7 = 1) |
| A | `lr.w rd, (rs1)`, `sc.w rd, rs2, (rs1)`, `amoswap.w amoadd.w amoxor.w amoand.w amoor.w amomin.w amomax.w amominu.w amomaxu.w rd, rs2, (rs1)`, each also with `.aq`, `.rl` or `.aqrl` (see below) |
| Zifencei, fences | `fence` (no operands = `iorw, iorw`, or two sets like `rw, w`), `fence.tso`, `fence.i` |
| Privileged | `ecall ebreak mret sret wfi sfence.vma [rs1[, rs2]]` |
| Zicsr | `csrrw csrrs csrrc csrrwi csrrsi csrrci` |
| Xcal (spec §9) | `xcal.dbg rs1`, `xcal.halt rs1` (custom-0, funct3 0 / 1) |
| RV64I | `ld lwu sd` (funct3 `011`, `110`; `011`), `addiw slliw srliw sraiw` (OP-IMM-32), `addw subw sllw srlw sraw` (OP-32) |
| M, RV64's own | `mulw divw divuw remw remuw` (OP-32 with funct7 = 1) |
| A, RV64's own | `lr.d`, `sc.d`, `amoswap.d` ... `amomaxu.d`, with the same suffixes |

RV64's own mnemonics live in tables of their own (`OP_32_FUNCT`,
`OP_IMM_32_FUNCT3`, `SHIFT_32_FUNCT`, `LOAD_RV64_FUNCT3`,
`STORE_RV64_FUNCT3`), looked up with the others; the C assembler
(`kernel/user/as`) gets them all from `crates/xtask/src/c_tables.rs`.

Shift amounts are 6 bits: `slli`, `srli` and `srai` put the amount in
bits [25:20] and keep only funct6 in [31:26] (`srai` is `010000`), so
`slli a0, a0, 63` is legal and 64 is `shift amount 64 out of range (0..=63)`.
The W shifts take 5 bits (`0..=31`) under a full funct7.

Pseudo-instructions, as in the RISC-V Assembly Programmer's Manual and LLVM:

| Pseudo | Expands to |
|---|---|
| `nop` | `addi zero, zero, 0` |
| `li rd, imm` | see below |
| `mv`, `not`, `neg`, `seqz`, `snez`, `sltz`, `sgtz`, `zext.b` | `addi rd, rs, 0`, `xori rd, rs, -1`, `sub rd, zero, rs`, `sltiu rd, rs, 1`, `sltu rd, zero, rs`, `slt rd, rs, zero`, `slt rd, zero, rs`, `andi rd, rs, 255` |
| `beqz bnez bltz bgez` / `blez bgtz` | the branch with `zero` as rs2 / as rs1 |
| `sgt sgtu rd, a, b` | `slt sltu rd, b, a` (sources swapped) |
| `bgt ble bgtu bleu a, b, t` | `blt bge bltu bgeu b, a, t` (operands swapped) |
| `j t`, `jal t` | `jal zero, t`, `jal ra, t` |
| `jr rs`, `jalr rs`, `ret` | `jalr zero, 0(rs)`, `jalr ra, 0(rs)`, `jalr zero, 0(ra)` |
| `la rd, sym`, `lla rd, sym` | `auipc rd, %pcrel_hi(sym)` + `addi rd, rd, %pcrel_lo(.)` |
| `lw rd, sym` (any load, `ld` and `lwu` too) | `auipc rd, %pcrel_hi(sym)` + `lw rd, %pcrel_lo(.)(rd)` |
| `sw rs, sym, rt` (any store, `sd` too) | `auipc rt, %pcrel_hi(sym)` + `sw rs, %pcrel_lo(.)(rt)` |
| `call sym` / `tail sym` | `auipc ra/t1, 0` + `jalr ra/zero, 0(ra/t1)`, one `R_RISCV_CALL_PLT` on the auipc |
| `csrr rd, c` | `csrrs rd, c, zero` |
| `csrw csrs csrc c, rs` | `csrrw csrrs csrrc zero, c, rs` (with an immediate: the `i` form) |
| `csrwi csrsi csrci c, imm` | `csrrwi csrrsi csrrci zero, c, imm` |
| `rdcycle rdtime rdinstret` | `csrrs rd, cycle/time/instret, zero` |
| `unimp` | `csrrw zero, cycle, zero` |
| `sext.w`, `negw` | `addiw rd, rs, 0`, `subw rd, zero, rs` |
| `zext.w` | `slli rd, rs, 32` + `srli rd, rd, 32` (LLVM's expansion without Zba) |

`sext.b`, `sext.h` and `zext.h`, which LLVM expands without Zbb into two
shifts, are not provided.

RV64 has no high counter halves: its CSRs hold all 64 bits. So
`rdcycleh a0` is `'rdcycleh' exists only on RV32 (on RV64, rdcycle reads all 64 bits)`
(and `rdtimeh`, `rdinstreth`), and the name of an RV32-only CSR (`cycleh`,
`mstatush`, `pmpcfg1`..., `excalibur::csr::rv32_only`) is
`CSR 'cycleh' exists only on RV32`, as LLVM refuses them. A CSR number is
accepted all the same, as LLVM does (`csrr a0, 0xc80`).

`%pcrel_lo(.)` above stands for the hidden `.Lpcrel_hiN` label the driver puts
on the `auipc` (`Target::Auipc(0)`), as the psABI requires.

### Atomics (A)

R-type words on the AMO opcode (`0101111`) with funct3 = `010` (`.w`, a
word) or `011` (`.d`, a doubleword). What R-type calls funct7 is
split in two: funct5 in bits [31:27] picks the operation, then the two
ordering bits, aq (26) and rl (25):

```
funct5[31:27] aq[26] rl[25] rs2[24:20] rs1[19:15] 01w rd[11:7] 0101111
```

The suffix sets the ordering bits: none = 00, `.aq` = 10, `.rl` = 01,
`.aqrl` = 11 (`atomic()` strips it, looks the rest up in `ATOMIC_FUNCT5`, a
`.d` form under its `.w` row, and returns the funct3 of the width). `lr.w` and
`lr.d` have no rs2: their rs2 field is zero. The address is written `(rs1)`;
as in LLVM, `0(rs1)` is accepted too, and any other offset is an error, since
there is no offset field. Like LLVM, it must be the literal `0`, one integer
token (`0`, `0x0`): an expression or a symbol that equals 0 (`-0(rs1)`,
`+0(rs1)`, `(0)(rs1)`, `1-1(rs1)`) is an error too. The parser folds `+0` and
`(0)` to the number 0, so it records in the operand (`int_token`) whether the
offset was a single integer token, and `atomic_address()` checks that. The
source register comes before the address, as for stores
(`amoadd.w rd, rs2, (rs1)`), and rd gets the old memory word.

### `li`

`li rd, value` picks exactly LLVM's sequence: `li_steps(value)` is a
port of `RISCVMatInt::generateInstSeq` (llvm/lib/Target/RISCV/MCTargetDesc/
RISCVMatInt.cpp) for the base ISA. With Zba, Zbb or Zbs LLVM would have more
instructions to try, and with C it would weigh the compressed ones; merlin-as
has none of them, so a sequence costs its length. The constant is an
expression computed in 64 bits, wrapping around (`-0x8000000000000000 - 1` is
`0x7fffffffffffffff`, as in LLVM).

It takes any 64-bit constant and gives 1 to 8 instructions, from
`lui`, `addi`, `addiw`, `slli`, `srli` and `xori`. The length depends only on
the value, so the assembler stays single pass, and 8 words is the most a `li`
takes. LLVM builds a first sequence, then tries other shapes and keeps one
only if it is shorter (the comments on `li_steps` go through each):

1. The plain sequence (`li_base`, LLVM's `generateInstSeqImpl`). A
   sign-extended 32-bit value is `lui hi` + `addi lo`; `lui` sign-extends
   from bit 31, so when the 64-bit sum would leave the 32-bit range the
   `addi` becomes an `addiw` (`0x7fffffff` is `lui 0x80000; addiw -1`, but
   `0x12345678` stays `lui 0x12345; addi 0x678`). A wider value puts its low
   12 bits in an `addi` at the end and builds the rest shifted right, then
   shifted back with `slli`: `li a0, 0x80000000` is
   `addi a0, zero, 1; slli a0, a0, 31`, `li sp, 0x80400000` is
   `addi sp, zero, 513; slli sp, sp, 22`, and `0x123456789abcdef0` is `lui`,
   `addi`, then `slli`+`addi` three times, 8 instructions.
2. Trailing zeros (when the low 12 bits are not all zero, which rule 1
   already shifts away): build `value >> tz`, then `slli tz`. Kept when
   shorter, or when the shifted value fits in 6 bits (LLVM prefers it
   because with the C extension both halves compress):
   `li a0, 2048` is `addi a0, zero, 1; slli a0, a0, 11` where rule 1 has
   `lui a0, 1; addi a0, a0, -2048`, and `li a0, 0x80002b04` is
   `lui a0, 0x20001; addi a0, a0, -0x53f; slli a0, a0, 2` where rule 1 takes
   four instructions. One or two instructions cannot be beaten, so a
   sequence that short stops here.
3. Low 13 bits from 0x1001 to 0x17ff: build `value + (0x800 - low 12 bits)`,
   whose low bits are then 0x1800, and finish with an `addi` that takes it
   back.
4. A positive value: build it shifted all the way left, with ones or zeros
   shifted in, and finish with `srli` by the leading zeros
   (`li_leading_zeros`). `0xffffffff` is `addi -1; srli 32`, zero-extended.
5. A negative value: build its complement as in 4, then `xori -1` (`not`).

### Relocation modifiers

| Operand | Allowed in | Relocation |
|---|---|---|
| `%hi(sym)` | `lui` | `R_RISCV_HI20` |
| `%lo(sym)` | I-type immediates (`addi`..., loads, `jalr`) / stores | `R_RISCV_LO12_I` / `_S` |
| `%pcrel_hi(sym)` | `auipc` | `R_RISCV_PCREL_HI20` |
| `%pcrel_lo(label)` | I-type immediates / stores; `label` is on the `auipc` | `R_RISCV_PCREL_LO12_I` / `_S` |
| a label | branches, `jal`, `j`, `la`, `call`, ... | `R_RISCV_BRANCH`, `R_RISCV_JAL`, ... |

`%hi`/`%lo` of a plain number are computed on the spot (`hi_lo64()`). A
modifier anywhere
else (on a branch, a shift amount, `%hi` in an `addi`, `%pcrel_hi` in a `lui`)
and a label where only a number fits (`li a0, sym`) are errors.

### `.insn`

For encodings without a mnemonic. `xcal.dbg a0` is `.insn i 0x0b, 0, x0, a0, 0`
(that is how `examples/xcal.inc` teaches clang the Xcal mnemonics the examples
use). The parser turns `.insn i ...` into the mnemonic `.insn i`. Same forms
and encodings as LLVM:

```
.insn r opcode, funct3, funct7, rd, rs1, rs2
.insn i opcode, funct3, rd, rs1, simm12      .insn i opcode, funct3, rd, simm12(rs1)
.insn s opcode, funct3, rs2, simm12(rs1)
.insn b opcode, funct3, rs1, rs2, offset
.insn u opcode, rd, imm20
.insn value                                  .insn 4, value
```

`.insn j` is not supported: LLVM (21, and still 23) packs its numeric offset
without the J-type scrambling (`.insn j 0x6f, a0, 2048` gives `0040056f`,
`jal a0, 0x4`), and matching that would be matching a bug.

## Errors

Everything is range checked and reported on the instruction's line:
immediates (`immediate 2048 out of range (-2048..=2047)`), shift amounts
(`shift amount 64 out of range (0..=63)`, `(0..=31)` for the W shifts), even branch/jump offsets, CSR numbers and 5-bit CSR
immediates, `.insn` fields, operand kinds (`expected a register, found an
immediate`), operand counts (`usage: add rd, rs1, rs2`) and unknown mnemonics
(`unknown instruction 'frob'`), and RV32's own: `'rdcycleh' exists only on
RV32 (on RV64, rdcycle reads all 64 bits)` and `CSR 'cycleh' exists only on
RV32`. The CLI prints `file:line: error: message`; a `-march` it does not
take is `merlin-as: unsupported -march 'rv64gc' (merlin-as takes
rv64ima_zicsr_zifencei)`, and so is RV32's (`unsupported -march
'rv32ima_zicsr_zifencei'`), with exit status 2.

Data directives (`asm.rs`, not the encoder) check their numbers too: N-byte
data takes [-2^(8N-1), 2^(8N) - 1], as LLVM does, else
`data value 256 does not fit in 1 byte`; a `.dword` takes any 64-bit value;
a symbol fits only `.word` (`R_RISCV_32`) and `.dword` (`R_RISCV_64`)
(`only .word and .dword can hold a symbol address`).
Expressions are 64-bit and wrap around, and `>>` is logical, as in LLVM:
`-16 >> 2` is `0x3ffffffffffffffc`.

## After the last line: resolving local labels

Like clang with `-mno-relax`, `asm.rs` resolves pc-relative references to
labels that are local (not `.globl`) and defined in the same section as the
instruction, once the whole file is read: the distance can't change at link
time. That covers branches, `jal`/`j`, `call`/`tail` (both words of the pair)
and `auipc` + `%pcrel_lo` pairs (`la`, `lw rd, sym`, `sw rs, sym, rt`,
`ld`/`lwu rd, sym` and `sd rs, sym, rt`, explicit
`%pcrel_hi`/`%pcrel_lo`, whose low half gets the low 12 bits of its own
auipc's distance). The immediate is patched with the same `x_imm` functions
and no relocation is written; an out-of-range branch or jump is an error on
its line. Everything else stays a relocation: global or undefined symbols, labels in other sections, and absolute references (`%hi`, `%lo`,
`.word sym`, `.dword sym`).

`.`, the location counter, is the address of the instruction (its first
word) or data word it appears in: each use becomes a hidden `.Ldot$N` label
there, so `j .` and `beq a0, a1, .-8` are resolved like any local label.

`.L` labels (numeric labels like `1:`, the hidden `.Lpcrel_hiN` and `.Ldot$N`) go into
the symbol table only when a relocation still uses them. Every section
marks where its code and data start with clang's mapping symbols, `$x<isa>`
before a run of instructions and `$d` before a run of data
(`Assembler::mapping_symbol`; spec section 10 has the rules), so a
disassembler lists a `.word` among instructions as data; `.file "name"`
is an `STT_FILE` symbol, the first of the locals as in clang. The object
carries the same `.riscv.attributes` as clang's for its `-march`
(`rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0`,
`elf::ARCH`), so llvm-objdump decodes the M and A instructions without extra
flags. The object is ELF64 (`src/elf.rs`).

For every source in `examples/` (clang assembling it with
`-include examples/xcal.inc`), `llvm-objdump -dr` prints exactly the same text
for our object as for clang's (with the flags of the reference builds,
`xtask::ld_fixtures::FLAGS`; `crates/xtask/tests/hello.rs`, below). The differences that remain on other
input (spec section 10):

- clang rewrites a branch it can't resolve or reach (to a global, an
  undefined symbol, another section, or too far) into an inverted branch over
  a `jal`. merlin-as keeps it one instruction with `R_RISCV_BRANCH`, or
  reports it out of range.
- A numeric label a relocation refers to is called `.L1$0` (`1:`, first
  definition) where clang says `.Ltmp0`.
- Operand forms LLVM accepts and merlin-as doesn't: `jalr rd, rs`,
  `jalr off(rs)`, `jr off(rs)`, `jr rs, off`, `call rd, sym`, `jump sym, rt`,
  `%hi(sym)` in `.insn u`, named `.insn` opcodes (`.insn r OP, ...`),
  upper-case CSR names and CSRs Excalibur doesn't have (`fflags`). And
  `call`/`tail` take `sym+offset` or `.`, where clang wants a bare symbol.
- An immediate in a register operation (`add a0, a1, 5`,
  `addw a0, a1, 5`, `sllw a0, a1, 3`), which LLVM assembles as the
  immediate form, is an error: write `addi`, `addiw`, `slliw`.
- `li rd, %lo(sym)` is an error (LLVM: `addi rd, zero, %lo(sym)`), and
  `li rd, %hi(c)` of a constant is accepted, where LLVM refuses it.
- No `sext.b`, `sext.h` or `zext.h`.
- `.option` and `.attribute` change nothing: only `-march` sets the ISA,
  and one that names another ISA is an error.

## Tests

```
cargo test -p merlin-as
```

Camelot's `/bin/as` assembles the references in
`crates/xtask/tests/selfhost.rs`, the values of `li.golden` too.

- `encode_golden`: lines assembled by LLVM, words and relocations identical:
  299 lines (`tests/golden/encode.s` → `encode.golden`): the base ISA, M,
  A with every ordering suffix, CSRs, pseudo-instructions, relocations, the W
  operations, `ld`/`lwu`/`sd` and their pc-relative forms, 6-bit shifts, the
  `.d` atomics with every ordering, `sext.w`/`negw`/`zext.w` and `li` edge
  cases. And `li` against LLVM's choice for 2246 values (`golden/li.golden`,
  which reach every length from 1 to 8): the words, and `li_steps`'s length.
- `encode_roundtrip`: `excalibur::decode` of our words gives back the
  operation and operands of a hand-written table (Xcal, CSRs by name, `li`
  splits, explicit `%pcrel` pairs, and the relocation on each word), and of
  every A mnemonic with every suffix (`.w` and `.d`);
  every LLVM word, disassembled by `excalibur`, assembles back to itself;
  and so does every decodable word of a 400 000-word pseudo-random sample.
- `encode_errors`: bad lines are rejected on their own line, with the
  wording checked for unknown mnemonics and out-of-range local branches,
  shift and immediate ranges, the RV32-only counters and CSRs, an
  `.option arch` of another ISA, and the CLI's `-march` (RV32's refused).
- `encode_hello`: hello.S against clang's object (`tests/golden/hello.ref`,
  `tests/golden/hello.dr.txt`), word for word and as `llvm-objdump -dr` text
  (printed by merlin-objdump).
- `elf64`: an RV64 object read back with the `elf` crate against what clang
  writes for the same source (`--target=riscv64 -march=rv64ima_zicsr_zifencei
  -mabi=lp64 -mno-relax`): header, sections, symbols, relocations and line
  table.
- `debug_line`: every `.debug_*` section and its relocations against clang's
  objects, `tests/debug_line/*.o` (`merlin.s` is compiler output; `files.s`
  is a table of `.file`s without a `.loc`).
- `mapping`: the `$x`/`$d` mapping symbols, the whole symbol table and
  every section against clang's objects, `tests/mapping/*.o`, and
  merlin-objdump's listing against
  llvm-objdump's (`*.d.txt`).
- Unit tests in `encode.rs` (packers against the decoder, `hi_lo`, `li` and
  `li_steps`, the 64-bit instructions decoded back), `asm.rs` (local
  resolution, against words from clang; data of every size), `elf.rs` and
  `dwarf.rs` (the attributes and line tables).

Elsewhere: `crates/xtask/tests/examples.rs` builds every example with
merlin-as and merlin-ld, runs it in Grail and compares `hello` and
`multiobj` with ld.lld's link of clang's objects;
`crates/merlin-ld/tests/merlin_as_programs.rs` builds
`examples/muldiv/muldiv.S` with merlin-as and merlin-ld and runs it in Grail;
`crates/xtask/tests/hello.rs` runs the committed merlin-as + ld.lld builds of
hello (`tests/golden/hello.elf`) in Grail and, when LLVM is installed,
rebuilds it and compares `llvm-objdump -dr` of every example object with
clang's;
`crates/xtask/tests/kernel.rs` boots the kernel with `kernel/entry.s`
assembled by merlin-as.

### Regenerating the references

The scripts need LLVM (clang, llvm-mc, llvm-objcopy, llvm-readobj,
llvm-objdump; LLVM 23 made the committed files) and bash (Git Bash on
Windows); their output is committed, so `cargo test` needs none of it.

- `tests/golden/encode.golden`: add lines to `encode.s` and run
  `scripts/gen-encode-golden.sh`. clang
  relaxes a conditional branch to a symbol even with `-mno-relax`, and LLVM
  has no option to stop it, so for those lines the script takes the word and
  the fixup from `llvm-mc -show-encoding`, which prints them as written.
- `tests/golden/li.golden`: `scripts/gen-li-golden.sh`, whose list of values is fixed (powers of two
  and their neighbours, masks, and a deterministic pseudo-random sample).
- `tests/golden/hello.ref` and `hello.dr.txt`: `scripts/gen-hello-ref.sh`. Rerun when
  `examples/hello/hello.S` changes.
- `tests/debug_line/*.o`: `scripts/gen-debug-line-golden.sh`.
- `tests/mapping/*.o` and `*.d.txt`: `scripts/gen-mapping-golden.sh`.
- `tests/golden/hello.elf`: `cargo xtask hello` (merlin-as and ld.lld; LLD
  23.1.4 made it).
