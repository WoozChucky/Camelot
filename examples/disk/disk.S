# Block device demo for Grail (spec section 9.6). Finds the first disk in the sysinfo
# device table, prints its size, reads sector 0 and prints it as text (up to the first
# NUL byte), checks that a read past the end fails with error 2, then tests writing:
# it saves the last sector, overwrites it with a pattern, reads it back, compares, and
# writes the saved contents back, so the image is unchanged afterwards.
#
# Every command waits for the disk's completion interrupt: the disk's PLIC source is
# enabled for context 0 and mie.MEIE is set, but mstatus.MIE stays clear, so wfi wakes
# up when MEIP rises and no trap handler is needed. The handler-less "interrupt" is
# then claimed, acknowledged at the disk (BLK_STATUS.DONE) and completed at the PLIC.
#
#   grail --bios none --disk examples/disk/disk.img examples/disk/disk.elf
#   grail --bios none --disk-ro examples/disk/disk.img examples/disk/disk.elf
#
# Exit code 0 on success, 1 without a disk, 2 on a disk error, 3 if the data read back
# differs or the out-of-range read did not fail, 4 on an unexpected PLIC source.
#
# disk.img is 8 sectors (4 KiB) of zeros, with a line of text at the start of sector 0
# and of sector 7. crates/grail/tests/block.rs builds the same bytes and checks the
# committed file against them (set GRAIL_WRITE_DISK_IMG=1 to rewrite it from there).
#
# The device table and the disk have the registers of version 2 of their interfaces
# (spec sections 9.5 and 9.6): 24-byte table entries with a 64-bit base, and a disk
# whose BLK_CAPACITY, BLK_SECTOR and BLK_DMA are 64-bit (REG_L and REG_S: ld and sd).
# Its address prints as 16 hex digits.
#
# Built with our own tools by `cargo xtask examples disk`, which does the same as
#
#   merlin -E -I include examples/disk/disk.S -o disk.s
#   merlin-as disk.s -o disk.o
#   merlin-ld -T examples/disk/link.ld disk.o -o examples/disk/disk.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/disk/link.ld examples/disk/disk.S -o disk.elf

#include <asm.h>

#define DEC_END 20      /* put_dec's digits: sp+0 .. sp+19, the 20 of 2^64 - 1 */

    .equ UART_BASE,       0x10000000
    .equ UART_LSR,        5
    .equ LSR_THRE,        0x20

    .equ SYSINFO_BASE,    0x30002000
    .equ SYS_DEV_COUNT,   0x24
    .equ SYS_DEV_TABLE,   0x40
    .equ DEV_ENTRY_SIZE,  24            # type, irq, base (64-bit), size (64-bit)
    .equ DEV_TYPE,        0x00
    .equ DEV_IRQ,         0x04
    .equ DEV_BASE,        0x08
    .equ DEV_BLOCK,       9             # device type code of a disk

    .equ BLK_CAPACITY,    0x00          # size in sectors (64-bit)
    .equ BLK_SECTOR,      0x08          # 64-bit
    .equ BLK_DMA,         0x10          # buffer address in RAM (64-bit)
    .equ BLK_COUNT,       0x18          # sectors
    .equ BLK_FLAGS,       0x1c          # bit 0: read-only
    .equ BLK_CMD,         0x20
    .equ BLK_STATUS,      0x24          # bit 1: done (write 1 to clear)
    .equ BLK_ERROR,       0x28
    .equ BLK_CTRL,        0x2c          # bit 0: completion interrupt enable
    .equ CMD_READ,        1
    .equ CMD_WRITE,       2
    .equ CMD_FLUSH,       3
    .equ STATUS_DONE,     2
    .equ FLAG_READ_ONLY,  1
    .equ ERR_OUT_OF_RANGE, 2
    .equ SECTOR_SIZE,     512

    .equ PLIC_BASE,       0x0c000000    # + 4 * source: priority
    .equ PLIC_ENABLE0,    0x0c002000    # context 0 = hart 0 M-mode
    .equ PLIC_CLAIM0,     0x0c200004
    .equ MIE_MEIE,        0x800         # mie bit 11
    .equ MIP_MEIP,        0x800         # mip bit 11

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top

    # Find the first disk: s1 = its registers, s2 = its PLIC source, s3 = its size.
    li    t0, SYSINFO_BASE
    lwu   t1, SYS_DEV_COUNT(t0)
    addi  t0, t0, SYS_DEV_TABLE
    li    t2, DEV_BLOCK
