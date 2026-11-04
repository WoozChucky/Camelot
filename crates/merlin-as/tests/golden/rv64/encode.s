# The RV64-only lines of the RV64 golden, one instruction per line.
# scripts/gen-encode-golden.sh assembles ../encode.s and then this file with
# clang --target=riscv64 and writes the expected words and relocations to
# encode.golden next to it. These lines are not RV32 instructions, so they
# live here and not in ../encode.s, which crates/xtask/tests/selfhost.rs also
# assembles with Camelot's /bin/as (RV32).

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
