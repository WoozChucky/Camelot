/* Camelot's firmware (spec section 7, "Firmware"): what it knows of itself
 * and of the machine it runs on, Grail's. Constants only, for assembly: the
 * firmware is all assembly (entry.S, sbi.S, disk.S), and so is the RV32
 * kernel's M-mode layer, which is sbi.S itself until #159
 * (kernel/machine.S includes it). <asm.h> gives the width's instructions. */
#ifndef CAMELOT_FW_H
#define CAMELOT_FW_H

/* ---- The firmware's place in RAM: OpenSBI's fw_jump layout on QEMU virt ----
 *
 * The firmware is linked at the start of RAM and has its first 2 MiB, code,
 * data and the harts' scratch blocks; the next stage (the kernel, or a
 * disk's boot block) loads and starts at the 2 MiB after it, OpenSBI's
 * FW_JUMP_ADDR on virt. The device tree Grail makes reserves the 2 MiB
 * (/reserved-memory), and so does sysinfo (SYS_RESERVED_BASE, _SIZE). */
#define FW_BASE 0x80000000
#define FW_SIZE 0x200000
#define FW_JUMP_ADDR 0x80200000

/* ---- What the boot ROM hands over ----
 *
 * Every hart enters the firmware with a0 = its hart id, a1 = the device
 * tree's physical address and a2 = a struct fw_dynamic_info in the ROM, as
 * QEMU's reset vector hands them to OpenSBI (OpenSBI's
 * include/sbi/fw_dynamic.h; the fields are unsigned longs, a register
 * wide). next_addr is the program Grail loaded, or 0 for none, QEMU's value
 * when it has no kernel: then the firmware boots from disk 0 (disk.S). */
#define FW_DYNAMIC_INFO_MAGIC 0x4942534f       /* "OSBI" */
#define FW_DYNAMIC_INFO_VERSION_MIN 1          /* next_addr and next_mode from version 1 on */
#define FW_DYNAMIC_INFO_NEXT_MODE_S 1          /* the next stage runs in S-mode */
#define FW_DINFO_MAGIC (0 * SZREG)
#define FW_DINFO_VERSION (1 * SZREG)
#define FW_DINFO_NEXT_ADDR (2 * SZREG)
#define FW_DINFO_NEXT_MODE (3 * SZREG)
#define FW_DINFO_OPTIONS (4 * SZREG)
#define FW_DINFO_BOOT_HART (5 * SZREG)         /* version 2 */

/* ---- The harts ----
 *
 * The most a machine has (spec section 7, "Harts"): one scratch block each. */
#define FW_MAX_HARTS 8

/* Each hart's scratch block, which its mscratch points at: 16 slots a
 * register wide (XLEN bits). sbi.S uses these slot numbers, times SZREG.
 *   - Slots 0-5 hold t0-t5 while sbi_trap_vec runs (it swaps t6 with
 *     mscratch, so the SBI's arguments in a0-a7 stay where they are).
 *   - The hart's HSM state (SBI_HSM_STATE_*), a u32 in the slot's first 4
 *     bytes, which other harts read (hart_get_status) and change
 *     (hart_start, with lr.w/sc.w).
 *   - A start request for a stopped hart: the address to start at, 0 for
 *     none (RAM, where every start address is, starts at 0x8000_0000), and
 *     the opaque value for a1. hart_start writes the opaque value first and
 *     the address last; the stopped hart waits for the address. */
#define SCRATCH_SAVE 0          /* slots 0-5: t0-t5 */
#define SCRATCH_HSM_STATE 6     /* SBI_HSM_STATE_*, a u32 */
#define SCRATCH_START_ADDR 7    /* the physical address hart_start asked for, or 0 */
#define SCRATCH_START_ARG 8     /* hart_start's opaque, which the hart gets in a1 */
#define SCRATCH_WORDS 16        /* a power of two: a block is at hart << SCRATCH_SHIFT */
#if __riscv_xlen == 64
#define SCRATCH_SHIFT 7         /* log2(16 slots of 8 bytes) */
#else
#define SCRATCH_SHIFT 6         /* log2(16 slots of 4 bytes) */
#endif

/* ---- Grail's devices (spec sections 7 and 9) ---- */

/* The CLINT: hart h's msip, a 32-bit register, and its 64-bit mtimecmp. */
#define CLINT_BASE 0x02000000
#define CLINT_MSIP(h) (CLINT_BASE + 4 * (h))
#define CLINT_MTIMECMP(h) (CLINT_BASE + 0x4000 + 8 * (h))

/* The UART, 16550-compatible. */
#define UART_BASE 0x10000000
#define UART_THR 0
#define UART_RBR 0
#define UART_LSR 5
#define UART_LSR_DR 0x01
#define UART_LSR_THRE 0x20

/* Sysinfo (spec section 9.5): version 2 on RV64, 1 on RV32. */
#define SYSINFO_BASE 0x30002000
#define SYSINFO_MAGIC 0x53595349 /* "SYSI" */
#define SYS_MAGIC 0x00
#if __riscv_xlen == 64
#define SYS_RAM_BASE 0x08       /* 64-bit */
#define SYS_RAM_SIZE 0x10       /* 64-bit */
#define SYS_HARTS 0x28
#define SYS_RESERVED_BASE 0x30  /* 64-bit: this firmware's region, from Grail */
#define SYS_RESERVED_SIZE 0x38  /* 64-bit: 0 without firmware */
#else
#define SYS_RAM_BASE 0x08
#define SYS_RAM_SIZE 0x0c
#define SYS_HARTS 0x20
#endif

/* Disk 0's registers (spec section 9.6): version 2 on RV64, with the 64-bit
 * BLK_CAPACITY, BLK_SECTOR and BLK_DMA. The firmware is RV64 only (RV32
 * machines have none), and only it reads a disk. */
#define DISK0 0x10001000
#define BLK_CAPACITY 0x00       /* 64-bit */
#define BLK_SECTOR 0x08         /* 64-bit */
#define BLK_DMA 0x10            /* 64-bit */
#define BLK_COUNT 0x18
#define BLK_CMD 0x20
#define BLK_STATUS 0x24
#define BLK_ERROR 0x28
#define BLK_CMD_READ 1
#define BLK_STATUS_DONE 2

/* ---- Exit codes (xcal.halt) ---- */
#define EXIT_SYSTEM_FAILURE 0xfe   /* SRST's "system failure": the kernel's panic code */
#define EXIT_UNEXPECTED_TRAP 0xff  /* a trap the firmware does not handle */
#define EXIT_BOOT_FAILED 0xfd      /* no next stage: the boot ROM's code too */

#endif
