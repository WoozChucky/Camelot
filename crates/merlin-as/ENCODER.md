# Writing the merlin-as encoder

Everything in merlin-as is in place except the encoder: `encode()` in
[`src/encode.rs`](src/encode.rs). It receives one instruction and returns its
machine words. `add` is done as a worked example.

```rust
pub fn encode(mnemonic: &str, ops: &[Operand], ctx: &Ctx) -> Result<Vec<Emit>, String>
```

- `mnemonic` is lower-case (`"addi"`, `"fence.i"`, `"xcal.dbg"`).
- `ops` are already parsed: `Operand::Reg(n)`, `Operand::Expr(e)`, `Operand::Mem { offset, base }`.
- `ctx.constant(e)` evaluates numbers and `.equ` constants. `ctx.symbol(e)` gives
  `(name, addend)` for `label`, `label+4`, `1f`... `ctx.csr(e)` resolves `mstatus` or `0x300`.
- Return `Emit::word(w)` for plain words, or `Emit::with_fixup(w, kind, target)`
  when a field refers to a label (leave that field zero; the linker fills it).

The contract is in the doc comment at the top of `encode.rs`, and the
expected output for every case is in `tests/golden/encode.golden`.

## Checking progress

```
cargo test -p merlin-as --test encode_golden -- --nocapture
```

prints `encoder: N/118 lines match llvm-mc` and the mnemonics still missing.
Lines you haven't implemented are counted, never failed; a line you *have*
implemented that differs from llvm-mc fails with the expected and actual words.
When everything is done:

```
cargo test -p merlin-as --test encode_golden -- --ignored
```

Two more suites follow the same rules (unimplemented lines are counted, not
failed; `-- --ignored` requires everything):

- `encode_roundtrip`: `excalibur::decode` of your words must give back the
  operation and operands in a hand-written table that has every golden
  mnemonic plus Xcal, CSRs by name, `li` splits and explicit
  `%pcrel_hi`/`%pcrel_lo` pairs. Each word must carry exactly the relocation
  the table lists, or none. Every llvm-mc word in the golden file, decoded and
  printed by the disassembler, must also assemble back to that exact word.
- `encode_errors`: out-of-range immediates, bad operands, misplaced
  `%hi`/`%lo`/`%pcrel_*` modifiers and unknown mnemonics must be rejected on
  their own source line. Unknown mnemonics currently land in the
  `UNIMPLEMENTED` arm; the complete run wants a real error there that names
  the mnemonic.

`encode_hello` has a single ignored test that assembles
`examples/hello/hello.S` and compares `.text.start` with clang
(`tests/golden/hello.ref`). It needs more than the encoder; see below.

## hello.S (acceptance criteria 3 and 4)

A finished encoder is not enough to assemble `examples/hello/hello.S`:

- **`.insn`.** hello.S writes the two Xcal instructions as
  `.insn i 0x0b, 0, x0, a0, 0` so that clang can build it. The merlin-as
  parser rejects that line (`unexpected text at end of line`) before
  `encode()` is ever called. Writing `xcal.dbg a0` instead fixes merlin-as but
  breaks the clang build, which is the reference. So either merlin-as learns
  `.insn i opcode, funct3, rd, rs1, imm` (parser plus an I-type encode), or
  the clang build stops using hello.S as is. That needs a decision.
- **Same-section labels.** merlin-as is single pass and turns every label
  reference into a relocation. clang resolves branches, jumps and `call` to
  labels in the same section, so `llvm-objdump -d -r` of the two objects
  differs even with a correct encoder. (With relaxation on, clang also adds
  `R_RISCV_RELAX` to every relocation.) `encode_hello` compares against
  clang with `-mno-relax` and accepts the extra relocations, as long as the
  bits outside the immediate match and the label is where clang's resolved
  offset points. Resolving same-section fixups in `asm.rs` is the other
  option.

Running the result in Grail still needs `ld.lld -T examples/hello/link.ld`
and is checked by hand.

## Suggested order

1. **Field packers.** `r_type`, `i_type`, `s_type`, `b_type`, `u_type`, `j_type`.
   They are the mirror image of `imm_i`/`imm_s`/`imm_b`/`imm_j` in
   `crates/excalibur/src/decode.rs`. A round-trip unit test
   (`decode(i_type(...))`) catches bit-scrambling mistakes fast.
2. **OP and OP-IMM** (`add sub sll ... and`, `addi slti ... srai`), with range
   checks: 12-bit signed immediates, 5-bit shift amounts.
3. **M extension** (`mul ... remu`): same as OP with funct7 = 1.
4. **Loads and stores** (`lw a0, 8(sp)`: `Operand::Mem`).
5. **`lui`/`auipc`, `jal`/`jalr`, branches** with numeric offsets (check
   alignment and range).
6. **System**: `ecall ebreak mret sret wfi fence fence.i sfence.vma`, the six
   `csrr*` forms, and the `csrr csrw csrs csrc csrwi csrsi csrci` aliases.
7. **Simple pseudo-instructions**: `nop mv not neg seqz snez sltz sgtz j jr ret`,
   the `beqz`-style branches and the swapped ones (`bgt a, b` = `blt b, a`).
8. **`li`**, following LLVM (which the golden file comes from):
   - fits in 12 signed bits: `addi rd, zero, v`;
   - low 12 bits zero: `lui`;
   - `v` is `c << s` with `c` in -32..=31: `addi rd, zero, c` + `slli rd, rd, s`
     (`li a0, 2048` is `addi a0, zero, 1; slli a0, a0, 11`);
   - anything else: `lui` + `addi`. Remember `addi` sign-extends, so the upper
     part is `(v + 0x800) >> 12`.
9. **Symbols and relocations**: branches/jal to labels (`Branch`, `Jal`),
   `call`/`tail` (auipc + jalr, one `CallPlt` fixup on the auipc), `la`/`lla`
   (auipc `PcrelHi20` + addi `PcrelLo12I` targeting `Target::Auipc(0)`),
   `%hi`/`%lo` operands (`Hi20`, `Lo12I`, `Lo12S`), `lw a0, sym` /
   `sw a0, sym, t0`, and explicit pairs: `1: auipc a0, %pcrel_hi(sym)`
   (`PcrelHi20`) with `addi a0, a0, %pcrel_lo(1b)` (`PcrelLo12I`, or
   `PcrelLo12S` in a store), whose target is the label `ctx.symbol` gives for
   `1b`. The driver checks that the label really is on an auipc with
   `%pcrel_hi`. Reject a modifier in the wrong place (`addi` with `%hi`, `lui`
   with `%lo`, `auipc` with `%hi`, any modifier on a branch, jump or shift
   amount) and a label where only a constant fits (`li a0, sym`).
10. **Xcal** (not in the golden file because llvm-mc doesn't know them; spec
    section 9): `xcal.dbg rs1` = custom-0 funct3 0, `xcal.halt rs1` = funct3 1.
    For hello.S, see the `.insn` note above.

## Regenerating the golden files

Add lines to `tests/golden/encode.s` and run `scripts/gen-encode-golden.sh`
(needs llvm-mc, llvm-objcopy and llvm-readelf from any LLVM install, plus bash,
for example Git Bash on Windows). Addends in the relocation column are hex, as
llvm-readelf prints them.

`tests/golden/hello.ref` comes from `scripts/gen-hello-ref.sh` (clang and
llvm-objdump). Rerun it whenever `examples/hello/hello.S` changes.
