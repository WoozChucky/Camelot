#!/bin/bash
# Regenerates crates/merlin-as/tests/mapping/*.o and *.d.txt, the reference
# for merlin-as's mapping symbols ($x<isa>, $d) and the padding of code
# (tests/mapping.rs): each .s there assembled by clang's integrated
# assembler (LLVM 23), with -mno-relax like merlin-as, and `llvm-objdump -d -r`
# of the object, which decodes by those symbols. rv64/ holds the same for
# RV64, from the same sources (Camelot's /bin/as, RV32, assembles them too:
# crates/xtask/tests/selfhost.rs).
#
# Needs clang and llvm-objdump from LLVM 23, plus bash (Git Bash on Windows).
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/mapping" && pwd)
mkdir -p "$DIR/rv64"
# gen <xlen> <abi> <out dir> <source>
gen() {
  local name
  name=$(basename "${4%.s}")
  clang --target=riscv$1 -march=rv$1ima_zicsr_zifencei -mabi=$2 -mno-relax -c -x assembler "$4" -o "$3/$name.o"
  # dot.s's listing would name clang's `.L0 ` labels, which merlin-as calls
  # otherwise: only its symbols are compared.
  [ "$name" = dot ] && return
  # Run inside the directory so the header line names plain "<name>.o".
  (cd "$3" && llvm-objdump -d -r "$name.o") | tr -d '\r' > "$3/$name.d.txt"
}
for s in "$DIR"/*.s; do
  gen 32 ilp32 "$DIR" "$s"
  gen 64 lp64 "$DIR/rv64" "$s"
done
echo "wrote $(ls "$DIR"/*.o | wc -l) RV32 objects and $(ls "$DIR"/rv64/*.o | wc -l) RV64 objects"
