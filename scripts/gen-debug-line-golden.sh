#!/bin/bash
# Regenerates crates/merlin-as/tests/debug_line/*.o, the reference for
# merlin-as's line tables (tests/debug_line.rs): each .s there assembled by
# clang's integrated assembler (LLVM 23), with -mno-relax like merlin-as.
# merlin.s is merlin's -g output; crates/xtask/tests/selfhost.rs assembles
# every .s here with Camelot's /bin/as too.
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/debug_line" && pwd)
RV64="--target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mno-relax"
# $RV64 is split into its words on purpose.
# shellcheck disable=SC2086
for s in "$DIR"/*.s; do
  clang $RV64 -c -x assembler "$s" -o "${s%.s}.o"
done
echo "wrote $(ls "$DIR"/*.o | wc -l) objects"
