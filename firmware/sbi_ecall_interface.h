/* The RISC-V Supervisor Binary Interface, version 2.0, as the firmware
 * implements it (sbi.S): the extension and function IDs, the HSM states and
 * the error codes, with OpenSBI's names (include/sbi/sbi_ecall_interface.h).
 * The kernel has its own copy of the numbers (kernel/include/sbi.h), as
 * Linux has (arch/riscv/include/asm/sbi.h): the SBI is the interface
 * between the two, not a header they share. Constants only, for assembly. */
#ifndef CAMELOT_SBI_ECALL_INTERFACE_H
#define CAMELOT_SBI_ECALL_INTERFACE_H

/* The version implemented: the major number in bits 30:24, the minor one in
 * bits 23:0. */
#define SBI_SPEC_VERSION 0x02000000

/* The implementation: "CMLT" in ASCII, which no registry has assigned (the
 * SBI spec numbers BBL 0, OpenSBI 1 and so on up, so a large one well clear
 * of them), and its version. */
#define CAMELOT_SBI_IMPL_ID 0x434D4C54
#define CAMELOT_SBI_IMPL_VERSION 1

/* Extension IDs. */
#define SBI_EXT_BASE 0x10
#define SBI_EXT_TIME 0x54494D45
#define SBI_EXT_IPI 0x735049
#define SBI_EXT_HSM 0x48534D
#define SBI_EXT_SRST 0x53525354
#define SBI_EXT_DBCN 0x4442434E

/* BASE. */
#define SBI_BASE_GET_SPEC_VERSION 0
#define SBI_BASE_GET_IMPL_ID 1
#define SBI_BASE_GET_IMPL_VERSION 2
#define SBI_BASE_PROBE_EXT 3
#define SBI_BASE_GET_MVENDORID 4
#define SBI_BASE_GET_MARCHID 5
#define SBI_BASE_GET_MIMPID 6

/* HSM. */
#define SBI_HSM_HART_START 0
#define SBI_HSM_HART_STOP 1
#define SBI_HSM_HART_GET_STATUS 2
#define SBI_HSM_HART_SUSPEND 3
#define SBI_HSM_STATE_STARTED 0
#define SBI_HSM_STATE_STOPPED 1
#define SBI_HSM_STATE_START_PENDING 2
#define SBI_HSM_STATE_SUSPENDED 4
#define SBI_HSM_SUSPEND_RET_DEFAULT 0x00000000
#define SBI_HSM_SUSPEND_NON_RET_DEFAULT 0x80000000

/* SRST. */
#define SBI_SRST_RESET 0
#define SBI_SRST_TYPE_SHUTDOWN 0
#define SBI_SRST_TYPE_WARM_REBOOT 2
#define SBI_SRST_REASON_SYSFAIL 1
#define SBI_SRST_REASON_SBI_START 0xE0000000
#define SBI_SRST_REASON_SBI_END 0xEFFFFFFF

/* DBCN. */
#define SBI_DBCN_CONSOLE_WRITE 0
#define SBI_DBCN_CONSOLE_READ 1
#define SBI_DBCN_CONSOLE_WRITE_BYTE 2

/* Errors, in a0. */
#define SBI_SUCCESS 0
#define SBI_ERR_NOT_SUPPORTED (-2)
#define SBI_ERR_INVALID_PARAM (-3)
#define SBI_ERR_INVALID_ADDRESS (-5)
#define SBI_ERR_ALREADY_AVAILABLE (-6)

#endif
