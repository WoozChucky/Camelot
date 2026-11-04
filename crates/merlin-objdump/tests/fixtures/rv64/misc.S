# merlin-objdump fixture, RV64: misc.S for RV64 (8-byte entries in the
# init and fini arrays, 16-digit columns in -h, -t and -r).
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -c -mno-relax rv64/misc.S -o rv64/misc.o

    .text
    .globl entry
    .hidden entry
    .type entry, @function
entry:
    nop
    nop
    .reloc 4, R_RISCV_NONE, bar
    .reloc 0, R_RISCV_NONE, baz
    nop
    ret
    .size entry, .-entry

    .comm common_buf, 32, 8

    .data
    .globl prot
    .protected prot
prot:
    .dword 1

    .section .init_array, "aw", @init_array
    .dword entry
    .section .preinit_array, "aw", @preinit_array
    .dword entry
    .section .fini_array, "aw", @fini_array
    .dword entry
    .section .note.x, "a", @note
    .word 0
    .section .debug_x, "", @progbits
    .word 0
