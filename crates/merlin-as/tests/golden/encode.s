# One instruction per line. scripts/gen-encode-golden.sh assembles each line
# with llvm-mc and writes the expected words and relocations to encode.golden.

# --- RV32I: upper immediates and jumps
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
