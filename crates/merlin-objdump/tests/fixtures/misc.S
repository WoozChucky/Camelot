# merlin-objdump fixture: section and symbol kinds the other fixtures lack,
# for -h/-t/-r: a COMMON symbol, hidden and protected visibility, sections
# llvm-objdump counts as DATA although they are not SHT_PROGBITS
# (.init_array, .preinit_array, .fini_array, an alloc note), a DEBUG
# section, and relocations stored out of offset order (-r keeps file order,
# -dr sorts them under their instructions).
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -c -mno-relax misc.S -o misc.o

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

    .comm common_buf, 32, 4

    .data
    .globl prot
    .protected prot
prot:
    .word 1

    .section .init_array, "aw", @init_array
    .word entry
    .section .preinit_array, "aw", @preinit_array
    .word entry
    .section .fini_array, "aw", @fini_array
    .word entry
    .section .note.x, "a", @note
    .word 0
    .section .debug_x, "", @progbits
    .word 0
