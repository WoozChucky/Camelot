/* The kernel's side of the SBI (include/sbi.h): one function per call it
 * makes, over sbi_ecall() (sbi_ecall.S), as Linux's arch/riscv/kernel/sbi.c
 * has them, and the boot-time check that the calls are there. */
#include "sbi.h"
#include "errno.h"
#include "kprintf.h"
#include "riscv.h"
#include "trap.h"

/* The extensions the kernel cannot do without: the timer tick, IPIs,
 * starting the other harts and stopping the machine. */
static const struct {
    long ext;
    const char *name;
} required[] = {
    {SBI_EXT_TIME, "TIME"},
    {SBI_EXT_IPI, "IPI"},
    {SBI_EXT_HSM, "HSM"},
    {SBI_EXT_SRST, "SRST"},
};

static struct sbiret sbi_call0(int ext, int fid) {
    return sbi_ecall(ext, fid, 0, 0, 0, 0, 0, 0);
}

/* BASE probe_extension: nonzero if the SBI has extension `ext`. */
static long sbi_probe_extension(long ext) {
    struct sbiret r = sbi_ecall(SBI_EXT_BASE, SBI_BASE_PROBE_EXT, (uintptr)ext, 0, 0, 0, 0, 0);
    return r.error == SBI_SUCCESS ? r.value : 0;
}

void sbi_init(void) {
    /* get_spec_version is BASE's, which every SBI from v0.2 on has. An
     * older one (v0.1, the "legacy" calls only) answers an unknown
     * extension with SBI_ERR_NOT_SUPPORTED, if it answers at all. */
    struct sbiret v = sbi_call0(SBI_EXT_BASE, SBI_BASE_GET_SPEC_VERSION);
    if (v.error != SBI_SUCCESS) {
        panic("sbi: no SBI v0.2 or later (the base extension is missing)");
    }
    long major = (v.value >> SBI_SPEC_VERSION_MAJOR_SHIFT) & SBI_SPEC_VERSION_MAJOR_MASK;
    long minor = v.value & SBI_SPEC_VERSION_MINOR_MASK;
    struct sbiret id = sbi_call0(SBI_EXT_BASE, SBI_BASE_GET_IMPL_ID);
    struct sbiret version = sbi_call0(SBI_EXT_BASE, SBI_BASE_GET_IMPL_VERSION);
    kprintf("sbi: v%ld.%ld, implementation 0x%lx version 0x%lx\n", major, minor, id.value, version.value);

    for (u32 i = 0; i < sizeof required / sizeof required[0]; i++) {
        if (sbi_probe_extension(required[i].ext) == 0) {
            char msg[64];
            ksnprintf(msg, sizeof msg, "sbi: no %s extension", required[i].name);
            panic(msg);
        }
    }
    /* The debug console is optional: the kernel has its own UART driver. */
    bool dbcn = sbi_probe_extension(SBI_EXT_DBCN) != 0;
    kprintf("sbi: extensions TIME IPI HSM SRST%s\n", dbcn ? " DBCN" : "");
}

void sbi_set_timer(u64 stime) {
    sbi_ecall(SBI_EXT_TIME, SBI_TIME_SET_TIMER, stime, 0, 0, 0, 0, 0);
}

long sbi_send_ipi(uintptr mask, uintptr base) {
    struct sbiret r = sbi_ecall(SBI_EXT_IPI, SBI_IPI_SEND_IPI, mask, base, 0, 0, 0, 0);
    if (r.error != SBI_SUCCESS) {
        /* Linux's __sbi_send_ipi_v02() message, pr_err("%s: hbase = [%lu]
         * hmask = [0x%lx] failed (error [%d])\n", __func__, hbase, hmask,
         * sbi_err_map_linux_errno(ret.error)): the error as minus an errno,
         * -22 (-EINVAL) for SBI_ERR_INVALID_PARAM. */
        kprintf("sbi_send_ipi: hbase = [%lu] hmask = [0x%lx] failed (error [%d])\n", base, mask,
                sbi_err_map_linux_errno(r.error));
    }
    return r.error;
}

long sbi_hart_start(uintptr hartid, phys_addr_t start, uintptr opaque) {
    return sbi_ecall(SBI_EXT_HSM, SBI_HSM_HART_START, hartid, start, opaque, 0, 0, 0).error;
}

long sbi_hart_stop(void) {
    return sbi_ecall(SBI_EXT_HSM, SBI_HSM_HART_STOP, 0, 0, 0, 0, 0, 0).error;
}

