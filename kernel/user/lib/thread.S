# thread_start: a thread that runs a C function (lib/README.md, "Threads").
#
# The thread_create system call starts a thread at an address with one
# argument in a0 and a stack pointer, every other register zero: ra
# included, so the code it starts must never return. thread_start gives it
# thread_entry to start at instead, with the function and its argument on
# the new stack, where thread_entry finds them, calls the function, and
# ends the thread with what it returns.
#
# The two slots are pointers, a register wide, stored and loaded with
# REG_S and REG_L (asm.h, Linux's names): sd and ld.

#include <asm.h>

    .text

# int thread_start(int (*fn)(void *), void *arg, void *stack, uint size):
# runs fn(arg) in a new thread, on the `size` bytes at `stack`. Returns the
# new thread's tid, or thread_create's error.
    .globl thread_start
thread_start:
    add   a2, a2, a3            # the top of the stack,
    andi  a2, a2, -16           # 16-byte aligned, as the ABI wants sp
    addi  a2, a2, -16           # and a slot for fn and arg on it
    REG_S a0, 0(a2)
    REG_S a1, SZREG(a2)
    la    a0, thread_entry
    mv    a1, a2                # (the argument is unused)
    tail  thread_create         # its result is ours

# The new thread: sp points at fn and arg. fn's frame goes below them.
thread_entry:
    REG_L t0, 0(sp)
    REG_L a0, SZREG(sp)
    jalr  t0
    call  thread_exit           # fn's return value is already in a0
1:  j     1b                    # thread_exit never returns
