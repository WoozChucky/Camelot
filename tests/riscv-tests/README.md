Built from riscv-software-src/riscv-tests @ 3df703c with scripts/build-riscv-tests.sh. BSD-licensed, see LICENSE.

Suites: rv32ui, rv32um, rv32ua, rv32mi, rv32si (the `p` environment). rv32ua is built with `-march=rv32ima_zicsr_zifencei` (clang and ld.lld 21 on Windows: `scripts/build-riscv-tests.sh <riscv-tests> <out> rv32ua`); the others with `rv32im_zicsr_zifencei` (ld.lld 18). Rebuilding the others with LLVM 21 gives the same objects; the executables then differ in the linker's `.comment` (and the offsets after it).

`obj/` holds an object for each executable, assembled from the same preprocessed source with `-mno-relax`, and the env's `link.ld`: merlin-ld's tests relink them and check that the loaded bytes equal these executables'.
