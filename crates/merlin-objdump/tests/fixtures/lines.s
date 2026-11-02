# merlin -g output (crates/merlin-as/tests/debug_line/merlin.s), linked into
# lines.elf by ld.lld for merlin-objdump -l (scripts/gen-objdump-golden.sh).
# merlin -g -fdebug-compilation-dir=src m.c, for this m.c, whose header inc/h.h has
# a function (static int twice(int x) { return x * 2; }):
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
    .file 0 "src" "m.c"

    .text

    # int twice(int x): frame 16 bytes
    #     -12(s0)  param int x
    .type twice, @function
twice:
    .file 1 "src" "inc/h.h"
    .loc 1 1 0
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    .loc 1 2 5 prologue_end
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
    .loc 1 3 1
.L.return.twice:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
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

    # int add(int a, int b): frame 32 bytes
    #     -12(s0)  param int a
    #     -16(s0)  param int b
    #     -20(s0)  local int c
    #     -24(s0)  local int i
    .globl add
    .type add, @function
add:
    .loc 0 3 0
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    .loc 0 4 9 prologue_end
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    .loc 0 5 14
    li    a0, 0
    sw    a0, -24(s0)
.L.loop.1:
    .loc 0 5 23
    lw    a0, -24(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    .loc 0 6 9
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -12
    call  twice
    addi  sp, sp, 12
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
.L.continue.2:
    .loc 0 5 29
    lw    a0, -24(s0)
    addi  a0, a0, 1
    sw    a0, -24(s0)
    addi  a0, a0, -1
    j     .L.loop.1
.L.break.3:
.L.continue.4:
    .loc 0 7 14
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 100
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    beqz  a0, .L.break.5
    .loc 0 8 9
    lw    a0, -20(s0)
    addi  a0, a0, -1
    sw    a0, -20(s0)
    addi  a0, a0, 1
    j     .L.continue.4
.L.break.5:
    .loc 0 9 5
    lw    a0, -20(s0)
    beqz  a0, .L.end.6
    .loc 0 10 9
    la    a0, g
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -20(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
.L.end.6:
    .loc 0 11 5
    lw    a0, -20(s0)
    .loc 0 12 1
.L.return.add:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
.Lfunc_end.add:
    .size add, . - add

    # void nothing(void): frame 16 bytes
    .globl nothing
    .type nothing, @function
nothing:
    .loc 0 13 0
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    .loc 0 14 1 prologue_end
    li    a0, 0
.L.return.nothing:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
.Lfunc_end.nothing:
    .size nothing, . - nothing

    .section .debug_abbrev,"",@progbits
.Ldebug_abbrev0:
    .byte 0x01, 0x11, 0x01    # 1: the compile unit, with children
    .byte 0x25, 0x08
    .byte 0x13, 0x05
    .byte 0x03, 0x08
    .byte 0x1b, 0x08
    .byte 0x10, 0x17
    .byte 0x11, 0x01
    .byte 0x12, 0x06
    .byte 0x00, 0x00
    .byte 0x02, 0x2e, 0x00    # 2: a function
    .byte 0x11, 0x01
    .byte 0x12, 0x06
    .byte 0x03, 0x08
    .byte 0x3a, 0x05
    .byte 0x3b, 0x06
    .byte 0x3f, 0x0c
    .byte 0x00, 0x00
    .byte 0    # the end of the abbreviations

    .section .debug_info,"",@progbits
    .word 105    # unit length
    .half 5    # DWARF version
    .byte 1    # a compile unit
    .byte 4    # address size
    .word .Ldebug_abbrev0
    .byte 1    # the compile unit
    .asciz "merlin"
    .half 0xc    # C99
    .asciz "m.c"
    .asciz "src"
    .word .Lline_table_start0
    .word twice
    .word .Lfunc_end.nothing - twice
    .byte 2    # twice
    .word twice
    .word .Lfunc_end.twice - twice
    .asciz "twice"
    .half 1
    .word 1
    .byte 0
    .byte 2    # add
    .word add
    .word .Lfunc_end.add - add
    .asciz "add"
    .half 0
    .word 3
    .byte 1
    .byte 2    # nothing
    .word nothing
    .word .Lfunc_end.nothing - nothing
    .asciz "nothing"
    .half 0
    .word 13
    .byte 1
    .byte 0    # the end of the compile unit's children

    .section .debug_line,"",@progbits
.Lline_table_start0:
