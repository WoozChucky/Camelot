# merlin-objdump fixture: every RV32IMA/Zicsr/Zifencei/privileged encoding,
# the operand forms where llvm-objdump picks a pseudo-instruction (and the
# tie-breaks when both registers are zero), Xcal words, data words in code,
# and auipc+jalr pairs whose targets the disassembler resolves (or must not).
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv32 -march=rv32ima_zicsr_zifencei -mabi=ilp32 -c -mno-relax isa.S -o isa.o

    .text
    .globl isa_start
isa_start:
    lui a0, 0x12345
    lui t6, 0xfffff
    auipc a1, 0x1
    auipc s0, 0
    jal ra, 1f
1:  jal zero, 1b
    jal t0, 1b
    jalr ra, 0(a0)
    jalr zero, 0(ra)
    jalr zero, 0(a5)
    jalr ra, 4(a0)
    jalr zero, -8(a1)
    jalr t1, 0(a2)
    jalr t1, 12(a2)
    beq a0, a1, 2f
    beq a0, zero, 2f
    beq zero, a0, 2f
    bne a0, a1, 2f
    bne a0, zero, 2f
    bne zero, a0, 2f
    blt a0, a1, 2f
    blt a0, zero, 2f
    blt zero, a0, 2f
    bge a0, a1, 2f
    bge a0, zero, 2f
    bge zero, a0, 2f
    bltu a0, a1, 2f
    bltu zero, a0, 2f
    bgeu a0, a1, 2f
    bgeu a0, zero, 2f
2:  beq a0, a1, 1b
    lb a0, 0(a1)
    lh a0, -1(sp)
    lw a0, 2047(sp)
    lbu a0, -2048(s0)
    lhu t6, 16(t5)
    sb a0, 0(a1)
    sh a0, -2(sp)
    sw ra, 2044(sp)
    nop
    addi zero, a0, 1
    addi a0, zero, 5
    addi a0, zero, -2048
    addi a1, a0, 0
    addi a1, a0, -1
    addi sp, sp, 2047
    slti a0, a1, -5
    slti a0, a1, 0
    sltiu a0, a1, 1
    sltiu a0, a1, -1
    xori a0, a1, -1
    xori a0, a1, 0x55
    ori a0, a1, 0
    andi a0, a1, 255
    andi a0, a1, -16
    slli a0, a1, 0
    slli a0, a1, 31
    srli a0, a1, 7
    srai a0, a1, 31
    add a0, a1, a2
    add a0, zero, a2
    sub a0, a1, a2
    sub a0, zero, a2
    sll a0, a1, a2
    slt a0, a1, a2
    slt a0, a1, zero
    slt a0, zero, a1
    sltu a0, a1, a2
    sltu a0, zero, a2
    sltu a0, a2, zero
    xor a0, a1, a2
    srl a0, a1, a2
    sra a0, a1, a2
    or a0, a1, a2
    and a0, a1, a2
    mul a0, a1, a2
    mulh a0, a1, a2
    mulhsu a0, a1, a2
    mulhu a0, a1, a2
    div a0, a1, a2
    divu a0, a1, a2
    rem a0, a1, a2
    remu s11, t6, gp
    fence
    fence rw, rw
    fence r, w
    fence i, o
    fence iorw, r
    fence.tso
    fence.i
    ecall
    ebreak
    mret
    sret
    wfi
    sfence.vma
    sfence.vma a0
    sfence.vma a0, a1
    sfence.vma zero, a1
    csrrw a0, mscratch, a0
    csrw mtvec, t0
    csrr a0, mstatus
    csrrs a0, mie, a1
    csrs mie, a1
    csrrc a0, mip, a1
    csrc sstatus, a1
    csrrwi a0, mstatus, 5
    csrwi mstatus, 31
    csrsi mstatus, 8
    csrci sie, 2
    csrrsi a0, mstatus, 0
    csrrci a0, mstatus, 1
    rdcycle a0
    rdtime a1
    rdinstret a2
    rdcycleh a3
    rdtimeh a4
    rdinstreth a5
    csrr a0, mhartid
    csrr a0, satp
    csrr a0, 0x7c0
    csrr a0, 0x3a1
    csrw 0x800, a0
    csrr t0, mcycle
    unimp
    .insn i 0x0b, 0, x0, a0, 0
    .insn i 0x0b, 1, x0, zero, 0
    # Data words: the assembler marks them with a $d mapping symbol, and
    # llvm-objdump prints them as .word.
    .word 0
    .word 0xffffffff
    .insn 4, 0x0005200b
