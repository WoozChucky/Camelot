Built from riscv-software-src/riscv-tests @ 3df703c with scripts/build-riscv-tests.sh. BSD-licensed, see LICENSE.

Suites: rv32ui, rv32um, rv32ua, rv32mi, rv32si (the `p` environment). rv32ua is built with `-march=rv32ima_zicsr_zifencei` (clang and ld.lld 21 on Windows: `scripts/build-riscv-tests.sh <riscv-tests> <out> rv32ua`); the others with `rv32im_zicsr_zifencei` (ld.lld 18). Rebuilding the others with LLVM 21 gives the same objects; the executables then differ in the linker's `.comment` (and the offsets after it).

`rv64/` holds the RV64 suites rv64ui, rv64um, rv64ua, rv64mi and rv64si, built with `-march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany` (clang and ld.lld 23: `scripts/build-riscv-tests.sh <riscv-tests> <out> rv64ui rv64um rv64ua rv64mi rv64si`), with their objects and `link.ld` in `rv64/obj/`. Grail runs every one but `rv64ui-p-ma_data` (spec section 4).

`obj/` holds an object for each executable, assembled from the same preprocessed source with `-mno-relax`, and the env's `link.ld`, and so does `rv64/obj/` for the RV64 ones: merlin-ld's tests (`crates/merlin-ld/tests/riscv_tests.rs`) relink both and check that the loaded bytes equal these executables', and for RV64, linked by the LLD merlin-ld follows (23.1.4), every field of their program headers too.
