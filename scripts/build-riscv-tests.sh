#!/bin/bash
# Builds riscv-tests (p env) for RV32 with clang/lld.
# Usage: build-riscv-tests.sh <riscv-tests dir> <out dir> [suite ...]
#
# Clone riscv-tests with its env submodule (git clone --recursive) at the
# commit named in tests/riscv-tests/README.md. With no suites given, builds
# rv32ui rv32um rv32ua rv32mi rv32si.
#
# Writes each test as an ELF linked by ld.lld (<out dir>/rv32ui-p-add, ...)
# and, for merlin-ld's relink test, the object it was linked from
# (<out dir>/obj/rv32ui-p-add.o) plus the env's linker script (obj/link.ld).
# Everything is built with -mno-relax: merlin-ld does no linker relaxation.
#
# rv32ua needs the A extension; the other suites are built without it (as they
# were before Excalibur had A), so rebuilding them gives the committed bytes.
set -u
RT=$(realpath "$1"); OUT=$(realpath -m "$2"); shift 2; mkdir -p "$OUT/obj"; TMP=$(mktemp -d)
SUITES=${*:-rv32ui rv32um rv32ua rv32mi rv32si}
cp "$RT/env/p/link.ld" "$OUT/obj/link.ld"
for s in $SUITES; do
  case $s in
    rv32ua) MARCH=rv32ima_zicsr_zifencei ;;
    *) MARCH=rv32im_zicsr_zifencei ;;
  esac
  for f in "$RT"/isa/$s/*.S; do
    n=$(basename "$f" .S)
    clang --target=riscv32 -march=$MARCH -mabi=ilp32 -E -P \
      -I "$RT/env/p" -I "$RT/isa/macros/scalar" -I "$(dirname "$f")" "$f" > "$TMP/a.s" || { echo "CPP FAIL $s-$n"; continue; }
    for h in mtvec_handler stvec_handler; do
      grep -q "^ *$h:" "$TMP/a.s" && sed -i "s/\.weak $h;//; s/\.weak $h\b//" "$TMP/a.s"
    done
    sed -i 's/\btselect\b/0x7a0/g; s/\btdata1\b/0x7a1/g; s/\btdata2\b/0x7a2/g; s/\btcontrol\b/0x7a5/g' "$TMP/a.s"
    clang --target=riscv32 -march=$MARCH -mabi=ilp32 -mno-relax -nostdlib -static -fuse-ld=lld \
      -T "$RT/env/p/link.ld" -x assembler "$TMP/a.s" -o "$OUT/$s-p-$n" 2>"$TMP/err" || echo "FAIL $s-$n: $(grep error "$TMP/err" | head -1)"
    clang --target=riscv32 -march=$MARCH -mabi=ilp32 -mno-relax -c \
      -x assembler "$TMP/a.s" -o "$OUT/obj/$s-p-$n.o" 2>"$TMP/err" || echo "FAIL $s-$n.o: $(grep error "$TMP/err" | head -1)"
  done
done
rm -rf "$TMP"
