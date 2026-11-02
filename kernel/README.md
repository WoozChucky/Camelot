# Camelot kernel

The Camelot OS kernel, in C with a little assembly. A small M-mode layer sets
up trap delegation and the timer, then the kernel runs in S-mode: it prints a
banner and a `kprintf` self-test over the UART, takes ten timer ticks, and
starts a tiny shell on the interrupt-driven UART console. Faults panic with the
cause, `sepc`, `stval` and a register dump.

```
cargo run --release -p grail -- kernel/kernel.elf
cargo run --release -p grail -- --clock 10MHz kernel/kernel.elf    # ticks in real time
```

```
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=80000054 a0 = 0x00000000 (0)
timer: 10 Hz tick, 10000000 Hz timebase
tick 1
tick 2
...
tick 10
Type help for the list of commands.
camelot> ticks
ticks: 10
camelot> halt
```

The shell knows `help`, `ticks` (timer ticks since boot) and `halt`. Lines
are edited with backspace and end with Enter. In a terminal, Grail passes each
key to the kernel as it is typed and the kernel does the echoing; piped input
works too:

```
printf 'ticks\nhalt\n' | cargo run --release -p grail -- kernel/kernel.elf
```

Without `--clock`, Grail runs at full speed and skips every `wfi` to the next
timer tick, so while the shell waits for a key the tick count races ahead;
with `--clock 10MHz` it counts ten a second.

The exit code is `kmain()`'s return value after `halt` (0 = all self-tests
passed), 0xfe after a kernel panic, or 0xff after an unexpected M-mode trap.
The traps, the M-mode layer and the panic format are described in
`docs/excalibur-spec.md` section 10, "Kernel traps"; the PLIC setup, the UART
driver, the console and the shell in "Kernel console".

A panic looks like this (from the deliberate illegal instruction the tests
trigger by patching `boot_fault` in the image to 2; 3 loads from an unmapped
address instead):

```
panic: illegal instruction
  scause 0x00000002  sepc 0x800002bc  stval 0xc0001073  sstatus 0x00000120
   ra 01010101   sp 80007400   gp 03030303   tp 04040404
   t0 05050505   t1 06060606   t2 07070707   s0 08080808
   ...
   t4 1d1d1d1d   t5 1e1e1e1e   t6 1f1f1f1f
```

## Files

| File | What it does |
|---|---|
| `entry.s` | `_start` (M-mode): early trap vector, stack, zero `.bss`, `call start`, `mret` to S-mode. `supervisor_entry`: `call kmain`, then `machine_halt` with its return value. Also `xcal_dbg`/`xcal_halt` for C and the early trap handler (prints `mcause`, `mepc`, `mtval`, exits 0xff) |
| `start.c` | `start()`: M-mode setup: `medeleg`/`mideleg`, `stvec`, `mtvec`, `mscratch`, the first timer interrupt, `mstatus.MPP = S`, `mepc` |
| `machine.s` | `machine_vec`, the M-mode trap handler: forwards the machine timer to S-mode as SSIP, serves machine calls (`MCALL_HALT`). `machine_halt()` makes that call from S-mode |
| `kernelvec.s` | `kernel_vec`, the S-mode trap vector: saves x1-x31 and the trap CSRs as a `struct trap_frame`, calls `kernel_trap`, restores, `sret`. Also `illegal_instruction()` for the panic tests |
| `trap.c` | `kernel_trap()`: counts timer ticks, claims/completes PLIC interrupts and runs the UART's handler, panics on anything else (cause name, `sepc`, `stval`, register dump); `trap_init()`, `panic()` |
| `timer.c` | `timebase_hz()` (sysinfo's `SYS_TIMEBASE_HZ`, read once), `timer_interval()`, `timer_init()` |
| `plic.c` | `plic_init()` (UART source enabled for the S-mode context), `plic_claim()`, `plic_complete()` |
| `riscv.s` | CSR accessors for C (`r_sstatus`, `w_mepc`, ...), `intr_off()`/`intr_on()` and `wfi()` |
| `main.c` | `kmain()`: banner, `kprintf` self-test, ten ticks, the optional deliberate fault, then the shell (`help`, `ticks`, `halt`) |
| `uart.c` | 16550 driver: receive interrupt handler `uart_intr()` filling a ring buffer (`uart_rx_pop()`), polling `uart_putc`/`uart_puts` |
| `console.c` | `console_getc()` (sleeps in `wfi` until a byte arrives), `console_readline()` (echo, backspace, CR/LF/CR LF) |
| `kprintf.c` | `kprintf`/`ksnprintf`: `%d %u %x %p %s %c %%`, `0` flag, width |
| `string.c` | `memset memcpy memmove memcmp strlen strcmp` (clang may call the `mem*` ones by itself) |
| `kernel.ld` | layout: `.text` at 0x8000_0000 (`_start` first), `.rodata`, `.data`, `.bss`, 16 KiB stack, `__kernel_end` |
| `include/` | `types.h` (fixed-width types), `mmio.h` (`volatile` accessors), `varargs.h`, `riscv.h` (CSR bits, cause numbers), `clint.h`, `sysinfo.h`, `plic.h`, and the headers for the above |
| `kernel.elf` | the prebuilt image, committed so `cargo test` and Grail need no LLVM |
| `kernel.elf.clang-version` | the clang that built `kernel.elf` |

## Building

Needs clang and ld.lld, e.g. from the official Windows LLVM installer (found on
`PATH` or in `C:\Program Files\LLVM\bin`):

```
cargo xtask kernel          # builds kernel/kernel.elf
cargo xtask kernel --run    # ... and runs it on Grail, with this terminal as its console
cargo xtask kernel -v       # shows the clang/ld.lld command lines
```

`cargo xtask` is a Rust program (`crates/xtask`), so there is no Makefile or
shell script. It compiles every `*.c` and assembles every `*.s` in this
directory, in name order, then links with `kernel.ld`. By hand, from this
directory, the build is:

```
clang --target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -c entry.s -o entry.s.o   # and each other .s
clang --target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -Iinclude -c main.c -o main.c.o        # and each other .c
ld.lld -T kernel.ld -o kernel.elf console.c.o entry.s.o kernelvec.s.o kprintf.c.o machine.s.o \
       main.c.o plic.c.o riscv.s.o start.c.o string.c.o timer.c.o trap.c.o uart.c.o
```

The build is reproducible: the same clang gives byte-identical output wherever
the repository is checked out. **After changing anything here, run `cargo xtask
kernel` and commit `kernel.elf` with the sources.** When the installed clang
matches `kernel.elf.clang-version`, `cargo test` rebuilds the kernel and fails
if the committed image is stale. With any clang it checks that a fresh build
boots with the same output.

`cargo xtask kernel --as merlin` assembles the `.s` files with merlin-as
instead (bootstrapping step 1, spec section 10). It works once the merlin-as
encoder covers what the `.s` files use; a test runs it automatically from then on.

## Rules for kernel code

The kernel will eventually be compiled by Merlin (#15), so it sticks to plain C:

- No libc: headers come from `include/` (`-nostdinc`), no `<stdint.h>`/`<stdarg.h>`.
- No floating point.
- No inline `asm()`: CSR access and custom instructions live in `.s` files.
- Assembly uses only what merlin-as understands (no `.insn`, no preprocessor);
  Xcal instructions are `.word` constants. A test checks this.
- Varargs (`kprintf`) use clang's `__builtin_va_*` through `include/varargs.h`;
  Merlin has to provide the same builtins.
