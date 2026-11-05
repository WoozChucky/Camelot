# merlin -g -fdebug-compilation-dir=src -march=rv64ima_zicsr_zifencei m.c:
# ../merlin.s for RV64 (LP64), the same m.c and the same line table, with
# what changes with the width (written by hand first, it is what merlin
# writes since #152). Registers are saved and pushed 8 bytes at a time
# (sd/ld), ints stay 4 bytes (sw/lw, and addw/addiw/mulw on them), and in
# .debug_info the address
# size is 8 and every DW_FORM_addr (the low pcs) is a .quad, which takes an
# R_RISCV_64; lengths and offsets stay 4 bytes (DWARF32). m.c, whose header
# inc/h.h has a function (static int twice(int x) { return x * 2; }):
#   #include "inc/h.h"
#   int g;
#   int add(int a, int b) {
#       int c = a + b;
#       for (int i = 0; i < 3; i++)
#           c += twice(i);
#       while (c > 100)
#           c--;
#       if (c)
#           g = c;
#       return c;
#   }
#   void nothing(void) {
#   }
    .file 0 "src" "m.c" md5 0x54ec47e7dca72bbb5aa63c2a481fb712

    .text

    # int add(int a, int b): frame 32 bytes
    #     -20(s0)  param int a
    #     -24(s0)  param int b
    #     -28(s0)  local int c
    #     -32(s0)  local int i
    .globl add
    .type add, @function
add:
    .loc 0 3 0
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    sw    a1, -24(s0)
    .loc 0 4 9 prologue_end
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
.Ldbg.block.1:
    .loc 0 5 14
    li    a0, 0
    sw    a0, -32(s0)
.L.loop.1:
    .loc 0 5 23
    lw    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    .loc 0 6 9
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -32(s0)
    addi  sp, sp, -8
    call  twice
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
.L.continue.2:
    .loc 0 5 29
    lw    a0, -32(s0)
    addiw a0, a0, 1
    sw    a0, -32(s0)
    addiw a0, a0, -1
    j     .L.loop.1
.L.break.3:
.Ldbg.block_end.1:
.L.continue.4:
    .loc 0 7 14
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 100
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a0, a1
    beqz  a0, .L.break.5
    .loc 0 8 9
    lw    a0, -28(s0)
    addiw a0, a0, -1
    sw    a0, -28(s0)
    addiw a0, a0, 1
    j     .L.continue.4
