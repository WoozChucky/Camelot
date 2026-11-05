#!/bin/bash
# Regenerates crates/merlin-as/tests/mapping/*.o and *.d.txt, the reference
# for merlin-as's mapping symbols ($x<isa>, $d) and the padding of code
# (tests/mapping.rs): each .s there assembled by clang's integrated
# assembler (LLVM 23), with -mno-relax like merlin-as, and `llvm-objdump -d -r`
# of the object, which decodes by those symbols (Camelot's /bin/as assembles
# them too: crates/xtask/tests/selfhost.rs).
#
# Needs clang and llvm-objdump from LLVM 23, plus bash (Git Bash on Windows).
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/mapping" && pwd)
# gen <out dir> <source>
gen() {
  local name
  name=$(basename "${2%.s}")
  clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mno-relax -c -x assembler "$2" -o "$1/$name.o"
  # dot.s's listing would name clang's `.L0 ` labels, which merlin-as calls
  # otherwise: only its symbols are compared.
  [ "$name" = dot ] && return
  # Run inside the directory so the header line names plain "<name>.o".
  (cd "$1" && llvm-objdump -d -r "$name.o") | tr -d '\r' > "$1/$name.d.txt"
}
for s in "$DIR"/*.s; do
  gen "$DIR" "$s"
done
echo "wrote $(ls "$DIR"/*.o | wc -l) objects"