long sbi_hart_get_status(uintptr hartid) {
    struct sbiret r = sbi_ecall(SBI_EXT_HSM, SBI_HSM_HART_GET_STATUS, hartid, 0, 0, 0, 0, 0);
    return r.error != SBI_SUCCESS ? r.error : r.value;
}

const char *sbi_hart_state_name(long state) {
    switch (state) {
    case SBI_HSM_STATE_STARTED:
        return "STARTED";
    case SBI_HSM_STATE_STOPPED:
        return "STOPPED";
    case SBI_HSM_STATE_START_PENDING:
        return "START_PENDING";
    case SBI_HSM_STATE_STOP_PENDING:
        return "STOP_PENDING";
    case SBI_HSM_STATE_SUSPENDED:
        return "SUSPENDED";
    case SBI_HSM_STATE_SUSPEND_PENDING:
        return "SUSPEND_PENDING";
    case SBI_HSM_STATE_RESUME_PENDING:
        return "RESUME_PENDING";
    }
    /* A negative one is the call's error. */
    return state < 0 ? sbi_strerror(state) : "an unknown state";
}

long sbi_system_reset(u32 type, u32 reason) {
    return sbi_ecall(SBI_EXT_SRST, SBI_SRST_RESET, type, reason, 0, 0, 0, 0).error;
}

/* A reset that, if it returns at all, failed: said so in the words of
 * Linux's sbi_srst_reset(), pr_warn("%s: type=0x%lx reason=0x%lx failed\n",
 * __func__, type, reason). */
static void sbi_srst_reset(u32 type, u32 reason) {
    sbi_system_reset(type, reason);
    kprintf("sbi_srst_reset: type=0x%x reason=0x%x failed\n", type, reason);
}

void machine_power_off(u32 code) {
    u32 reason;
    if (code == 0) {
        reason = SBI_SRST_REASON_NONE;
    } else if (code == PANIC_EXIT_CODE) {
        reason = SBI_SRST_REASON_SYSFAIL;
    } else {
        /* The code's low 28 bits, what the range has room for. Grail's exit
         * code is the low 8 bits of what xcal.halt is given anyway (spec
         * section 9.2), so a status of -1 still ends the machine with
         * 0xff. */
        reason = SBI_SRST_REASON_SBI_START | (code & (SBI_SRST_REASON_SBI_END - SBI_SRST_REASON_SBI_START));
    }
    sbi_srst_reset(SBI_SRST_TYPE_SHUTDOWN, reason);
    /* Still here: the SBI refused the reason (OpenSBI takes only "no
     * reason" and "system failure"), or cannot shut down at all. */
    if (reason != SBI_SRST_REASON_NONE && reason != SBI_SRST_REASON_SYSFAIL) {
        sbi_srst_reset(SBI_SRST_TYPE_SHUTDOWN, SBI_SRST_REASON_SYSFAIL);
    }
    intr_off();
    for (;;) {
        wfi();
    }
}

const char *sbi_strerror(long error) {
    switch (error) {
    case SBI_SUCCESS:
        return "SBI_SUCCESS";
    case SBI_ERR_FAILED:
        return "SBI_ERR_FAILED";
    case SBI_ERR_NOT_SUPPORTED:
        return "SBI_ERR_NOT_SUPPORTED";
    case SBI_ERR_INVALID_PARAM:
        return "SBI_ERR_INVALID_PARAM";
    case SBI_ERR_DENIED:
        return "SBI_ERR_DENIED";
    case SBI_ERR_INVALID_ADDRESS:
        return "SBI_ERR_INVALID_ADDRESS";
    case SBI_ERR_ALREADY_AVAILABLE:
        return "SBI_ERR_ALREADY_AVAILABLE";
    case SBI_ERR_ALREADY_STARTED:
        return "SBI_ERR_ALREADY_STARTED";
    case SBI_ERR_ALREADY_STOPPED:
        return "SBI_ERR_ALREADY_STOPPED";
    case SBI_ERR_NO_SHMEM:
        return "SBI_ERR_NO_SHMEM";
    case SBI_ERR_INVALID_STATE:
        return "SBI_ERR_INVALID_STATE";
    case SBI_ERR_BAD_RANGE:
        return "SBI_ERR_BAD_RANGE";
    case SBI_ERR_TIMEOUT:
        return "SBI_ERR_TIMEOUT";
    case SBI_ERR_IO:
        return "SBI_ERR_IO";
    case SBI_ERR_DENIED_LOCKED:
        return "SBI_ERR_DENIED_LOCKED";
    }
    return "an unknown SBI error";
}