1:  beqz  t1, no_disk
    lwu   t3, DEV_TYPE(t0)
    beq   t3, t2, 2f
    addi  t0, t0, DEV_ENTRY_SIZE
    addi  t1, t1, -1
    j     1b
2:  ld    s1, DEV_BASE(t0)
    lwu   s2, DEV_IRQ(t0)
    REG_L s3, BLK_CAPACITY(s1)

    la    a0, s_disk                # "disk 0x10001000, irq 3: 8 sectors[ (read-only)]"
    call  puts
    mv    a0, s1
    call  put_hex
    la    a0, s_irq
    call  puts
    mv    a0, s2
    call  put_dec
    la    a0, s_colon
    call  puts
    mv    a0, s3
    call  put_dec
    la    a0, s_sectors
    call  puts
    lw    t0, BLK_FLAGS(s1)
    andi  t0, t0, FLAG_READ_ONLY
    beqz  t0, 3f
    la    a0, s_read_only
    call  puts
3:  call  newline

    # Completion interrupts: priority 1 for the disk's source, enabled for context 0,
    # mie.MEIE on (mstatus.MIE stays off), and BLK_CTRL.0 on.
    li    t0, PLIC_BASE
    slli  t1, s2, 2
    add   t0, t0, t1
    li    t1, 1
    sw    t1, 0(t0)
    li    t0, PLIC_ENABLE0
    sll   t1, t1, s2
    sw    t1, 0(t0)
    li    t0, MIE_MEIE
    csrs  mie, t0
    li    t0, 1
    sw    t0, BLK_CTRL(s1)

    # Sector 0, as text.
    li    a0, CMD_READ
    li    a1, 0
    la    a2, buf
    call  disk_io
    bnez  a0, disk_error
    la    a0, s_sector0
    call  puts
    la    a0, buf
    li    a1, SECTOR_SIZE
    call  put_text

    # The sector just past the end does not exist: error 2, and the device moves no data.
    li    a0, CMD_READ
    mv    a1, s3
    la    a2, buf
    call  disk_io
    mv    s4, a0
    la    a0, s_out_of_range
    call  puts
    mv    a0, s4
    call  put_dec
    call  newline
    li    t0, ERR_OUT_OF_RANGE
    bne   s4, t0, failed

    lw    t0, BLK_FLAGS(s1)
    andi  t0, t0, FLAG_READ_ONLY
    bnez  t0, read_only

    # Write test on the last sector (s4): save it, write a pattern, read it back.
    addi  s4, s3, -1
    li    a0, CMD_READ
    mv    a1, s4
    la    a2, saved
    call  disk_io
    bnez  a0, disk_error

    la    t0, buf                   # buf[i] = i * 7 + 3
    li    t1, 0
    li    t2, SECTOR_SIZE
4:  slli  t3, t1, 3
    sub   t3, t3, t1
    addi  t3, t3, 3
    add   t4, t0, t1
    sb    t3, 0(t4)
    addi  t1, t1, 1
    bne   t1, t2, 4b

    li    a0, CMD_WRITE
    mv    a1, s4
    la    a2, buf
    call  disk_io
    bnez  a0, disk_error
    li    a0, CMD_READ
    mv    a1, s4
    la    a2, back
    call  disk_io
    bnez  a0, disk_error

    la    t0, buf                   # s5 = 0 if back == buf
    la    t1, back
    li    t2, SECTOR_SIZE
    li    s5, 0
5:  lbu   t3, 0(t0)
    lbu   t4, 0(t1)
    beq   t3, t4, 6f
    li    s5, 1
6:  addi  t0, t0, 1
    addi  t1, t1, 1
    addi  t2, t2, -1
    bnez  t2, 5b

    # Put the saved sector back, and make it durable.
    li    a0, CMD_WRITE
    mv    a1, s4
    la    a2, saved
    call  disk_io
    bnez  a0, disk_error
    li    a0, CMD_FLUSH
    call  disk_io
    bnez  a0, disk_error

    la    a0, s_write_test          # "write/read-back of sector 7: ok"
    call  puts
    mv    a0, s4
    call  put_dec
    la    a0, s_ok
    beqz  s5, 7f
    la    a0, s_mismatch
7:  call  puts
    bnez  s5, failed
    li    a0, 0
    j     halt

read_only:
    la    a0, s_skipped
    call  puts
    li    a0, 0
    j     halt

no_disk:
    la    a0, s_no_disk
    call  puts
    li    a0, 1
    j     halt

disk_error:                         # a0 = BLK_ERROR
    mv    s4, a0
    la    a0, s_error
    call  puts
    mv    a0, s4
    call  put_dec
    call  newline
    li    a0, 2
    j     halt

