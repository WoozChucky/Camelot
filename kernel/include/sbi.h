/* The RISC-V Supervisor Binary Interface (SBI), version 2.0: how the S-mode
 * kernel asks M-mode for what only M-mode can do (spec section 10, "Kernel
 * traps"). The kernel makes every such request through these calls, as
 * Linux does (arch/riscv/kernel/sbi.c), so that the same S-mode kernel runs
 * on Camelot's own SBI on Grail (machine.S) and under OpenSBI on a board.
 *
 * The calling convention: `ecall` from S-mode with the extension ID (EID) in
 * a7, the function ID (FID) in a6 and the arguments in a0-a5. The call
 * returns an error code in a0 (SBI_SUCCESS or a negative SBI_ERR_*) and a
 * value in a1, struct sbiret, and leaves every other register as it was.
 *
 * Assembly includes this file too (machine.S, entry.S), for the numbers:
 * the C is under #ifndef __ASSEMBLER__ (const.h). */
#ifndef CAMELOT_SBI_H
#define CAMELOT_SBI_H

#ifndef __ASSEMBLER__
#include "types.h"
#endif

/* The version this kernel and machine.S implement: the major number in bits
 * 30:24, the minor one in bits 23:0. */
#define SBI_SPEC_VERSION 0x02000000
#define SBI_SPEC_VERSION_MAJOR_SHIFT 24
#define SBI_SPEC_VERSION_MAJOR_MASK 0x7f
#define SBI_SPEC_VERSION_MINOR_MASK 0xffffff

/* Extension IDs. Most spell their name in ASCII: 0x54494D45 is "TIME". */
#define SBI_EXT_BASE 0x10
#define SBI_EXT_TIME 0x54494D45
#define SBI_EXT_IPI 0x735049 /* "sPI": S-mode IPI */
#define SBI_EXT_HSM 0x48534D
#define SBI_EXT_SRST 0x53525354
#define SBI_EXT_DBCN 0x4442434E

/* BASE functions: every SBI implementation has all of them. */
#define SBI_BASE_GET_SPEC_VERSION 0
#define SBI_BASE_GET_IMPL_ID 1
#define SBI_BASE_GET_IMPL_VERSION 2
#define SBI_BASE_PROBE_EXT 3
#define SBI_BASE_GET_MVENDORID 4
#define SBI_BASE_GET_MARCHID 5
#define SBI_BASE_GET_MIMPID 6

/* TIME: set_timer(stime_value), an absolute `time` value (on RV32 its low
 * half in a0, its high half in a1). It also clears the pending supervisor
 * timer interrupt. */
#define SBI_TIME_SET_TIMER 0

/* IPI: send_ipi(hart_mask, hart_mask_base), a supervisor software interrupt
 * to hart hart_mask_base + i for every bit i set in hart_mask; a base of -1
 * means every hart that can take one (STARTED or SUSPENDED, as OpenSBI
 * counts them), and a mask naming any other is SBI_ERR_INVALID_PARAM. */
#define SBI_IPI_SEND_IPI 0

/* HSM, hart state management. */
#define SBI_HSM_HART_START 0      /* (hartid, start_addr, opaque) */
#define SBI_HSM_HART_STOP 1       /* () the calling hart; does not return */
#define SBI_HSM_HART_GET_STATUS 2 /* (hartid): its state in a1 */
#define SBI_HSM_HART_SUSPEND 3    /* (suspend_type, resume_addr, opaque) */

/* The states hart_get_status reports. */
#define SBI_HSM_STATE_STARTED 0
#define SBI_HSM_STATE_STOPPED 1
#define SBI_HSM_STATE_START_PENDING 2
#define SBI_HSM_STATE_STOP_PENDING 3
#define SBI_HSM_STATE_SUSPENDED 4
#define SBI_HSM_STATE_SUSPEND_PENDING 5
#define SBI_HSM_STATE_RESUME_PENDING 6

/* hart_suspend's types: the default retentive one (the hart keeps every
 * register; the call returns once an interrupt is pending) and the default
 * non-retentive one (the hart resumes at resume_addr, as from hart_start). */
#define SBI_HSM_SUSPEND_RET_DEFAULT 0x00000000
#define SBI_HSM_SUSPEND_NON_RET_DEFAULT 0x80000000

/* SRST, system reset: system_reset(reset_type, reset_reason). */
#define SBI_SRST_RESET 0
#define SBI_SRST_TYPE_SHUTDOWN 0
#define SBI_SRST_TYPE_COLD_REBOOT 1
#define SBI_SRST_TYPE_WARM_REBOOT 2
#define SBI_SRST_REASON_NONE 0
#define SBI_SRST_REASON_SYSFAIL 1
/* 0xE000_0000 to 0xEFFF_FFFF: reasons each SBI implementation defines.
 * Camelot's (machine.S) makes reason 0xE000_0000 | code exit code `code` of
 * the machine. */
