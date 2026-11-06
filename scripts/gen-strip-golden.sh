#!/bin/bash
# Regenerates the strip fixtures of the elf crate (crates/elf/tests/strip),
# which crates/elf/tests/strip.rs holds elf::strip::strip_debug to: objects
# merlin-as writes for the C library's files, as `cargo xtask kernel`
# builds them (merlin -g for C, merlin -E for .S), and what `llvm-strip -g`
# makes of each, X.o and X.stripped.o.
#
#   scripts/gen-strip-golden.sh
#
#   getenv.o   C with -g: line tables, the debug sections' relocations,
#              symbols in them, a .bss and the file's STT_FILE symbol
#   printf.o   a bigger C file, with .rodata
#   crt0.o     assembly with no debug information: only the string tables
#              written anew
#   memcpy.o   assembly through the preprocessor
#
# Made with LLVM 23 and this repository's merlin and merlin-as.
set -eu
cd "$(dirname "$0")/.."
cargo build --release -q -p merlin -p merlin-as
root=$PWD
out=$root/crates/elf/tests/strip
mkdir -p "$out"
cd kernel/user
for c in getenv printf; do
    "$root/target/release/merlin" -g -fdebug-compilation-dir=kernel/user "lib/$c.c" -o "$out/$c.s"
done
"$root/target/release/merlin" -E -I ../../include crt0.S -o "$out/crt0.s"
"$root/target/release/merlin" -E -I ../../include lib/memcpy.S -o "$out/memcpy.s"
cd "$out"
for f in getenv printf crt0 memcpy; do
    "$root/target/release/merlin-as" "$f.s" -o "$f.o"
    rm "$f.s"
    llvm-strip -g "$f.o" -o "$f.stripped.o"
done
