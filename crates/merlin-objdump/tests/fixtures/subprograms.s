# merlin-objdump fixture for -l: function names from the debug information,
# where no symbol covers the code (LLVM's symbolizer prefers the symbol
# table, and falls back to the innermost subprogram or inlined subroutine of
# the compile unit, by its linkage name, else its name, through
# DW_AT_abstract_origin and DW_AT_specification). The symbol f covers only
# its first 8 bytes. A compile unit written by hand, as merlin -g writes one.
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv32 -march=rv32ima_zicsr_zifencei -mabi=ilp32 -c -mno-relax subprograms.s -o subprograms.o

    .text
    .file 0 "/src" "sub.c"
    .globl f
    .type f, @function
f:
.Louter:
    .loc 0 1 0
    nop                         # f (the symbol)
    nop
    .loc 0 2 0
    nop                         # outer (the subprogram)
    nop
.Linlined:
    .loc 0 3 0
    nop                         # inlined (its abstract origin's name)
    nop
.Linlined_end:
.Louter_end:
.Llinked:
    .loc 0 4 0
    nop                         # _Z6linkedv (the linkage name first)
    nop
.Llinked_end:
.Ldefined:
    .loc 0 5 0
    nop                         # declared (the declaration's name)
    nop
.Ldefined_end:
    .loc 0 6 0
    nop                         # no subprogram: no name
.Lend:
    .size f, 8

    .section .debug_abbrev,"",@progbits
    .byte 1, 0x11, 1            # compile unit, children
    .byte 0x25, 0x08            # producer, string
    .byte 0x03, 0x08            # name, string
    .byte 0x1b, 0x08            # comp_dir, string
    .byte 0x10, 0x17            # stmt_list, sec_offset
    .byte 0x11, 0x01            # low_pc, addr
    .byte 0x12, 0x06            # high_pc, data4
    .byte 0, 0
    .byte 2, 0x2e, 1            # subprogram, children
    .byte 0x11, 0x01, 0x12, 0x06
    .byte 0x03, 0x08            # name
    .byte 0, 0
    .byte 3, 0x2e, 0            # abstract subprogram
    .byte 0x03, 0x08
    .byte 0x20, 0x0b            # inline, data1
    .byte 0, 0
    .byte 4, 0x1d, 0            # inlined subroutine
    .byte 0x31, 0x13            # abstract_origin, ref4
    .byte 0x11, 0x01, 0x12, 0x06
    .byte 0, 0
    .byte 5, 0x2e, 0            # subprogram with a linkage name
    .byte 0x11, 0x01, 0x12, 0x06
    .byte 0x6e, 0x08            # linkage_name
    .byte 0x03, 0x08
    .byte 0, 0
    .byte 6, 0x2e, 0            # declaration
    .byte 0x03, 0x08
    .byte 0x3c, 0x19            # declaration, flag_present
    .byte 0, 0
    .byte 7, 0x2e, 0            # definition of a declaration
    .byte 0x47, 0x13            # specification, ref4
    .byte 0x11, 0x01, 0x12, 0x06
    .byte 0, 0
    .byte 0

    .section .debug_info,"",@progbits
.Lcu:
    .word .Lcu_end - .Lcu - 4
    .half 5
    .byte 1                     # DW_UT_compile
    .byte 4                     # address size
    .word 0                     # abbreviations
    .byte 1
    .asciz "hand"
    .asciz "sub.c"
    .asciz "/src"
    .word .Lline_table_start0
    .word f
    .word .Lend - .Louter
.Labstract:
    .byte 3
    .asciz "inlined"
    .byte 1
.Ldeclaration:
    .byte 6
    .asciz "declared"
    .byte 2
    .word f
    .word .Louter_end - .Louter
    .asciz "outer"
    .byte 4
    .word .Labstract - .Lcu
    .word f + (.Linlined - .Louter)
    .word .Linlined_end - .Linlined
    .byte 0                     # outer's children end
    .byte 5
    .word f + (.Llinked - .Louter)
    .word .Llinked_end - .Llinked
    .asciz "_Z6linkedv"
    .asciz "linked"
    .byte 7
    .word .Ldeclaration - .Lcu
    .word f + (.Ldefined - .Louter)
    .word .Ldefined_end - .Ldefined
    .byte 0                     # the unit's children end
.Lcu_end:

    .section .debug_line,"",@progbits
.Lline_table_start0:
