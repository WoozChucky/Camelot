# One instruction per line. scripts/gen-encode-golden.sh assembles each line
# with clang and writes the expected words and relocations to encode.golden.

# --- RV64I: upper immediates and jumps
lui a0, 0x12345
lui t0, 0xfffff
auipc sp, 1
jal ra, 2048
jal zero, -4
jalr ra, 0(t0)
jalr zero, -2048(a5)
jalr s1, 2047(s2)

# --- branches (numeric offsets)
beq a0, a1, 8
bne t0, t1, -8
blt s0, s1, 4094
bge a2, a3, -4096
bltu a4, a5, 16
bgeu t3, t4, -16

# --- loads and stores
lb a0, 0(a1)
lh a0, -1(sp)
lw ra, 2047(sp)
lbu t0, -2048(t1)
lhu s2, 12(s3)
sb a0, 0(a1)
sh t5, -2(t6)
sw ra, 2044(sp)

# --- OP-IMM
addi a0, a1, -1
addi sp, sp, -2048
slti t0, t1, 5
sltiu t0, t1, -1
xori a0, a0, 0x7ff
ori a0, a0, -2048
andi a0, a0, 255
slli a0, a0, 31
srli a0, a0, 1
srai a0, a0, 17

# --- OP
add a0, a1, a2
sub a0, a1, a2
sll s0, s1, s2
slt t0, t1, t2
sltu t3, t4, t5
xor a0, a0, a0
srl a1, a2, a3
sra a1, a2, a3
or zero, ra, sp
and x31, x30, x29

# --- M extension
mul a0, a1, a2
mulh a0, a1, a2
mulhsu a0, a1, a2
mulhu a0, a1, a2
div a0, a1, a2
divu a0, a1, a2
rem a0, a1, a2
remu a0, a1, a2

# --- A extension: every mnemonic with every ordering suffix
lr.w a0, (a1)
lr.w.aq t0, (s1)
lr.w.rl s1, (t6)
lr.w.aqrl a5, (sp)
sc.w a0, a1, (a2)
sc.w.aq t0, t1, (t2)
sc.w.rl s1, s2, (s3)
sc.w.aqrl a5, a6, (a7)
amoswap.w a0, a1, (a2)
amoswap.w.aq t3, t4, (t5)
amoswap.w.rl ra, sp, (gp)
amoswap.w.aqrl s10, s11, (tp)
amoadd.w a0, a1, (a2)
amoadd.w.aq a3, a4, (a5)
amoadd.w.rl t0, t1, (t2)
amoadd.w.aqrl s0, s1, (s2)
amoxor.w a0, a1, (a2)
amoxor.w.aq s3, s4, (s5)
amoxor.w.rl s6, s7, (s8)
amoxor.w.aqrl s9, t3, (t6)
amoand.w a0, a1, (a2)
amoand.w.aq a6, a7, (ra)
amoand.w.rl t0, a0, (sp)
amoand.w.aqrl t1, a1, (gp)
amoor.w a0, a1, (a2)
amoor.w.aq t2, a2, (tp)
amoor.w.rl s0, a3, (t0)
amoor.w.aqrl s1, a4, (t1)
amomin.w a0, a1, (a2)
amomin.w.aq a5, s2, (t2)
amomin.w.rl a6, s3, (s0)
amomin.w.aqrl a7, s4, (s1)
amomax.w a0, a1, (a2)
amomax.w.aq s5, t3, (a0)
amomax.w.rl s6, t4, (a1)
amomax.w.aqrl s7, t5, (a2)
amominu.w a0, a1, (a2)
amominu.w.aq s8, t6, (a3)
amominu.w.rl s9, ra, (a4)
amominu.w.aqrl s10, sp, (a5)
amomaxu.w a0, a1, (a2)
amomaxu.w.aq s11, gp, (a6)
amomaxu.w.rl t3, tp, (a7)
amomaxu.w.aqrl t4, t0, (s2)
# rd = zero, x-register names, and the optional 0 offset
lr.w zero, 0(x31)
sc.w x0, x31, 0(a0)
amoadd.w zero, a1, 0(a2)
amoswap.w.aqrl x31, x0, (x0)

# --- system
fence
fence.i
ecall
ebreak
mret
sret
wfi
sfence.vma
sfence.vma a0, a1
csrrw a0, mstatus, a1
csrrs t0, mepc, zero
csrrc a0, 0x7c0, a1
csrrwi zero, mtvec, 5
csrrsi a0, sie, 31
csrrci a0, mie, 1
csrr a0, mcause
csrw mscratch, sp
csrs mie, t0
csrc mstatus, t1
csrwi mstatus, 8
csrsi sstatus, 2
csrci sstatus, 2

# --- pseudo-instructions
nop
li a0, 0
li a0, 2047
li a0, -2048
li a0, 2048
li a0, 0x12345678
li a0, 0x80000000
li a0, -1
li t0, 0x10000000
mv a0, a1
not a0, a1
neg a0, a1
seqz a0, a1
snez a0, a1
sltz a0, a1
sgtz a0, a1
sgt a0, a1, a2
sgtu t0, t1, t2
j 16
jr ra
jalr t0
ret
beqz a0, 8
bnez a0, -8
blez a0, 4
bgez a0, 4
bltz a0, 4
bgtz a0, 4
bgt a0, a1, 8
ble a0, a1, 8
bgtu a0, a1, 8
bleu a0, a1, 8

# --- symbol references (produce relocations)
beq a0, a1, target
bnez a0, target
jal ra, target
jal target
j target
call target
tail target
la a0, target
lla a0, target
lui a0, %hi(target)
addi a0, a0, %lo(target)
lw a0, %lo(target)(a0)
sw a1, %lo(target)(a0)
lui a0, %hi(target+16)
lw a0, target
sw a0, target, t0

# --- RV64I: OP-IMM-32
addiw a0, a1, -1
addiw a0, a1, 2047
addiw sp, sp, -2048
slliw a0, a1, 31
slliw a0, a1, 0
srliw a0, a1, 1
srliw t0, t1, 31
sraiw a0, a1, 17
sraiw a0, a1, 31

# --- RV64I: OP-32
addw a0, a1, a2
subw a0, a1, a2
sllw s0, s1, s2
srlw a1, a2, a3
sraw a1, a2, a3
addw x31, x30, x29

# --- M extension: OP-32
mulw a0, a1, a2
divw a0, a1, a2
divuw a0, a1, a2
remw a0, a1, a2
remuw a0, a1, a2

# --- loads and stores: doubleword and unsigned word
ld a0, -8(sp)
ld ra, 2047(sp)
ld t0, -2048(t1)
lwu a1, 4(a0)
lwu s2, 2047(s3)
sd ra, 2040(sp)
sd s0, -2048(a5)
sd zero, 0(a0)

# --- shift amounts: 6 bits on RV64
slli a0, a0, 32
slli a0, a0, 63
srli a0, a0, 32
srli a0, a0, 63
srai a0, a0, 32
srai a0, a0, 63
srai t0, t1, 33

# --- A extension: the .d forms, every mnemonic with every ordering suffix
lr.d a0, (a1)
lr.d.aq t0, (s1)
lr.d.rl s1, (t6)
lr.d.aqrl a5, (sp)
sc.d a0, a1, (a2)
sc.d.aq t0, t1, (t2)
sc.d.rl s1, s2, (s3)
sc.d.aqrl a5, a6, (a7)
amoswap.d a0, a1, (a2)
amoswap.d.aq t3, t4, (t5)
amoswap.d.rl ra, sp, (gp)
amoswap.d.aqrl s10, s11, (tp)
amoadd.d a0, a1, (a2)
amoadd.d.aq a3, a4, (a5)
amoadd.d.rl t0, t1, (t2)
amoadd.d.aqrl s0, s1, (s2)
amoxor.d a0, a1, (a2)
amoxor.d.aq s3, s4, (s5)
amoxor.d.rl s6, s7, (s8)
amoxor.d.aqrl s9, t3, (t6)
amoand.d a0, a1, (a2)
amoand.d.aq a6, a7, (ra)
amoand.d.rl t0, a0, (sp)
amoand.d.aqrl t1, a1, (gp)
amoor.d a0, a1, (a2)
amoor.d.aq t2, a2, (tp)
amoor.d.rl s0, a3, (t0)
amoor.d.aqrl s1, a4, (t1)
amomin.d a0, a1, (a2)
amomin.d.aq a5, s2, (t2)
amomin.d.rl a6, s3, (s0)
amomin.d.aqrl a7, s4, (s1)
amomax.d a0, a1, (a2)
amomax.d.aq s5, t3, (a0)
amomax.d.rl s6, t4, (a1)
amomax.d.aqrl s7, t5, (a2)
amominu.d a0, a1, (a2)
amominu.d.aq s8, t6, (a3)
amominu.d.rl s9, ra, (a4)
amominu.d.aqrl s10, sp, (a5)
amomaxu.d a0, a1, (a2)
amomaxu.d.aq s11, gp, (a6)
amomaxu.d.rl t3, tp, (a7)
amomaxu.d.aqrl t4, t0, (s2)
# rd = zero, x-register names, and the optional 0 offset
lr.d zero, 0(x31)
sc.d x0, x31, 0(a0)
amoadd.d zero, a1, 0(a2)
amoswap.d.aqrl x31, x0, (x0)

# --- pseudo-instructions
sext.w a0, a1
sext.w t0, t0
negw a0, a1
zext.w a0, a1
zext.w s0, t6

# --- li: LLVM's RISCVMatInt sequences, up to 8 instructions
li a0, 0x7fffffff
li a0, 0x7ffff800
li a0, 0x7ffff7ff
li a0, 0xffffffff
li sp, 0x80400000
li a0, -0x80000000
li a0, -0x80000001
li a0, 0x80000800
li a0, 0xfffff800
li a0, 0xffff0000
li a0, 0xdeadbeef
li a0, 0x80002b04
li a0, 0x100000000
li a0, 0x100000001
li a0, 0x1fffff000
li a0, 0x7fffffffffffffff
li a0, 0x8000000000000000
li a0, 0xfffffffffffff800
li a0, 0x00ffffffffffffff
li a0, 0x8000000000000fff
li a0, 0xffff000000000fff
li a0, 0xffffffff00000000
li a0, 0x7fffffff00000000
li a0, 0x00000ffffffff000
li a0, 0x00000000ffff0fff
li a0, 0xffffffff00000fff
li a0, 0x1000000017ff
li a0, 0x905acf7102b
li a0, 0x12345678abcd
li a0, 0x1234567800000000
li a0, 0x123456789abcdef0
li a0, 0xfedcba9876543210
li a0, 0x5555555555555555
li a0, 0xaaaaaaaaaaaaaaaa
li a0, 0x7fffffff + 1
li a0, 1 << 63

# --- symbol references (produce relocations)
ld a0, target
lwu a0, target
sd a0, target, t0
ld a0, %lo(target)(a0)
lwu a0, %lo(target)(a0)
sd a1, %lo(target)(a0)
addiw a0, a0, %lo(target)
