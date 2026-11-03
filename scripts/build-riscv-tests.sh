#!/bin/bash
# Builds riscv-tests (p env) for RV32 and RV64 with clang/lld.
# Usage: build-riscv-tests.sh <riscv-tests dir> <out dir> [suite ...]
#
# Clone riscv-tests with its env submodule (git clone --recursive) at the
# commit named in tests/riscv-tests/README.md. With no suites given, builds
# rv32ui rv32um rv32ua rv32mi rv32si rv64ui rv64um rv64ua rv64mi.
#
# Writes each RV32 test as an ELF linked by ld.lld (<out dir>/rv32ui-p-add, ...)
# and, for merlin-ld's relink test, the object it was linked from
# (<out dir>/obj/rv32ui-p-add.o) plus the env's linker script (obj/link.ld).
# The RV64 suites go the same way into <out dir>/rv64/ (rv64/rv64ui-p-add,
# rv64/obj/rv64ui-p-add.o, rv64/obj/link.ld).
# Everything is built with -mno-relax: merlin-ld does no linker relaxation.
#
# rv32ua needs the A extension; the other RV32 suites are built without it (as
# they were before Excalibur had A), so rebuilding them gives the committed
# bytes. The RV64 suites are built as RV64 programs are (spec section 8):
# -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany.
set -u
RT=$(realpath "$1"); OUT=$(realpath -m "$2"); shift 2; TMP=$(mktemp -d)
SUITES=${*:-rv32ui rv32um rv32ua rv32mi rv32si rv64ui rv64um rv64ua rv64mi}
for s in $SUITES; do
  case $s in
    rv32ua) FLAGS="--target=riscv32 -march=rv32ima_zicsr_zifencei -mabi=ilp32"; DIR=$OUT ;;
    rv32*) FLAGS="--target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32"; DIR=$OUT ;;
    rv64*) FLAGS="--target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany"; DIR=$OUT/rv64 ;;
    *) echo "unknown suite $s"; continue ;;
  esac
  mkdir -p "$DIR/obj"
  cp "$RT/env/p/link.ld" "$DIR/obj/link.ld"
  for f in "$RT"/isa/$s/*.S; do
    n=$(basename "$f" .S)
    clang $FLAGS -E -P \
      -I "$RT/env/p" -I "$RT/isa/macros/scalar" -I "$(dirname "$f")" "$f" > "$TMP/a.s" || { echo "CPP FAIL $s-$n"; continue; }
    for h in mtvec_handler stvec_handler; do
      grep -q "^ *$h:" "$TMP/a.s" && sed -i "s/\.weak $h;//; s/\.weak $h\b//" "$TMP/a.s"
    done
    sed -i 's/\btselect\b/0x7a0/g; s/\btdata1\b/0x7a1/g; s/\btdata2\b/0x7a2/g; s/\btcontrol\b/0x7a5/g' "$TMP/a.s"
    clang $FLAGS -mno-relax -nostdlib -static -fuse-ld=lld \
      -T "$RT/env/p/link.ld" -x assembler "$TMP/a.s" -o "$DIR/$s-p-$n" 2>"$TMP/err" || echo "FAIL $s-$n: $(grep error "$TMP/err" | head -1)"
    clang $FLAGS -mno-relax -c \
      -x assembler "$TMP/a.s" -o "$DIR/obj/$s-p-$n.o" 2>"$TMP/err" || echo "FAIL $s-$n.o: $(grep error "$TMP/err" | head -1)"
  done
done
rm -rf "$TMP"
