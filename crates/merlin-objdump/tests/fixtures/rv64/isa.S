# merlin-objdump fixture, RV64: the encodings that differ from RV32 or
# exist only on RV64 (64-bit loads and stores, the W operations, 6-bit
# shift amounts, the .d atomics, RV32-only CSRs by number), the aliases
# llvm-objdump picks for them (sext.w, negw), reserved words next to them,
# data in code, and auipc+jalr pairs whose 64-bit targets the disassembler
# resolves (or must not).
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -c -mno-relax rv64/isa.S -o rv64/isa.o

    .text
    .globl isa_start
isa_start:
    lui a0, 0x80000
    lui t6, 0xfffff
    auipc a1, 0xfffff
    auipc s0, 0x80000
    ld a0, -8(sp)
    ld t6, 2040(s0)
    sd a1, 0(sp)
    sd ra, -2048(s1)
    lwu a2, 4(a0)
    lw a3, 0(a0)
    li a0, -1
    addiw a0, a1, -1
    addiw a0, a1, 0
    addiw zero, a1, 0
    slli a0, a1, 63
    srli a0, a1, 32
    srai a0, a1, 63
    slli a0, a1, 31
    slliw a0, a1, 31
    srliw a0, a1, 0
    sraiw a0, a1, 31
    addw a0, a1, a2
    subw a0, a1, a2
    subw a0, zero, a2
    sllw a0, a1, a2
    srlw a0, a1, a2
    sraw a0, a1, a2
    mul a0, a1, a2
    mulh a0, a1, a2
    mulhsu a0, a1, a2
    mulhu a0, a1, a2
    div a0, a1, a2
    divu a0, a1, a2
    rem a0, a1, a2
    remu a0, a1, a2
    mulw a0, a1, a2
    divw a0, a1, a2
    divuw a0, a1, a2
    remw a0, a1, a2
    remuw a0, a1, a2
    # The counters' high halves and the other RV32-only CSRs have no name on
    # RV64; the rest do.
    rdcycle a0
    csrr a0, 0xc80
    csrr a0, 0x310
    csrr a0, satp
    csrr a0, mstatus
    csrw mtvec, t0
    sfence.vma a0, a1
    # Reserved on RV64: slliw/srliw with shamt[5] set, srai with bit 26
    # (a 7-bit shamt), ld with funct3 7, the W ops with M's funct7 bit
    # and another funct3, and an AMO with funct3 4.
    .insn 4, 0x0215951b
    .insn 4, 0x0205d51b
    .insn 4, 0x4405d513
    .insn 4, 0x0005f503
    .insn 4, 0x02c5953b
    .insn 4, 0x0005c52f
    .insn i 0x0b, 0, x0, a0, 0
    .insn i 0x0b, 1, x0, zero, 0
    # Data words ($d): .word, never .dword.
    .word 0xffffffff
    .dword 0x1122334455667788
atomics:
    lr.d a0, (a1)
    lr.d.aq t0, (s1)
    lr.d.rl zero, (t6)
    lr.d.aqrl a5, (sp)
    sc.d a0, a1, (a2)
    sc.d.aqrl a5, zero, (a7)
    amoswap.d a0, a1, (a2)
    amoadd.d.aq a3, a4, (a5)
    amoxor.d.rl s6, s7, (s8)
    amoand.d.aqrl a6, a7, (ra)
    amoor.d zero, a2, (tp)
    amomin.d a5, s2, (t2)
    amomax.d a5, s2, (t2)
    amominu.d a5, s2, (t2)
    amomaxu.d t3, tp, (a7)
    lr.w a0, (a1)
    amoadd.w a0, a1, (a2)
    # lr.d with rs2 != 0 is reserved.
    .insn 4, 0x1015b52f
calls:
    # 64-bit targets: auipc's immediate is sign-extended, so an auipc
    # with bit 31 set reaches below the section, which wraps at 2^64.
    auipc t0, 0
    jalr ra, 8(t0)
    auipc t1, 0x80000
    jalr ra, 0(t1)
    auipc t1, 0xfffff
    jalr zero, -4(t1)
    lui a0, 0x80000
    jalr ra, 0x10(a0)
    auipc a1, 0
    addi a1, a1, 4
    jalr ra, 0(a1)
    auipc a2, 0
    ld a2, 0(a2)
    jalr ra, 0(a2)
    auipc a3, 0
    addiw a3, a3, 0
    jalr ra, 0(a3)
    auipc a3, 0
    sd a3, 0(a3)
    lr.d a4, (a3)
    amoadd.d a4, a3, (a3)
    jalr ra, 0(a3)
    auipc a3, 0
    .word 0
    jalr ra, 0(a3)
    j isa_start
    jal ra, calls
    beqz a0, isa_start