failed:
    li    a0, 3

    # Stop the machine with exit code a0.
halt:
    xcal.halt a0

# disk_io(a0 = command, a1 = sector, a2 = buffer): runs the command on one sector and
# waits for its completion interrupt. Returns BLK_ERROR in a0. Clobbers t0-t2.
disk_io:
    REG_S a1, BLK_SECTOR(s1)
    REG_S a2, BLK_DMA(s1)
    li    t0, 1
    sw    t0, BLK_COUNT(s1)
    sw    a0, BLK_CMD(s1)
    li    t1, MIP_MEIP
1:  csrr  t0, mip                   # sleep until the PLIC raises MEIP
    and   t0, t0, t1
    bnez  t0, 2f
    wfi
    j     1b
2:  li    t0, PLIC_CLAIM0
    lw    t1, 0(t0)                 # claim: must be the disk
    bne   t1, s2, wrong_irq
    li    t2, STATUS_DONE
    sw    t2, BLK_STATUS(s1)        # acknowledge: the disk lowers its line
    sw    t1, 0(t0)                 # complete
    lw    a0, BLK_ERROR(s1)
    ret

wrong_irq:
    la    a0, s_wrong_irq
    call  puts
    li    a0, 4
    j     halt

# putc(a0 = byte). Clobbers t0, t1 only.
putc:
    li    t0, UART_BASE
1:  lbu   t1, UART_LSR(t0)          # wait until the transmitter is free
    andi  t1, t1, LSR_THRE
    beqz  t1, 1b
    sb    a0, 0(t0)
    ret

newline:
    li    a0, '\n'
    j     putc

# puts(a0 = NUL-terminated string)
puts:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    mv    a1, a0
1:  lbu   a0, 0(a1)
    beqz  a0, 2f
    call  putc
    addi  a1, a1, 1
    j     1b
2:  REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# put_text(a0 = buffer, a1 = length): the bytes up to the first NUL or the end.
put_text:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    mv    a2, a0
    add   a3, a0, a1
1:  beq   a2, a3, 2f
    lbu   a0, 0(a2)
    beqz  a0, 2f
    call  putc
    addi  a2, a2, 1
    j     1b
2:  REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# put_dec(a0 = unsigned value): decimal, no padding, up to the 20 digits of
# 2^64 - 1.
put_dec:
    addi  sp, sp, -32
    REG_S ra, 32-SZREG(sp)
    addi  a1, sp, DEC_END           # digits go backwards into the bytes below sp+DEC_END
    li    a2, 10
1:  remu  a3, a0, a2
    divu  a0, a0, a2
    addi  a3, a3, '0'
    addi  a1, a1, -1
    sb    a3, 0(a1)
    bnez  a0, 1b
    addi  a2, sp, DEC_END
2:  lbu   a0, 0(a1)
    call  putc
    addi  a1, a1, 1
    bne   a1, a2, 2b
    REG_L ra, 32-SZREG(sp)
    addi  sp, sp, 32
    ret

# put_hex(a0 = value): "0x" and XLEN/4 lowercase hex digits, 16.
put_hex:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    mv    a1, a0
    li    a0, '0'
    call  putc
    li    a0, 'x'
    call  putc
    li    a2, __riscv_xlen - 4      # shift of the next digit
    li    a3, 10
1:  srl   a0, a1, a2
    andi  a0, a0, 15
    blt   a0, a3, 2f
    addi  a0, a0, 'a' - 10 - '0'
2:  addi  a0, a0, '0'
    call  putc
    addi  a2, a2, -4
    bgez  a2, 1b
    REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

    .section .rodata
s_disk:         .asciz "disk "
s_irq:          .asciz ", irq "
s_colon:        .asciz ": "
s_sectors:      .asciz " sectors"
s_read_only:    .asciz " (read-only)"
s_sector0:      .asciz "sector 0:\n"
s_out_of_range: .asciz "read past the end: error "
s_write_test:   .asciz "write/read-back of sector "
s_ok:           .asciz ": ok\n"
s_mismatch:     .asciz ": FAILED, read back different data\n"
s_skipped:      .asciz "read-only disk: write test skipped\n"
s_error:        .asciz "disk error "
s_no_disk:      .asciz "no disk (try grail --bios none --disk examples/disk/disk.img)\n"
s_wrong_irq:    .asciz "unexpected PLIC source\n"

    .section .bss
    .balign 4
buf:    .space SECTOR_SIZE
back:   .space SECTOR_SIZE
saved:  .space SECTOR_SIZE
