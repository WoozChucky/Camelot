# The other side of main.S's cross-object references: functions and data it
# uses, and references back into main.o (calls, a jump, a branch, a data
# word). Built by `cargo xtask ld-fixtures` (with -mno-relax).

    .text
    .globl lib_double, lib_calls_back, lib_check, lib_inc
lib_double:                         # a0 * 2
    slli  a0, a0, 1
    ret

lib_calls_back:                     # main_tail(main_helper(main_helper(3)))
    addi  sp, sp, -16
    sw    ra, 12(sp)
    li    a0, 3
    call  main_helper               # CALL_PLT, backward into main.o
    jal   main_helper               # JAL, backward into main.o
    lw    ra, 12(sp)
    addi  sp, sp, 16
    tail  main_tail                 # CALL_PLT through t1, no return here

lib_check:                          # a0 must still be 0x12345678
    li    t0, 0x12345678
    li    a7, 50
    .option push
    .option exact
    bne   a0, t0, fail              # BRANCH, backward into main.o
    .option pop
    ret

lib_inc:                            # a0 + 1; main.o's reference to it is weak
    addi  a0, a0, 1
    ret

    .data
    .globl lib_value, lib_ptr
    .balign 4
lib_value:
    .word 0x12345678
lib_ptr:
    .word main_helper               # R_RISCV_32 into main.o
