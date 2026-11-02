#!/bin/bash
# Builds riscv-tests (p env) for RV32 with clang/lld. Usage: build-rvt.sh <riscv-tests dir> <out dir>
#
# Writes each test as an ELF linked by ld.lld (<out dir>/rv32ui-p-add, ...)
# and, for merlin-ld's relink test, the object it was linked from
# (<out dir>/obj/rv32ui-p-add.o) plus the env's linker script (obj/link.ld).
# Everything is built with -mno-relax: merlin-ld does no linker relaxation.
set -u
RT=$(realpath "$1"); OUT=$(realpath -m "$2"); mkdir -p "$OUT/obj"; TMP=$(mktemp -d)
cp "$RT/env/p/link.ld" "$OUT/obj/link.ld"
for s in rv32ui rv32um rv32mi rv32si; do
  for f in "$RT"/isa/$s/*.S; do
    n=$(basename "$f" .S)
    clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -E -P \
      -I "$RT/env/p" -I "$RT/isa/macros/scalar" -I "$(dirname "$f")" "$f" > "$TMP/a.s" || { echo "CPP FAIL $s-$n"; continue; }
    for h in mtvec_handler stvec_handler; do
      grep -q "^ *$h:" "$TMP/a.s" && sed -i "s/\.weak $h;//; s/\.weak $h\b//" "$TMP/a.s"
    done
    sed -i 's/\btselect\b/0x7a0/g; s/\btdata1\b/0x7a1/g; s/\btdata2\b/0x7a2/g; s/\btcontrol\b/0x7a5/g' "$TMP/a.s"
    clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -mno-relax -nostdlib -static -fuse-ld=lld \
      -T "$RT/env/p/link.ld" -x assembler "$TMP/a.s" -o "$OUT/$s-p-$n" 2>"$TMP/err" || echo "FAIL $s-$n: $(grep error "$TMP/err" | head -1)"
    clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -mno-relax -c \
      -x assembler "$TMP/a.s" -o "$OUT/obj/$s-p-$n.o" 2>"$TMP/err" || echo "FAIL $s-$n.o: $(grep error "$TMP/err" | head -1)"
  done
done
rm -rf "$TMP"