edge:
    addi a0, zero, 0
    addi zero, zero, 5
    addi zero, a0, 0
    slt a0, zero, zero
    sltu a0, zero, zero
    sub a0, zero, zero
    blt zero, zero, edge
    bge zero, zero, edge
    beq zero, zero, edge
    csrrs zero, cycle, zero
    csrrs zero, mstatus, zero
    csrrw zero, mstatus, zero
    # Hints LLVM 23 names (ntl.p1, prefetch.i, prefetch.w, lpad, pause),
    # next to the reserved fences (fm, rs1, rd), written as instructions.
    add zero, zero, sp
    ori zero, a0, 0x20
    ori zero, a0, 0x23
    auipc zero, 1
    .insn 4, 0x0100000f
    .insn 4, 0x8ff0000f
    .insn 4, 0x0ff5000f
    .insn 4, 0x0ff0008f
    xori a0, a1, 1
    jalr zero, 0x100(zero)
calls:
    auipc t0, 0
    jalr ra, 8(t0)
    auipc t1, 1
    jalr zero, -4(t1)
    lui a0, 0
    jalr ra, 0x10(a0)
    auipc a1, 0
    addi a1, a1, 4
    jalr ra, 0(a1)
    # Data between them: the mapping symbols around it forget everything.
    auipc a2, 0
    .word 0
    jalr ra, 0(a2)
    auipc a3, 0
    lw a4, 0(a3)
    jalr ra, 0(a3)
    auipc a3, 0
    mret
    jalr ra, 0(a3)
    auipc a3, 0
    ecall
    jalr ra, 0(a3)
    auipc a3, 0
    .insn i 0x0b, 0, x0, a0, 0
    jalr ra, 0(a3)
    auipc a3, 0
    bnez a0, 1f
1:  jalr ra, 0(a3)
    auipc a3, 0
second:
    jalr ra, 0(a3)
regs:
    # Writing the tracked register forgets it (lui, csr), as does any jalr
    # and any word llvm does not decode, reserved fences included; a plain
    # fence keeps it.
    auipc a3, 0
    lui a3, 0
    jalr ra, 0(a3)
    auipc a3, 0
    csrr a3, mstatus
    jalr ra, 0(a3)
    auipc a3, 0
    jalr t0
    jalr ra, 0(a3)
    auipc a3, 0
    .insn 4, 0x8ff0000f
    jalr ra, 0(a3)
    auipc a3, 0
    .insn 4, 0x0010100f
    jalr ra, 0(a3)
    auipc a3, 0
    fence
    jalr ra, 0(a3)
atomics:
    # The A extension: every mnemonic, every ordering suffix, rd = zero.
    lr.w a0, (a1)
    lr.w.aq t0, (s1)
    lr.w.rl zero, (t6)
    lr.w.aqrl a5, (sp)
    sc.w a0, a1, (a2)
    sc.w.aq t0, t1, (t2)
    sc.w.rl s1, s2, (s3)
    sc.w.aqrl a5, zero, (a7)
    amoswap.w a0, a1, (a2)
    amoadd.w.aq a3, a4, (a5)
    amoxor.w.rl s6, s7, (s8)
    amoand.w.aqrl a6, a7, (ra)
    amoor.w zero, a2, (tp)
    amomin.w.aq a5, s2, (t2)
    amomax.w.rl s5, t3, (a0)
    amominu.w.aqrl s8, t6, (a3)
    amomaxu.w t3, tp, (a7)
    # Reserved: lr.w with rs2 != 0, and amocas.w (Zacas, not in Excalibur).
    .insn 4, 0x1015a52f
    .insn 4, 0x2805a52f
    # lr/sc/amo write rd, so they forget a tracked rd; reading it as the
    # address keeps it.
    auipc a3, 0
    amoswap.w a3, a1, (a0)
    jalr ra, 0(a3)
    auipc a3, 0
    sc.w a3, a1, (a0)
    jalr ra, 0(a3)
    auipc a3, 0
    lr.w a3, (a0)
    jalr ra, 0(a3)
    auipc a3, 0
    lr.w a4, (a3)
    amoadd.w a4, a3, (a3)
    jalr ra, 0(a3)
data:
    # Data in code ($d): .word while 4 bytes are left before the next
    # label or mapping symbol, then .short, then .byte; a run of zeros is
    # `...` as in code.
    nop
    .word 0, 0, 0, 0
    .word 0x11223344
    .byte 1, 2, 3, 4, 5
data_mid:
    .byte 6, 7, 8
    .half 9
    .byte 10
    .balign 4
    nop
wrap:
    # Targets below 0: the operand wraps at 2^32, but LLVM looks the symbol
    # up for the 64-bit target, as on RV64 (<data_mid+0xffffffff...>).
    j .-0x400
    auipc t1, 0xfffff
    jalr ra, 0(t1)
    bnez a0, .-0x800
