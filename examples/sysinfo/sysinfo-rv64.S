# examples/sysinfo for RV64 (sysinfo.S is the RV32 one): prints what the Xcal
# sysinfo block (spec section 9.5) says about the machine: RAM, clock, timebase
# and the device table, as version 2 of the block lays them out on RV64
# machines: SYS_RAM_BASE and SYS_RAM_SIZE are 64-bit registers (ld), the
# others 32-bit ones read with lwu (they are unsigned), and the device table
# has 24-byte entries { u32 type, u32 irq, u64 base, u64 size }. Addresses and
# sizes print as 16 hex digits. Try different hardware:
#
#   grail examples/sysinfo/sysinfo-rv64.elf
#   grail --ram 16G --clock 2MHz examples/sysinfo/sysinfo-rv64.elf
#   grail --disk examples/disk/disk.img examples/sysinfo/sysinfo-rv64.elf
#
# Halts with exit code 0, or 1 if there is no sysinfo block (bad magic).
#
# Built with our own tools by `cargo xtask examples sysinfo`, which does the same as
#
#   merlin-as -march=rv64ima_zicsr_zifencei examples/sysinfo/sysinfo-rv64.S -o sysinfo-rv64.o
#   merlin-ld -T examples/sysinfo/link.ld sysinfo-rv64.o -o examples/sysinfo/sysinfo-rv64.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/sysinfo/link.ld examples/sysinfo/sysinfo-rv64.S -o sysinfo-rv64.elf

    .equ UART_BASE,       0x10000000
    .equ UART_LSR,        5
    .equ LSR_THRE,        0x20

    .equ SYSINFO_BASE,    0x30002000
    .equ SYS_MAGIC,       0x00
    .equ SYS_VERSION,     0x04
    .equ SYS_RAM_BASE,    0x08          # 64-bit
    .equ SYS_RAM_SIZE,    0x10          # 64-bit
    .equ SYS_TIMEBASE_HZ, 0x18
    .equ SYS_CPU_HZ,      0x1c
    .equ SYS_FLAGS,       0x20
    .equ SYS_DEV_COUNT,   0x24
    .equ SYS_DEV_TABLE,   0x40
    .equ DEV_ENTRY_SIZE,  24            # type, irq, base (64-bit), size (64-bit)
    .equ DEV_TYPE,        0x00
    .equ DEV_IRQ,         0x04
    .equ DEV_BASE,        0x08
    .equ DEV_SIZE,        0x10
    .equ SYSINFO_MAGIC,   0x53595349
    .equ FLAG_REAL_TIME,  1
    .equ DEV_TYPES,       11            # highest type code with a name below

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    li    s0, SYSINFO_BASE
    lwu   t0, SYS_MAGIC(s0)
    li    t1, SYSINFO_MAGIC
    bne   t0, t1, no_sysinfo

    la    a0, s_title               # "sysinfo v<version>"
    call  puts
    lwu   a0, SYS_VERSION(s0)
    call  put_dec
    call  newline

    la    a0, s_ram                 # "RAM      <MiB> MiB at <base>"
    call  puts
    ld    a0, SYS_RAM_SIZE(s0)
    srli  a0, a0, 20
    call  put_dec
    la    a0, s_mib_at
    call  puts
    ld    a0, SYS_RAM_BASE(s0)
    call  put_hex
    call  newline

    la    a0, s_cpu                 # "CPU      <hz> Hz (nominal | real time)"
    call  puts
    lwu   a0, SYS_CPU_HZ(s0)
    call  put_dec
    lwu   t0, SYS_FLAGS(s0)
    andi  t0, t0, FLAG_REAL_TIME
    la    a0, s_nominal
    beqz  t0, 1f
    la    a0, s_real_time
1:  call  puts

    la    a0, s_timebase            # "timebase <hz> Hz"
    call  puts
    lwu   a0, SYS_TIMEBASE_HZ(s0)
    call  put_dec
    la    a0, s_hz
    call  puts
    call  newline

    lwu   s1, SYS_DEV_COUNT(s0)     # "<n> devices:", then one line each
    mv    a0, s1
    call  put_dec
    la    a0, s_devices
    call  puts
    addi  s2, s0, SYS_DEV_TABLE