.L.break.5:
    .loc 0 9 5
    lw    a0, -28(s0)
    beqz  a0, .L.end.6
    .loc 0 10 9
    la    a0, g
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -28(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
.L.end.6:
    .loc 0 11 5
    lw    a0, -28(s0)
    .loc 0 12 1
.L.return.add:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
.Lfunc_end.add:
    .size add, . - add

    # int twice(int x): frame 32 bytes
    #     -20(s0)  param int x
    .type twice, @function
twice:
    .file 1 "./inc" "h.h" md5 0x5cb865eca3e578cf21a8ed9ff78cb654
    .loc 1 1 0
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    .loc 1 1 27 prologue_end
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
    .loc 1 1 41
.L.return.twice:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
.Lfunc_end.twice:
    .size twice, . - twice

    .bss

    # int g: 4 bytes
    .globl g
    .balign 4
    .type g, @object
    .size g, 4
g:
    .zero 4

    .text

    # void nothing(void): frame 16 bytes
    .globl nothing
    .type nothing, @function
nothing:
    .loc 0 13 0
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    .loc 0 14 1 prologue_end
    li    a0, 0
.L.return.nothing:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
.Lfunc_end.nothing:
    .size nothing, . - nothing

    .section .debug_abbrev,"",@progbits
.Ldebug_abbrev0:
    .byte 0x01, 0x11, 0x01    # 1: DW_TAG_compile_unit
    .byte 0x25, 0x25
    .byte 0x13, 0x05
    .byte 0x03, 0x25
    .byte 0x72, 0x17
    .byte 0x10, 0x17
    .byte 0x1b, 0x25
    .byte 0x11, 0x1b
    .byte 0x12, 0x06
    .byte 0x73, 0x17
    .byte 0, 0
    .byte 0x02, 0x2e, 0x01    # 2: DW_TAG_subprogram
    .byte 0x11, 0x1b
    .byte 0x12, 0x06
    .byte 0x40, 0x18
    .byte 0x03, 0x25
    .byte 0x3a, 0x0b
    .byte 0x3b, 0x0b
    .byte 0x27, 0x19
    .byte 0x49, 0x13
    .byte 0x3f, 0x19
    .byte 0, 0
    .byte 0x03, 0x05, 0x00    # 3: DW_TAG_formal_parameter
    .byte 0x02, 0x18
    .byte 0x03, 0x25
    .byte 0x3a, 0x0b
    .byte 0x3b, 0x0b
    .byte 0x49, 0x13
    .byte 0, 0
    .byte 0x04, 0x34, 0x00    # 4: DW_TAG_variable
    .byte 0x02, 0x18
    .byte 0x03, 0x25
    .byte 0x3a, 0x0b
    .byte 0x3b, 0x0b
    .byte 0x49, 0x13
    .byte 0, 0
    .byte 0x05, 0x0b, 0x01    # 5: DW_TAG_lexical_block
    .byte 0x11, 0x1b
    .byte 0x12, 0x06
    .byte 0, 0
    .byte 0x06, 0x2e, 0x01    # 6: DW_TAG_subprogram
    .byte 0x11, 0x1b
    .byte 0x12, 0x06
    .byte 0x40, 0x18
    .byte 0x03, 0x25
    .byte 0x3a, 0x0b
    .byte 0x3b, 0x0b
    .byte 0x27, 0x19
    .byte 0x49, 0x13
    .byte 0, 0
    .byte 0x07, 0x2e, 0x00    # 7: DW_TAG_subprogram
    .byte 0x11, 0x1b
    .byte 0x12, 0x06
    .byte 0x40, 0x18
    .byte 0x03, 0x25
    .byte 0x3a, 0x0b
    .byte 0x3b, 0x0b
    .byte 0x27, 0x19
    .byte 0x3f, 0x19
    .byte 0, 0
    .byte 0x08, 0x34, 0x00    # 8: DW_TAG_variable
    .byte 0x03, 0x25
    .byte 0x49, 0x13
    .byte 0x3f, 0x19
    .byte 0x3a, 0x0b
    .byte 0x3b, 0x0b
    .byte 0x02, 0x18
    .byte 0, 0
    .byte 0x09, 0x24, 0x00    # 9: DW_TAG_base_type
    .byte 0x03, 0x25
    .byte 0x3e, 0x0b
    .byte 0x0b, 0x0b
    .byte 0, 0
    .byte 0    # the end of the abbreviations

    .section .debug_info,"",@progbits
    .word 152    # unit length
    .half 5    # DWARF version
    .byte 1    # a compile unit
    .byte 8    # address size
    .word .Ldebug_abbrev0
    # DW_TAG_compile_unit: the compile unit
    .byte 0x01
    .byte 0x00
    .half 12
    .byte 0x01
    .word .Lstr_offsets_base0
    .word .Lline_table_start0
    .byte 0x02
    .byte 0x00
    .word .Lfunc_end.nothing - add
    .word .Laddr_table_base0
    #   DW_TAG_subprogram: add
    .byte 0x02
    .byte 0x00
    .word .Lfunc_end.add - add
    .byte 0x01    # an expression of 1 byte
    .byte 0x58
    .byte 0x03
    .byte 0
    .byte 3
    .word 151
    #     DW_TAG_formal_parameter: a
    .byte 0x03
    .byte 0x02    # an expression of 2 bytes
    .byte 0x91, 0x6c
    .byte 0x04
    .byte 0
    .byte 3
    .word 151
    #     DW_TAG_formal_parameter: b
    .byte 0x03
    .byte 0x02    # an expression of 2 bytes
    .byte 0x91, 0x68
    .byte 0x05
    .byte 0
    .byte 3
    .word 151
    #     DW_TAG_variable: c
    .byte 0x04
    .byte 0x02    # an expression of 2 bytes
    .byte 0x91, 0x64
    .byte 0x06
    .byte 0
    .byte 4
    .word 151
    #     DW_TAG_lexical_block: a block
    .byte 0x05
    .byte 0x01
    .word .Ldbg.block_end.1 - .Ldbg.block.1
    #       DW_TAG_variable: i
    .byte 0x04
    .byte 0x02    # an expression of 2 bytes
    .byte 0x91, 0x60
    .byte 0x07
    .byte 0
    .byte 5
    .word 151
    .byte 0    # the end of the children
    .byte 0    # the end of the children
    #   DW_TAG_subprogram: twice
    .byte 0x06
    .byte 0x02
    .word .Lfunc_end.twice - twice
    .byte 0x01    # an expression of 1 byte
    .byte 0x58
    .byte 0x08
    .byte 1
    .byte 1
    .word 151
    #     DW_TAG_formal_parameter: x
    .byte 0x03
    .byte 0x02    # an expression of 2 bytes
    .byte 0x91, 0x6c
    .byte 0x09
    .byte 1
    .byte 1
    .word 151
    .byte 0    # the end of the children
    #   DW_TAG_subprogram: nothing
    .byte 0x07
    .byte 0x03
    .word .Lfunc_end.nothing - nothing
    .byte 0x01    # an expression of 1 byte
    .byte 0x58
    .byte 0x0a
    .byte 0
    .byte 13
    #   DW_TAG_variable: g
    .byte 0x08
    .byte 0x0b
    .word 151
    .byte 0
    .byte 2
    .byte 0x02    # an expression of 2 bytes
    .byte 0xa1, 0x04
    #   DW_TAG_base_type: int
    .byte 0x09
    .byte 0x0c
    .byte 5
    .byte 4
    .byte 0    # the end of the children

    .section .debug_str_offsets,"",@progbits
    .word 56    # length
    .half 5    # DWARF version
    .half 0    # padding
.Lstr_offsets_base0:
    .word .Linfo_string0
    .word .Linfo_string1
    .word .Linfo_string2
    .word .Linfo_string3
    .word .Linfo_string4
    .word .Linfo_string5
    .word .Linfo_string6
    .word .Linfo_string7
    .word .Linfo_string8
    .word .Linfo_string9
    .word .Linfo_string10
    .word .Linfo_string11
    .word .Linfo_string12

    .section .debug_str,"MS",@progbits,1
.Linfo_string0:
    .asciz "merlin"
.Linfo_string1:
    .asciz "m.c"
.Linfo_string2:
    .asciz "src"
.Linfo_string3:
    .asciz "add"
.Linfo_string4:
    .asciz "a"
.Linfo_string5:
    .asciz "b"
.Linfo_string6:
    .asciz "c"
.Linfo_string7:
    .asciz "i"
.Linfo_string8:
    .asciz "twice"
.Linfo_string9:
    .asciz "x"
.Linfo_string10:
    .asciz "nothing"
.Linfo_string11:
    .asciz "g"
.Linfo_string12:
    .asciz "int"

    .section .debug_addr,"",@progbits
    .word 44    # length
    .half 5    # DWARF version
    .byte 8    # address size
    .byte 0    # segment selector size
.Laddr_table_base0:
    .quad add
    .quad .Ldbg.block.1
    .quad twice
    .quad nothing
    .quad g

    .section .debug_line,"",@progbits
.Lline_table_start0:
