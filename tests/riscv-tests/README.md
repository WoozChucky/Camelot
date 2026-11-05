Built from riscv-software-src/riscv-tests @ 3df703c with scripts/build-riscv-tests.sh. BSD-licensed, see LICENSE.

Suites: rv64ui, rv64um, rv64ua, rv64mi and rv64si (the `p` environment), built with `-march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany` (clang and ld.lld 23: `scripts/build-riscv-tests.sh <riscv-tests> <out> rv64ui rv64um rv64ua rv64mi rv64si`). Grail runs every one but `rv64ui-p-ma_data` (spec section 4).

`obj/` holds an object for each executable, assembled from the same preprocessed source with `-mno-relax`, and the env's `link.ld`: merlin-ld's tests (`crates/merlin-ld/tests/riscv_tests.rs`) relink them and check that the loaded bytes and every field of the program headers equal these executables', linked by the LLD merlin-ld follows (23.1.4).

The RV32 suites (rv32ui, rv32um, rv32ua, rv32mi, rv32si) were here until Excalibur became RV64 only (#159); four of their tests stay as merlin-objdump's fixtures (`crates/merlin-objdump/tests/fixtures/rv32`), for the disassembler, which still reads RV32.
