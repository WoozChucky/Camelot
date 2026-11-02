#!/bin/bash
# Regenerates crates/merlin-as/tests/debug_line/*.o, the reference for
# merlin-as's line tables (tests/debug_line.rs): each .s there assembled by
# clang's integrated assembler (LLVM 23), with -mno-relax like merlin-as.
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/debug_line" && pwd)
for s in "$DIR"/*.s; do
  clang --target=riscv32 -march=rv32ima_zicsr_zifencei -mno-relax -c -x assembler "$s" -o "${s%.s}.o"
done
echo "wrote $(ls "$DIR"/*.o | wc -l) objects"
