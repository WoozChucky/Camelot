#!/bin/bash
# Regenerates the elf crate's LLVM fixtures (crates/elf/tests/llvm), with
# clang, ld.lld, llvm-readelf and llvm-dwarfdump. On Windows llvm-readelf is
# llvm-readobj under that name (copy it), and llvm-dwarfdump comes from the
# release archive (clang+llvm-*-x86_64-pc-windows-msvc), not the installer.
#
#   scripts/gen-elf-golden.sh
#
# For each width, prog.c becomes, in rv32/ and rv64/:
#
#   prog.o        compiled with -g and without relaxation
#   prog-relax.o  the same with relaxation: R_RISCV_ADD/SUB pairs in the
#                 debug sections, R_RISCV_ALIGN and R_RISCV_RELAX in the code
#   prog.elf      prog.o linked by ld.lld at 0x8000_0000
#
# and for each of them FILE.readelf (llvm-readelf -h -S -l -s -r),
# FILE.lines (llvm-dwarfdump --debug-line) and FILE.info (llvm-dwarfdump
# --debug-info). crates/elf/tests/llvm.rs checks that the elf crate reads the
# same header, sections, segments, symbols, relocations, line table rows and
# debugging information entries.
#
# Made with LLVM 23.
set -eu
# Git Bash would turn /camelot below into a Windows path.
export MSYS_NO_PATHCONV=1
cd "$(dirname "$0")/.."
DIR=crates/elf/tests/llvm

for xlen in 32 64; do
    out="$DIR/rv$xlen"
    mkdir -p "$out"
    if [ $xlen = 64 ]; then abi=lp64; else abi=ilp32; fi
    # -fdebug-compilation-dir: the line tables name /camelot, not this machine.
    flags="--target=riscv$xlen -march=rv${xlen}ima_zicsr_zifencei -mabi=$abi -mcmodel=medany
        -O0 -g -fdebug-compilation-dir=/camelot -ffreestanding -fno-pic"
    (cd "$DIR" && clang $flags -mno-relax -c prog.c -o "rv$xlen/prog.o")
    (cd "$DIR" && clang $flags -mrelax -c prog.c -o "rv$xlen/prog-relax.o")
    ld.lld -Ttext=0x80000000 -e _start "$out/prog.o" -o "$out/prog.elf"
    for f in prog.o prog-relax.o prog.elf; do
        # Run in the directory: both tools name the file they read.
        (cd "$out" && llvm-readelf -h -S -l -s -r "$f" > "$f.readelf")
        (cd "$out" && llvm-dwarfdump --debug-line "$f" > "$f.lines")
        (cd "$out" && llvm-dwarfdump --debug-info "$f" > "$f.info")
    done
done
echo "wrote $(ls "$DIR"/rv*/* | wc -l) files"
