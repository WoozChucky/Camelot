#!/bin/bash
# Regenerates merlin-as's `li` corpus with clang's integrated assembler
# (LLVM 23), for crates/merlin-as/tests/encode_golden.rs:
#
#   crates/merlin-as/tests/golden/li.golden  2246 values
#
# `li rd, value` expands to the sequence LLVM's RISCVMatInt::generateInstSeq
# picks, up to 8 instructions on RV64; merlin-as must pick the same one
# (crates/merlin-as/src/encode.rs, li_steps). encode.golden has a few
# hand-picked values; this file has many, chosen to reach every rule
# of the algorithm: each power of two, its neighbours and negations, masks of
# runs of ones, and pseudo-random values from a linear congruential generator
# (Knuth's MMIX constants) that are thinned out, shifted and masked to vary
# their shape, plus families that take the rarer paths (negative values whose
# complement has leading zeros, for the final xori; low 13 bits 0x1000..0x17ff,
# for the "round the low 12 bits up" rule). The list is the same on every run.
#
# Output format, one line per value:
#   0x<value, two's complement, 16 hex digits> | <word> ...
#
# Bash arithmetic is 64-bit two's complement with wrapping, as the algorithm's
# is; `>>` is arithmetic, so a logical shift right is written out (lsr).
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/golden" && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

VALUES=()
add() { VALUES+=("$(($1))"); }

# R = $1 >> $2, shifting in zeros (0 <= $2 <= 63).
lsr() { if (($2 == 0)); then R=$1; else R=$((($1 >> $2) & ((1 << (64 - $2)) - 1))); fi; }
# R = $1 mod $2, reading $1 as unsigned.
umod() {
  if (($1 >= 0)); then R=$(($1 % $2)); else
    lsr "$1" 1
    R=$(((2 * (R % $2) + ($1 & 1)) % $2))
  fi
}
# One step of the generator, on X.
lcg() { X=$((X * 6364136223846793005 + 1442695040888963407)); }

values64() {
  local v k a b i lz s
  for v in 0 1 -1 2047 2048 -2048 -2049 0x7FF 0x800 0x7FFFFFFF 0x80000000 0xFFFFFFFF \
    0x100000000 0xFFFFFFFF80000000 0x7FFFFFFFFFFFFFFF 0x8000000000000000 0xFFFFFFFFFFFFFFFF \
    0x80400000 0x123456789ABCDEF0 0xFEDCBA9876543210 0x00FFFFFFFFFFFFFF 0x0FFFFFFFFFFFFFFF \
    0xFFFFFFFFFFFFF800 0x8000000080000000 0x1000 0x7FFFF800 0x7FFFF7FF 0x0000FFFF0000FFFF \
    0x5555555555555555 0xAAAAAAAAAAAAAAAA 0x00000FFFFFFFFFFF 0xFFFFFF0000000000 \
    0x00FF00FF00FF00FF 0x8000000000000001 0x7FFFFFFFFFFFF800 0xFFF 0x1FFFFF \
    0x12345678 0x801 0x10000000 0x17FF 0x7FFFFFFF000 0x1234567817FF 0xFFFFFFFF000 \
    0x80000001 0xFFFFFFFE 0x1FFFFFFFE 0x800000000 0x800000800 0xFFFFFFFF7FFFFFFF \
    0x7FFFFFFF80000000 0xFFFF8000 0x8000000000000800 0x00007FFFFFFFF800 \
    -0x80000000 -0x80000001 0xDEADBEEF 0xDEADBEEFCAFEBABE 0xCAFEBABE00000000 \
    0x00000000FFFFF000 0x00000000FFFFF7FF 0x00000000FFFFF800 0x7FFFF7FFFFFFF800; do
    add "$v"
  done
  for ((k = 0; k < 64; k++)); do
    add "1 << k"; add "(1 << k) - 1"; add "-(1 << k)"; add "(1 << k) + 1"
    add "~((1 << k) - 1) ^ (1 << 63)"
  done
  # Runs of ones, from bit a up to bit b - 1.
  for ((a = 0; a < 64; a += 3)); do
    for ((b = a + 1; b < 65; b += 5)); do
      if ((b == 64)); then add "-1 ^ ((1 << a) - 1)"; else add "((1 << b) - 1) ^ ((1 << a) - 1)"; fi
    done
  done
  X=0x0123456789ABCDEF
  for ((i = 0; i < 600; i++)); do
    lcg
    v=$X
    case $((i % 6)) in
      1) v=$((v & 0x0000FFFFFFFFFFFF)) ;;
      2) v=$((v & 0xFFF000000000FFFF)) ;;
      3) umod "$X" 61; lsr "$v" "$R"; v=$R ;;
      4) umod "$X" 50; v=$((v >> R)) ;;
      5) umod "$X" 40; v=$((v & ~((1 << R) - 1))) ;;
    esac
    add "$v"
  done
  X=7
  for ((i = 0; i < 400; i++)); do
    lcg
    lsr "$X" 58; lz=$((1 + R % 40))
    lsr "$X" 20; s=$((R % 24))
    lsr "$X" "$lz"; v=$((R & ~((1 << s) - 1)))
    add "~v"
    add "(X & ~0x1FFF) | 0x1000 | ((X >> 7) & 0x7FF)"
    add "(R & ~0x1FFF) | 0x1000 | ((X >> 7) & 0x7FF)"
  done
  for v in 0xfffaccc39887ffff 0xff43dc144cdc4fff 0xffffc00042e98fff 0xffffc00071a5efff; do
    add "$v"
  done
}

# gen <out>: assembles `li a0, value` for every value, each followed by an
# `ecall` to mark where its sequence ends (li never writes that word: its rd
# is a0), and writes the values with their words.
gen() {
  local out=$1 v
  VALUES=()
  values64
  # Each value once, in the order first given.
  for v in "${VALUES[@]}"; do printf '%x\n' "$v"; done | awk '!seen[$0]++' > "$TMP/values"
  awk '{ printf "li a0, 0x%s\necall\n", $0 }' "$TMP/values" > "$TMP/li.s"
  clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mno-relax -c "$TMP/li.s" -o "$TMP/li.o"
  llvm-objcopy -O binary --only-section=.text "$TMP/li.o" "$TMP/li.bin"
  {
    echo "# clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mno-relax: li a0, VALUE"
    echo "# regenerate with scripts/gen-li-golden.sh"
    od -An -tx4 -v "$TMP/li.bin" | awk -v d=16 '
      NR == FNR { value[n++] = $0; next }
      {
        for (f = 1; f <= NF; f++) {
          if ($f != "00000073") { words = words " " $f; continue }
          v = value[i++]; while (length(v) < d) v = "0" v
          print "0x" v " |" words; words = ""
        }
      }
      END { if (i != n || words != "") { print "values and sequences do not pair up" > "/dev/stderr"; exit 1 } }
    ' "$TMP/values" -
  } > "$out"
  echo "wrote $(grep -vc '^#' "$out") values to $out"
}

gen "$DIR/li.golden"