2:  beqz  s1, 4f
    lwu   a0, DEV_TYPE(s2)          # type -> padded name
    li    t0, DEV_TYPES
    bgtu  a0, t0, 3f
    slli  a0, a0, 3
    la    t0, dev_names
    add   t0, t0, a0
    ld    a0, 0(t0)
    j     5f
3:  la    a0, s_unknown
5:  call  puts
    ld    a0, DEV_BASE(s2)
    call  put_hex
    la    a0, s_plus
    call  puts
    ld    a0, DEV_SIZE(s2)
    call  put_hex
    lwu   s3, DEV_IRQ(s2)           # irq, if it has one
    beqz  s3, 6f
    la    a0, s_irq
    call  puts
    mv    a0, s3
    call  put_dec
6:  call  newline
    addi  s2, s2, DEV_ENTRY_SIZE
    addi  s1, s1, -1
    j     2b

    # Stop the machine with exit code 0.
4:  xcal.halt zero

no_sysinfo:
    la    a0, s_no_sysinfo
    call  puts
    li    a0, 1
    xcal.halt a0

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
    sd    ra, 8(sp)
    mv    a1, a0
1:  lbu   a0, 0(a1)
    beqz  a0, 2f
    call  putc
    addi  a1, a1, 1
    j     1b
2:  ld    ra, 8(sp)
    addi  sp, sp, 16
    ret

# put_dec(a0 = unsigned value): decimal, no padding, up to the 20 digits of
# 2^64 - 1.
put_dec:
    addi  sp, sp, -32
    sd    ra, 24(sp)
    addi  a1, sp, 20                # digits go backwards into sp+0 .. sp+19
    li    a2, 10
1:  remu  a3, a0, a2
    divu  a0, a0, a2
    addi  a3, a3, '0'
    addi  a1, a1, -1
    sb    a3, 0(a1)
    bnez  a0, 1b
    addi  a2, sp, 20
2:  lbu   a0, 0(a1)
    call  putc
    addi  a1, a1, 1
    bne   a1, a2, 2b
    ld    ra, 24(sp)
    addi  sp, sp, 32
    ret

# put_hex(a0 = value): "0x" and 16 lowercase hex digits.
put_hex:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    mv    a1, a0
    li    a0, '0'
    call  putc
    li    a0, 'x'
    call  putc
    li    a2, 60                    # shift of the next digit
    li    a3, 10
1:  srl   a0, a1, a2
    andi  a0, a0, 15
    blt   a0, a3, 2f
    addi  a0, a0, 'a' - 10 - '0'
2:  addi  a0, a0, '0'
    call  putc
    addi  a2, a2, -4
    bgez  a2, 1b
    ld    ra, 8(sp)
    addi  sp, sp, 16
    ret

    .section .rodata
    .balign 8
# Device names by type code (spec section 9.5), padded to one column: 64-bit
# pointers.
dev_names:
    .dword s_unknown, s_clint, s_plic, s_uart, s_test, s_fb_ctrl, s_fb_vram, s_sysinfo
    .dword s_keyboard, s_block, s_pointer, s_net
s_unknown:      .asciz "  unknown   "
s_clint:        .asciz "  CLINT     "
s_plic:         .asciz "  PLIC      "
s_uart:         .asciz "  UART      "
s_test:         .asciz "  test      "
s_fb_ctrl:      .asciz "  fb-ctrl   "
s_fb_vram:      .asciz "  fb-vram   "
s_sysinfo:      .asciz "  sysinfo   "
s_keyboard:     .asciz "  keyboard  "
s_block:        .asciz "  block     "
s_pointer:      .asciz "  pointer   "
s_net:          .asciz "  net       "
s_title:        .asciz "sysinfo v"
s_ram:          .asciz "RAM      "
s_mib_at:       .asciz " MiB at "
s_cpu:          .asciz "CPU      "
s_nominal:      .asciz " Hz (nominal)\n"
s_real_time:    .asciz " Hz (real time)\n"
s_timebase:     .asciz "timebase "
s_hz:           .asciz " Hz"
s_devices:      .asciz " devices:\n"
s_plus:         .asciz " + "
s_irq:          .asciz "  irq "
s_no_sysinfo:   .asciz "no sysinfo block\n"
