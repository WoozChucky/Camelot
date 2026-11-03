#!/bin/bash
# Regenerates excalibur's disassembly golden files with LLVM (clang +
# llvm-objdump; the official Windows LLVM installer works).
#
#   scripts/gen-excalibur-golden.sh
#
# Assembles crates/excalibur/tests/golden/rv32.s and rv64.s with clang, for
# their width and without relaxation, and writes what llvm-objdump prints for
# each, with and without aliases: <width>.objdump.txt and
# <width>.no-aliases.txt. crates/excalibur/tests/disasm.rs decodes every word
# in them and checks that excalibur formats it as llvm-objdump did. The
# objects themselves are not kept: the text holds every word. Illegal words
# are written with `.insn 4, WORD`, so llvm-objdump decodes them (as
# <unknown>) instead of printing them back as data.
#
# Made with LLVM 23 (hex immediates are the default since LLVM 19).
set -eu
cd "$(dirname "$0")/.."
DIR=crates/excalibur/tests/golden

for xlen in 32 64; do
    src="$DIR/rv$xlen.s"
    # The object goes next to its source (and is removed): llvm-objdump
    # names it in its header, which should not depend on the machine.
    obj="$DIR/rv$xlen.o"
    clang --target=riscv$xlen -march=rv${xlen}ima_zicsr_zifencei -mno-relax -c "$src" -o "$obj"
    llvm-objdump -d "$obj" > "$DIR/rv$xlen.objdump.txt"
    llvm-objdump -d -M no-aliases "$obj" > "$DIR/rv$xlen.no-aliases.txt"
    rm "$obj"
done
echo "wrote $DIR/rv{32,64}.{objdump,no-aliases}.txt"
