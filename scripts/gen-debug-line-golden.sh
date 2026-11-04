#!/bin/bash
# Regenerates crates/merlin-as/tests/debug_line/*.o, the reference for
# merlin-as's line tables (tests/debug_line.rs): each .s there assembled by
# clang's integrated assembler (LLVM 23), with -mno-relax like merlin-as.
#
# And the RV64 references, debug_line/rv64/*.o: the same .s files assembled
# for RV64, except those with an RV64 version of their own in rv64/ (merlin.s,
# which is compiler output and so depends on the width), assembled from that.
# The RV64-only sources stay in rv64/ because crates/xtask/tests/selfhost.rs
# assembles every .s directly in debug_line with Camelot's /bin/as (RV32).
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/debug_line" && pwd)
RV32="--target=riscv32 -march=rv32ima_zicsr_zifencei -mno-relax"
RV64="--target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mno-relax"
# $RV32 and $RV64 are split into their words on purpose.
# shellcheck disable=SC2086
for s in "$DIR"/*.s; do
  clang $RV32 -c -x assembler "$s" -o "${s%.s}.o"
  name=$(basename "${s%.s}")
  [ -e "$DIR/rv64/$name.s" ] || clang $RV64 -c -x assembler "$s" -o "$DIR/rv64/$name.o"
done
# shellcheck disable=SC2086
for s in "$DIR"/rv64/*.s; do
  clang $RV64 -c -x assembler "$s" -o "${s%.s}.o"
done
echo "wrote $(ls "$DIR"/*.o | wc -l) RV32 objects and $(ls "$DIR"/rv64/*.o | wc -l) RV64 objects"
