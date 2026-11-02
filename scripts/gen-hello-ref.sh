#!/bin/bash
# Regenerates crates/merlin-as/tests/golden/hello.ref: what clang's assembler
# makes of examples/hello/hello.S, for the encode_hello test to compare with.
# Needs clang and llvm-objdump from any LLVM install, plus bash (Git Bash on Windows).
#
# Relaxation is off: with it on, clang adds R_RISCV_RELAX next to every
# relocation, which merlin-as never emits.
#
# Output format, one line per word of .text.start:
#   <hex offset> <word> [<R_RISCV_TYPE> <symbol>[+hexaddend]]
# with <auipc> for the hidden label a %pcrel_lo relocation points at.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/crates/merlin-as/tests/golden/hello.ref"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -mno-relax \
  -c "$ROOT/examples/hello/hello.S" -o "$TMP/hello.o"
{
  echo "# clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -mno-relax -c examples/hello/hello.S"
  echo "# regenerate with scripts/gen-hello-ref.sh"
  llvm-objdump -d -r --section=.text.start "$TMP/hello.o" | tr -d '\r' | awk '
    function flush() { if (word != "") print off " " word (rel == "" ? "" : " " rel); word = ""; rel = "" }
    /^ +[0-9a-f]+: +[0-9a-f]{8}( |\t|$)/ {
      flush(); off = $1; sub(/:$/, "", off); word = $2; next
    }
    /R_RISCV_/ {
      sym = $3; add = ""
      if (match(sym, /\+0x[0-9a-f]+$/)) { add = "+" substr(sym, RSTART + 3); sym = substr(sym, 1, RSTART - 1) }
      if (sym ~ /^\.L(pcrel_hi|tmp)/) sym = "<auipc>"
      rel = $2 " " sym add
    }
    END { flush() }'
} > "$OUT"
echo "wrote $(grep -vc '^#' "$OUT") words to $OUT"
