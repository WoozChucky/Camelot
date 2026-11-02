#!/bin/bash
# Regenerates the merlin-ld test fixtures in crates/merlin-ld/tests/fixtures
# with clang + ld.lld (official Windows LLVM installer works, from Git Bash).
#
# For each fixture set it assembles the .S sources with clang into the
# committed .o files, links them with ld.lld under the set's linker script, and
# records `llvm-readelf -S -l -s` of the ld.lld executable as expected.txt.
# The merlin-ld tests link the same .o files and compare section addresses,
# PT_LOAD segments and symbol values against expected.txt.
#
# Objects are built with -mno-relax: merlin-ld does no linker relaxation, and
# ld.lld must not shrink code either or the layouts would differ.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
FIX="$ROOT/crates/merlin-ld/tests/fixtures"
CC="clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -c"
READELF="llvm-readobj --elf-output-style=GNU"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# examples/hello under its own link.ld (it has relocations; only the layout is compared).
cd "$FIX/hello"
$CC "$ROOT/examples/hello/hello.S" -o hello.o
ld.lld -T "$ROOT/examples/hello/link.ld" hello.o -o "$TMP/hello.elf"
(cd "$TMP" && $READELF -S -l -s hello.elf) > expected.txt

# Relocation-free objects: linked end to end and run in Grail.
cd "$FIX/norelocs"
for f in start data more; do
  $CC "$f.S" -o "$f.o"
done
ld.lld -T link.ld start.o data.o more.o -o "$TMP/norelocs.elf"
(cd "$TMP" && $READELF -S -l -s norelocs.elf) > expected.txt

# Symbol-resolution objects (layout tests only, no expected output).
cd "$FIX/symbols"
$CC weak.S -o weak.o

echo "fixtures regenerated in $FIX"
