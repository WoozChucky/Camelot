#!/bin/bash
# Regenerates merlin-objdump's test fixtures and golden files with LLVM
# (clang + llvm-objdump; the official Windows LLVM installer works).
#
#   scripts/gen-objdump-golden.sh
#
# 1. Assembles the fixture sources in crates/merlin-objdump/tests/fixtures/
#    into .o files with clang (hello.o is examples/hello/hello.S), and links
#    linked.elf with ld.lld and linked.ld. hello.elf is examples/hello built
#    by clang and ld.lld with linker relaxation (what examples/hello/hello.elf
#    was before our own tools built it): an executable with everything ld.lld
#    writes (.riscv.attributes, .comment, PT_GNU_STACK, mapping symbols).
# 2. Runs llvm-objdump over those files and a few riscv-tests ELFs, writing
#    tests/golden/<name>.txt.
#
# hello.S writes the Xcal instructions as mnemonics, which clang learns from
# examples/xcal.inc (-include).
#
# Paths are passed relative to the repository root because llvm-objdump
# prints them in its header line; tests/golden.rs uses the same paths.
# The golden files are made with LLVM 23, which the script checks: versions
# differ in what they print (LLVM 23 names hints such as `pause` and
# `prefetch.i` and prints data in code under `$d` as `.word`, which LLVM 21
# did not; hex immediates are the default since LLVM 19).
# Keep the list below in sync with CASES in crates/merlin-objdump/tests/golden.rs.
set -eu
cd "$(dirname "$0")/.."
FIX=crates/merlin-objdump/tests/fixtures
OUT=crates/merlin-objdump/tests/golden
mkdir -p "$OUT"

LLVM_MAJOR=23
for tool in clang ld.lld llvm-objdump; do
    # "clang version 23.1.4", "LLVM version 23.1.4", "LLD 23.1.4".
    version=$("$tool" --version | grep -oE '(version|LLD) [0-9]+' | head -n 1 | cut -d ' ' -f 2)
    if [ "$version" != "$LLVM_MAJOR" ]; then
        echo "$tool is LLVM ${version:-?}, expected LLVM $LLVM_MAJOR" >&2
        exit 1
    fi
done

CC="clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -c"
# isa.S has the A instructions too: its .riscv.attributes names A.
clang --target=riscv32 -march=rv32ima_zicsr_zifencei -mabi=ilp32 -c -mno-relax "$FIX/isa.S" -o "$FIX/isa.o"
$CC "$FIX/relocs.S" -o "$FIX/relocs.o"
$CC -include examples/xcal.inc examples/hello/hello.S -o "$FIX/hello.o"
clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -nostdlib -static -fuse-ld=lld \
  -include examples/xcal.inc -T examples/hello/link.ld examples/hello/hello.S -o "$FIX/hello.elf"
$CC -mno-relax "$FIX/misc.S" -o "$FIX/misc.o"
# -fdebug-compilation-dir keeps the build directory out of the debug info.
$CC -O1 -g -fdebug-compilation-dir=. "$FIX/debug.c" -o "$FIX/debug.o"
$CC -mno-relax "$FIX/linked.S" -o "$FIX/linked.o"
ld.lld -T "$FIX/linked.ld" "$FIX/linked.o" -o "$FIX/linked.elf"
rm "$FIX/linked.o"
# lines.s is merlin -g output: its line table and compile unit, linked.
clang --target=riscv32 -march=rv32ima_zicsr_zifencei -mabi=ilp32 -c -mno-relax "$FIX/lines.s" -o "$FIX/lines.o"
ld.lld -Ttext=0x10000 -e add "$FIX/lines.o" -o "$FIX/lines.elf"
rm "$FIX/lines.o"

gen() { # golden-name input llvm-objdump-args...
  local name=$1 input=$2
  shift 2
  llvm-objdump "$@" "$input" > "$OUT/$name.txt"
}
# -l: source paths with / (llvm-objdump joins them with \ on Windows).
gen_lines() { # golden-name input llvm-objdump-args...
  gen "$@"
  tr '\134' '/' < "$OUT/$1.txt" > "$OUT/$1.tmp" && mv "$OUT/$1.tmp" "$OUT/$1.txt"
}

gen hello-elf.d                "$FIX/hello.elf" -d
gen hello-elf.d-no-raw         "$FIX/hello.elf" -d --no-show-raw-insn
gen hello-elf.d-no-aliases     "$FIX/hello.elf" -d -M no-aliases
gen hello-elf.x                "$FIX/hello.elf" -x
gen hello-elf.s                "$FIX/hello.elf" -s
gen hello-o.dr                 "$FIX/hello.o" -dr
gen hello-o.x                  "$FIX/hello.o" -x
gen isa-o.d                    "$FIX/isa.o" -d
gen isa-o.d-no-aliases         "$FIX/isa.o" -d -M no-aliases --no-show-raw-insn
gen relocs-o.d                 "$FIX/relocs.o" -d
gen relocs-o.dr                "$FIX/relocs.o" -dr
gen relocs-o.r                 "$FIX/relocs.o" -r
gen relocs-o.t                 "$FIX/relocs.o" -t
gen relocs-o.h                 "$FIX/relocs.o" -h
gen relocs-o.x                 "$FIX/relocs.o" -x
gen relocs-o.s                 "$FIX/relocs.o" -s
gen misc-o.x                   "$FIX/misc.o" -x
gen misc-o.dr                  "$FIX/misc.o" -dr
gen debug-o.d                  "$FIX/debug.o" -d
gen debug-o.h                  "$FIX/debug.o" -h
gen_lines debug-o.dl           "$FIX/debug.o" -d -l
gen_lines lines-elf.dl         "$FIX/lines.elf" -d -l
gen linked-elf.d               "$FIX/linked.elf" -d
gen linked-elf.x               "$FIX/linked.elf" -x
# The riscv-tests ELFs' mapping symbols ($x.0) make llvm-objdump warn on
# stderr that they name no ISA.
gen rv32ui-p-add.d             tests/riscv-tests/rv32ui-p-add -d
gen rv32mi-p-csr.d             tests/riscv-tests/rv32mi-p-csr -d
gen rv32si-p-dirty.dx          tests/riscv-tests/rv32si-p-dirty -d -x
gen rv32ua-p-lrsc.d            tests/riscv-tests/rv32ua-p-lrsc -d

echo "wrote $(ls "$OUT"/*.txt | wc -l) golden files"
