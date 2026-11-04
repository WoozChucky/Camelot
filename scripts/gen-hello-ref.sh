#!/bin/bash
# Regenerates what clang's assembler makes of examples/hello/hello.S, for
# crates/merlin-as/tests/encode_hello.rs to compare merlin-as with, for both
# widths (hello.S is the same source on RV32 and RV64):
#
#   crates/merlin-as/tests/golden/hello.ref          words and relocations of .text.start
#   crates/merlin-as/tests/golden/hello.dr.txt       llvm-objdump -dr of the whole object
#   crates/merlin-as/tests/golden/rv64/hello.ref     the same, for RV64
#   crates/merlin-as/tests/golden/rv64/hello.dr.txt
#
# Needs clang and llvm-objdump from any LLVM install, plus bash (Git Bash on
# Windows). LLVM 23 made the committed files. merlin-objdump follows LLVM 21's
# syntax, which differs from LLVM 23's in one place only, `fence w, 0` (LLVM 23
# prints it as Zihintpause's `pause`); hello.S has no such fence, so both
# versions write the same hello.dr.txt.
# rv64/hello.dr.txt is not compared yet: merlin-objdump reads ELF32 only until
# #150.
#
# Relaxation is off: with it on, clang adds R_RISCV_RELAX next to every
# relocation and leaves calls to the linker, which merlin-as never does.
# hello.S writes the Xcal instructions as mnemonics; clang learns them from
# examples/xcal.inc (-include).
#
# hello.ref format, one line per word of .text.start:
#   <hex offset> <word> [<R_RISCV_TYPE> <symbol>[+hexaddend]]
# with <auipc> for the hidden label a %pcrel_lo relocation points at.
#
# The linked image of merlin-as's output (crates/merlin-as/tests/golden/hello.elf)
# is rebuilt by `cargo xtask hello` instead.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
GOLDEN="$ROOT/crates/merlin-as/tests/golden"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# gen <xlen> <abi> <directory>: writes hello.ref and hello.dr.txt there.
gen() {
  local xlen=$1 abi=$2 out="$3/hello.ref" dr="$3/hello.dr.txt"
  local flags="--target=riscv$xlen -march=rv${xlen}ima_zicsr_zifencei -mabi=$abi -mno-relax"
  # $flags is split into its words on purpose.
  # shellcheck disable=SC2086
  clang $flags -include "$ROOT/examples/xcal.inc" -c "$ROOT/examples/hello/hello.S" -o "$TMP/hello.o"
  {
    echo "# clang $flags"
    echo "#   -include examples/xcal.inc -c examples/hello/hello.S"
    echo "# regenerate with scripts/gen-hello-ref.sh"
    llvm-objdump -d -r --section=.text.start "$TMP/hello.o" | tr -d '\r' | awk '
      function flush() { if (word != "") print off " " word (rel == "" ? "" : " " rel); word = ""; rel = "" }
      /^ +[0-9a-f]+: +[0-9a-f]{8}( |\t|$)/ {
        flush(); off = $1; sub(/:$/, "", off); sub(/^0+/, "", off); if (off == "") off = "0"; word = $2; next
      }
      /R_RISCV_/ {
        sym = $3; add = ""
        if (match(sym, /\+0x[0-9a-f]+$/)) { add = "+" substr(sym, RSTART + 3); sym = substr(sym, 1, RSTART - 1) }
        if (sym ~ /^\.L(pcrel_hi|tmp)/) sym = "<auipc>"
        rel = $2 " " sym add
      }
      END { flush() }'
  } > "$out"
  echo "wrote $(grep -vc '^#' "$out") words to $out"
  # Run inside $TMP so the header line names plain "hello.o".
  (cd "$TMP" && llvm-objdump -d -r hello.o) | tr -d '\r' > "$dr"
  echo "wrote $dr"
}

gen 32 ilp32 "$GOLDEN"
gen 64 lp64 "$GOLDEN/rv64"
