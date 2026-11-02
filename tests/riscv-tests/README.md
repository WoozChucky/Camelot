Built from riscv-software-src/riscv-tests @ 3df703c with scripts/build-riscv-tests.sh. BSD-licensed, see LICENSE.

`obj/` holds an object for each executable, assembled from the same preprocessed source with `-mno-relax`, and the env's `link.ld`: merlin-ld's tests relink them and check that the loaded bytes equal these executables'.
