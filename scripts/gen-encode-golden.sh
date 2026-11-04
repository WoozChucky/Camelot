#!/bin/bash
# Regenerates merlin-as's encoder goldens with clang's integrated assembler
# and llvm-mc (LLVM 23), for crates/merlin-as/tests/encode_golden.rs:
#
#   crates/merlin-as/tests/golden/encode.golden       RV32: encode.s
#   crates/merlin-as/tests/golden/rv64/encode.golden  RV64: encode.s, then rv64/encode.s
#
# `gen-encode-golden.sh rv32` or `gen-encode-golden.sh rv64` makes only one.
# The RV64 golden starts with every line of encode.s, assembled for RV64: the
# same source must give the same words on both widths, except where the ISA
# says otherwise (`li a0, 0x80000000` is one instruction on RV32, two on
# RV64). The RV64-only lines are in rv64/encode.s, because encode.s is also
# assembled by Camelot's /bin/as, which is RV32 (crates/xtask/tests/selfhost.rs).
#
# Output format, one line per instruction:
#   <source> | <word> <word> ... | <offset>:<R_RISCV_TYPE>:<symbol>[+addend] ...
# Words are the bytes the assembler emits before relocation (immediates of
# relocated fields are zero).
#
# One exception: a conditional branch to a symbol. clang (and llvm-mc's
# object output) relaxes it into the inverted branch over a `jal` even with
# -mno-relax, and LLVM no longer has an option that turns this off. merlin-as
# does no branch relaxation (spec, "Differences from clang"), so for those
# lines the words and the fixup come from `llvm-mc -show-encoding`, which
# prints the instruction as written, before relaxation:
#   beq a0, a1, target  # encoding: [0x63'A',A,0xb5'A',A]
#                       #   fixup A - offset: 0, value: target, kind: fixup_riscv_branch
# Bytes are little-endian; `0xNN'A'` is the byte NN with fixup bits (zero
# here), a bare `A` is a byte of fixup bits only, and `0b...` bytes spell
# their fixup bits as A.
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/golden" && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# The word of a branch to a symbol, from llvm-mc -show-encoding's text in $1.
branch_word() {
  local bytes b v word=""
  bytes=$(printf '%s\n' "$1" | sed -n 's/.*encoding: \[\(.*\)\].*/\1/p' | tr ',' ' ')
  for b in $bytes; do
    b=${b%\'A\'}
    case "$b" in
      A) v=0 ;;
      0b*) b=${b#0b}; v=$((2#${b//A/0})) ;;
      *) v=$((b)) ;;
    esac
    word=$(printf '%02x' "$v")$word
  done
  printf '%s' "$word"
}

# gen <xlen> <out> <in>...: one golden line per instruction of the inputs.
gen() {
  local xlen=$1 out=$2 line src words relocs enc sym
  shift 2
  local target=riscv$xlen march=rv${xlen}ima_zicsr_zifencei mabi=ilp32
  [ "$xlen" = 64 ] && mabi=lp64
  : > "$out"
  cat "$@" | while IFS= read -r line; do
    src=$(echo "$line" | sed 's/#.*//; s/^[[:space:]]*//; s/[[:space:]]*$//')
    [ -z "$src" ] && continue
    printf '%s\n' "$src" > "$TMP/a.s"
    clang --target=$target -march=$march -mabi=$mabi -mno-relax -c -x assembler "$TMP/a.s" -o "$TMP/a.o"
    words=$(llvm-objcopy -O binary --only-section=.text "$TMP/a.o" "$TMP/a.bin" && od -An -tx4 -v "$TMP/a.bin" | xargs)
    relocs=$(llvm-readobj --elf-output-style=GNU -r "$TMP/a.o" | awk '/R_RISCV/ { off=$1; sub(/^0+/,"",off); if (off=="") off="0"; sym=$5; add=""; for(i=6;i<=NF;i++) add=add $i; gsub(/ /,"",add); if (add=="+0") add=""; if (sym ~ /^\.Lpcrel/) sym="<auipc>"; printf "%s:%s:%s%s ", off, $3, sym, add }' | sed 's/ $//')
    case "$src" in
      b*) if [[ $relocs == *R_RISCV_JAL* ]]; then
            enc=$(llvm-mc --triple=$target -mattr=+m,+a,+zicsr,+zifencei,-relax -show-encoding "$TMP/a.s" | tr -d '\r')
            sym=$(printf '%s\n' "$enc" | sed -n 's/.*value: \([^,]*\), kind: fixup_riscv_branch.*/\1/p')
            [ -n "$sym" ] || { echo "$src: llvm-mc shows no branch fixup" >&2; exit 1; }
            # llvm-mc writes an addend in decimal, the golden in hex.
            case "$sym" in *+*) sym=${sym%%+*}+$(printf '%x' "${sym#*+}") ;; esac
            words=$(branch_word "$enc")
            relocs="0:R_RISCV_BRANCH:$sym"
          fi ;;
    esac
    printf '%s | %s | %s\n' "$src" "$words" "$relocs" >> "$out"
  done
  echo "wrote $(wc -l < "$out") lines to $out"
}

want=${1:-both}
case "$want" in
  rv32|rv64|both) ;;
  *) echo "usage: $0 [rv32|rv64]" >&2; exit 2 ;;
esac
if [ "$want" != rv64 ]; then
  gen 32 "$DIR/encode.golden" "$DIR/encode.s"
fi
if [ "$want" != rv32 ]; then
  gen 64 "$DIR/rv64/encode.golden" "$DIR/encode.s" "$DIR/rv64/encode.s"
fi
