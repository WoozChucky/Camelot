#!/bin/bash
# Regenerates merlin-objdump's test fixtures and golden files with LLVM
# (clang + llvm-objdump; the official Windows LLVM installer works).
#
#   scripts/gen-objdump-golden.sh
#
# 1. Assembles the fixture sources in crates/merlin-objdump/tests/fixtures/
#    into .o files with clang (hello.o is examples/hello/hello.S).
# 2. Runs llvm-objdump over those objects, examples/hello/hello.elf and a few
#    riscv-tests ELFs, writing tests/golden/<name>.txt.
#
# Paths are passed relative to the repository root because llvm-objdump
# prints them in its header line; tests/golden.rs uses the same paths.
# The golden files were made with LLVM 21 (hex immediates are the default
# since LLVM 19; older versions print decimal and will not match).
# Keep the list below in sync with CASES in crates/merlin-objdump/tests/golden.rs.
set -eu
cd "$(dirname "$0")/.."
FIX=crates/merlin-objdump/tests/fixtures
OUT=crates/merlin-objdump/tests/golden
mkdir -p "$OUT"

CC="clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -c"
$CC -mno-relax "$FIX/isa.S" -o "$FIX/isa.o"
$CC "$FIX/relocs.S" -o "$FIX/relocs.o"
$CC examples/hello/hello.S -o "$FIX/hello.o"

gen() { # golden-name input llvm-objdump-args...
  local name=$1 input=$2
  shift 2
  llvm-objdump "$@" "$input" > "$OUT/$name.txt"
}

gen hello-elf.d                examples/hello/hello.elf -d
gen hello-elf.d-no-raw         examples/hello/hello.elf -d --no-show-raw-insn
gen hello-elf.d-no-aliases     examples/hello/hello.elf -d -M no-aliases
gen hello-elf.x                examples/hello/hello.elf -x
gen hello-elf.s                examples/hello/hello.elf -s
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
gen rv32ui-p-add.d             tests/riscv-tests/rv32ui-p-add -d
gen rv32mi-p-csr.d             tests/riscv-tests/rv32mi-p-csr -d
gen rv32si-p-dirty.dx          tests/riscv-tests/rv32si-p-dirty -d -x

echo "wrote $(ls "$OUT" | wc -l) golden files"
