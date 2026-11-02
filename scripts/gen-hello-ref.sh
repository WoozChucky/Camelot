#!/bin/bash
# Regenerates what clang's assembler makes of examples/hello/hello.S, for
# crates/merlin-as/tests/encode_hello.rs to compare merlin-as with:
#
#   crates/merlin-as/tests/golden/hello.ref     words and relocations of .text.start
#   crates/merlin-as/tests/golden/hello.dr.txt  llvm-objdump -dr of the whole object
#
# Needs clang and llvm-objdump from any LLVM install (LLVM 21 for hello.dr.txt,
# whose syntax merlin-objdump follows), plus bash (Git Bash on Windows).
#
# Relaxation is off: with it on, clang adds R_RISCV_RELAX next to every
# relocation and leaves calls to the linker, which merlin-as never does.
#
# hello.ref format, one line per word of .text.start:
#   <hex offset> <word> [<R_RISCV_TYPE> <symbol>[+hexaddend]]
# with <auipc> for the hidden label a %pcrel_lo relocation points at.
#
# The linked image of merlin-as's output (crates/merlin-as/tests/golden/hello.elf)
# is rebuilt by `cargo xtask hello` instead.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/crates/merlin-as/tests/golden/hello.ref"
DR="$ROOT/crates/merlin-as/tests/golden/hello.dr.txt"
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
# Run inside $TMP so the header line names plain "hello.o".
(cd "$TMP" && llvm-objdump -d -r hello.o) | tr -d '\r' > "$DR"
echo "wrote $DR"
