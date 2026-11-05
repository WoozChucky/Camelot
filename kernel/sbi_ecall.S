# struct sbiret sbi_ecall(int ext, int fid, uintptr arg0, ..., uintptr arg5)
# (include/sbi.h): an SBI call, as Linux's sbi_ecall() makes it with inline
# assembly. The C arguments arrive in a0-a7; the SBI wants the extension in
# a7, the function in a6 and the arguments in a0-a5, so they move down two
# registers, and ext and fid go up. The SBI returns struct sbiret in a0 and
# a1, which is where a C function returns a struct of two registers, and
# keeps every other register: nothing to do after the ecall but return.
#
# Every register here is a whole register, and ext and fid, 32-bit ints,
# arrive sign-extended as the SBI wants them.

    .text
    .globl sbi_ecall
sbi_ecall:
    mv    t0, a0                # ext
    mv    t1, a1                # fid
    mv    a0, a2
    mv    a1, a3
    mv    a2, a4
    mv    a3, a5
    mv    a4, a6
    mv    a5, a7
    mv    a7, t0
    mv    a6, t1
    ecall
    ret