#define SBI_SRST_REASON_SBI_START 0xE0000000
#define SBI_SRST_REASON_SBI_END 0xEFFFFFFF

/* DBCN, the debug console. */
#define SBI_DBCN_CONSOLE_WRITE 0      /* (num_bytes, base_addr_lo, base_addr_hi) */
#define SBI_DBCN_CONSOLE_READ 1       /* (num_bytes, base_addr_lo, base_addr_hi) */
#define SBI_DBCN_CONSOLE_WRITE_BYTE 2 /* (byte) */

/* Error codes, in a0. */
#define SBI_SUCCESS 0
#define SBI_ERR_FAILED (-1)
#define SBI_ERR_NOT_SUPPORTED (-2)
#define SBI_ERR_INVALID_PARAM (-3)
#define SBI_ERR_DENIED (-4)
#define SBI_ERR_INVALID_ADDRESS (-5)
#define SBI_ERR_ALREADY_AVAILABLE (-6)
#define SBI_ERR_ALREADY_STARTED (-7)
#define SBI_ERR_ALREADY_STOPPED (-8)
#define SBI_ERR_NO_SHMEM (-9)

/* Camelot's SBI implementation (machine.S): its ID, which no registry has
 * given it (the SBI spec lists BBL as 0, OpenSBI as 1, and so on up from
 * there), so a large one well clear of those, "CMLT" in ASCII as the EIDs
 * spell theirs; and its version, whose encoding is the implementation's
 * own: 1, the first. */
#define CAMELOT_SBI_IMPL_ID 0x434D4C54
#define CAMELOT_SBI_IMPL_VERSION 1

#ifndef __ASSEMBLER__
/* What every SBI call returns: a0 and a1, a struct of two registers, which
 * the calling convention returns in those (merlin's too: crates/merlin's
 * abi.rs). */
struct sbiret {
    long error; /* SBI_SUCCESS or SBI_ERR_* */
    long value;
};

/* The call itself (sbi_ecall.S), Linux's sbi_ecall(): ecall with `ext` in
 * a7, `fid` in a6 and arg0-arg5 in a0-a5. */
struct sbiret sbi_ecall(int ext, int fid, uintptr arg0, uintptr arg1, uintptr arg2, uintptr arg3,
                        uintptr arg4, uintptr arg5);

/* Hart 0, at boot: checks that the SBI is version 0.2 or later (the calling
 * convention above) and has every extension the kernel uses, TIME, IPI,
 * HSM and SRST, and panics with the one missing if not; then prints what
 * it found: `sbi: v2.0, implementation 0x434d4c54 version 0x1` and the
 * extensions. */
void sbi_init(void);

/* TIME: the next supervisor timer interrupt of this hart at `time` value
 * `stime`, and the pending one cleared. */
void sbi_set_timer(u64 stime);

/* IPI: a supervisor software interrupt to hart `base` + i for every bit i
 * set in `mask`. Returns the SBI error. */
long sbi_send_ipi(uintptr mask, uintptr base);

/* HSM: starts hart `hartid` in S-mode at physical address `start` with
 * paging off, a0 = its id and a1 = `opaque`. Returns the SBI error:
 * SBI_SUCCESS once the hart is on its way. */
long sbi_hart_start(uintptr hartid, phys_addr_t start, uintptr opaque);

/* HSM: stops the calling hart, which the SBI holds STOPPED until a
 * hart_start; the hart's state in S-mode is lost. Returns only if the stop
 * failed: the SBI error. */
long sbi_hart_stop(void);

/* HSM: hart `hartid`'s state (SBI_HSM_STATE_*), or a negative SBI error. */
long sbi_hart_get_status(uintptr hartid);

/* A short name for what sbi_hart_get_status() returns ("STARTED", or the
 * error's name), for messages. */
const char *sbi_hart_state_name(long state);

/* SRST: system_reset(type, reason). Returns only if the reset failed: the
 * SBI error. */
long sbi_system_reset(u32 type, u32 reason);

/* Stops the machine with exit code `code`, as Linux's machine_power_off():
 * an SRST shutdown whose reason is "no reason" for code 0, "system failure"
 * for PANIC_EXIT_CODE, and Camelot's SBI's own 0xE000_0000 | code for any
 * other code (its low 28 bits). An SBI that refuses that reason (OpenSBI
 * takes only the standard two) gets "system failure" instead, as any code
 * but 0 is one; each refusal is said on the console first (`sbi:
 * system_reset type 0x0 reason 0xe0000007 failed (SBI_ERR_INVALID_PARAM)`),
 * as Linux's sbi_srst_reset() warns. Never returns: if the SBI cannot shut
 * down, the hart waits for good. */
void machine_power_off(u32 code);

/* A short name for an SBI error ("SBI_ERR_INVALID_PARAM"), for messages. */
const char *sbi_strerror(long error);
#endif

#endif
