#!/bin/bash
# Regenerates the archive fixtures of the elf crate (crates/elf/tests/archive)
# with clang and llvm-ar, which crates/elf/tests/archive.rs holds the elf
# crate's archive writer to, and through it Camelot's /bin/ar
# (crates/xtask/tests/selfhost.rs).
#
#   scripts/gen-archive-golden.sh
#
# Each X.s becomes X.o (clang), and llvm-ar archives them in its default
# deterministic mode (times, ids 0, mode 644). --format=gnu: the System V
# format with GNU's long names, what llvm-ar writes for ELF members on
# every host, and for any member on Linux (on Windows it would pick COFF's
# for an archive whose first member is not an object, or that has none).
#
#   objs.a      a.o (every kind of symbol), nosyms.o, b.o, the long-named
#               a_very_long_member_name.o (b.o's bytes) and notes.txt, a
#               text member of odd size
#   nosyms.a    nosyms.o alone: a symbol table with no symbols
#   short.a     a.o, nosyms.o, b.o, c.o and notes.txt: names a Camelot
#               directory holds (14 bytes at most), for /bin/ar's test
#   longnames.a names of 15 and 16 bytes, and a long name twice
#   replaced.a  objs.a after `llvm-ar rcs replaced.a new/a.o c.o`: a.o
#               replaced in place, c.o added at the end
#   nosymtab.a  longnames.a's members without a symbol table (rcS)
#   text.a      notes.txt and more.txt: no object, so no symbol table at all
#   empty.a     no members
#
# Made with LLVM 23.
set -eu
cd "$(dirname "$0")/../crates/elf/tests/archive"
flags="--target=riscv64-unknown-elf -march=rv64ima_zicsr_zifencei -mno-relax"
for s in a b c nosyms new/a; do
    clang $flags -c "$s.s" -o "$s.o"
done
cp b.o a_very_long_member_name.o
cp nosyms.o abcdefghijklm.o
cp a.o abcdefghijklmn.o
rm -f ./*.a
llvm-ar --format=gnu rcs objs.a a.o nosyms.o b.o a_very_long_member_name.o notes.txt
llvm-ar --format=gnu rcs nosyms.a nosyms.o
llvm-ar --format=gnu rcs short.a a.o nosyms.o b.o c.o notes.txt
llvm-ar --format=gnu rcs longnames.a abcdefghijklm.o abcdefghijklmn.o a_very_long_member_name.o \
    a_very_long_member_name.o
cp objs.a replaced.a
llvm-ar --format=gnu rcs replaced.a new/a.o c.o
llvm-ar --format=gnu rcS nosymtab.a abcdefghijklm.o abcdefghijklmn.o a_very_long_member_name.o \
    a_very_long_member_name.o
llvm-ar --format=gnu rcs text.a notes.txt more.txt
llvm-ar --format=gnu rcs empty.a
