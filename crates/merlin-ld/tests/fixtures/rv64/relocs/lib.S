# The other side of main.S's cross-object references, for RV64: functions
# and data it uses, and references back into main.o (calls, a jump, a tail
# call, a branch, a .dword pointer). Built by `cargo xtask ld-fixtures`
# (with -mno-relax).

    .text
    .globl lib_double, lib_calls_back, lib_check, lib_inc
lib_double:                         # a0 * 2
    slli  a0, a0, 1
    ret

lib_calls_back:                     # main_tail(main_helper(main_helper(3)))
    addi  sp, sp, -16
    sd    ra, 8(sp)
    li    a0, 3
    call  main_helper               # CALL_PLT, backward into main.o
    jal   main_helper               # JAL, backward into main.o
    ld    ra, 8(sp)
    addi  sp, sp, 16
    tail  main_tail                 # CALL_PLT through t1, no return here

lib_check:                          # a0 must still be lib_value
    ld    t0, lib_value
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
    .globl lib_value, lib_ptr, lib_scratch, lib_word
    .balign 8
lib_value:
    .dword 0x123456789abcdef0
lib_ptr:
    .dword main_helper              # R_RISCV_64 into main.o
lib_scratch:                        # main.o stores here
    .dword 0
lib_word:
    .word 0x89abcdef
