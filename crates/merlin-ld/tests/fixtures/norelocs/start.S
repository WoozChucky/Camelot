# Relocation-free startup code: absolute immediates only, so linking it needs
# no relocation processing. Prints "OK\n" to the UART and halts with code 0.
# Built by scripts/gen-ld-fixtures.sh.
    .section .text.start, "ax"
    .globl _start
_start:
    li    t0, 0x10000000        # UART THR
    li    t1, 79                # 'O'
    sb    t1, 0(t0)
    li    t1, 75                # 'K'
    sb    t1, 0(t0)
    li    t1, 10                # '\n'
    sb    t1, 0(t0)
    .insn i 0x0b, 1, x0, zero, 0    # xcal.halt zero
