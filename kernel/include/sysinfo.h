/* Xcal sysinfo (spec section 9.5): read-only 32-bit registers describing the
 * machine, so the kernel discovers its clock and RAM instead of hard-coding
 * them. Only the registers the kernel reads so far are listed. */
#ifndef CAMELOT_SYSINFO_H
#define CAMELOT_SYSINFO_H

#define SYSINFO_BASE 0x30002000
#define SYS_MAGIC (SYSINFO_BASE + 0x00)
#define SYS_TIMEBASE_HZ (SYSINFO_BASE + 0x10)

/* What SYS_MAGIC reads ("SYSI") when the block is present. */
#define SYSINFO_MAGIC 0x53595349

#endif
