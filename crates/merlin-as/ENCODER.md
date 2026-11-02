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
8. **`li`**: 1 word if the value fits in 12 signed bits (`addi`), 1 word if the low
   12 bits are zero (`lui`), else `lui` + `addi`. Remember `addi` sign-extends,
   so the upper part is `(v + 0x800) >> 12`.
9. **Symbols and relocations**: branches/jal to labels (`Branch`, `Jal`),
   `call`/`tail` (auipc + jalr, one `CallPlt` fixup on the auipc), `la`/`lla`
   (auipc `PcrelHi20` + addi `PcrelLo12I` targeting `Target::Auipc(0)`),
   `%hi`/`%lo` operands (`Hi20`, `Lo12I`, `Lo12S`), and `lw a0, sym` /
   `sw a0, sym, t0`.
10. **Xcal** (not in the golden file because llvm-mc doesn't know them; spec
    section 9): `xcal.dbg rs1` = custom-0 funct3 0, `xcal.halt rs1` = funct3 1.
    Once these work, `examples/hello/hello.S` can drop its `.insn` lines.

## Regenerating the golden file

Add lines to `tests/golden/encode.s` and run `scripts/gen-encode-golden.sh`
(needs llvm-mc, llvm-objcopy and llvm-readelf from any LLVM install, plus bash,
for example Git Bash on Windows). Addends in the relocation column are hex, as
llvm-readelf prints them.
