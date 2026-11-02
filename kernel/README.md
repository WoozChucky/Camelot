# Camelot kernel

The Camelot OS kernel, in C with a little assembly. A small M-mode layer sets
up trap delegation and the timer, then the kernel runs in S-mode: it prints a
banner and a `kprintf` self-test over the UART, puts every free page of RAM on
the page allocator's free list, turns on Sv32 paging with a kernel page table,
runs the memory self-tests, takes ten timer ticks, and halts. Faults panic with
the cause, `sepc`, `stval` and a register dump.

```
cargo run --release -p grail -- kernel/kernel.elf
```

```
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=80000054 a0 = 0x00000000 (0)
ram: 128 MiB at 0x80000000
paging: on (Sv32)
vm self-test: 24/24 passed
timer: 10 Hz tick, 10000000 Hz timebase
tick 1
tick 2
...
tick 10
```

The RAM size comes from Grail's sysinfo device, so `--ram` changes what the
allocator manages (`cargo run --release -p grail -- --ram 16 kernel/kernel.elf`
prints `ram: 16 MiB at 0x80000000`). Booting takes about 4.5 million
instructions with 128 MiB, nearly all of them spent setting up memory one page
at a time.

The exit code is `kmain()`'s return value (0 = all self-tests passed), 0xfe
after a kernel panic, or 0xff after an unexpected M-mode trap. The traps,
the M-mode layer and the panic format are described in
`docs/excalibur-spec.md` section 10, "Kernel traps"; the allocator, the page
tables and the kernel address space in "Kernel memory".

A panic looks like this (from the deliberate illegal instruction the tests
trigger by patching `boot_fault` in the image to 2):

```
panic: illegal instruction
  scause 0x00000002  sepc 0x800002bc  stval 0xc0001073  sstatus 0x00000120
   ra 01010101   sp 80007400   gp 03030303   tp 04040404
   t0 05050505   t1 06060606   t2 07070707   s0 08080808
   ...
   t4 1d1d1d1d   t5 1e1e1e1e   t6 1f1f1f1f
```

A page fault adds a line on how the kernel page table maps `stval`. With
`boot_fault` = 4 the kernel stores to its own code, which is mapped read and
execute only (3 loads from the unmapped address 0x100 instead, which reports
`va 0x00000100 is not mapped`):

```
panic: store page fault
  scause 0x0000000f  sepc 0x80002974  stval 0x8000199c  sstatus 0x00000120
  va 0x8000199c maps to pa 0x8000199c, pte 0x2000044b -A--X-RV
   ra 80002918   sp 8000a060   gp 00000000   tp 00000000
   ...
```

The PTE flags are printed bit 7 to bit 0 (`DAGUXWRV`), `-` where clear.

## Files

| File | What it does |
|---|---|
| `entry.s` | `_start` (M-mode): early trap vector, stack, zero `.bss`, `call start`, `mret` to S-mode. `supervisor_entry`: `call kmain`, then `machine_halt` with its return value. Also `xcal_dbg`/`xcal_halt` for C and the early trap handler (prints `mcause`, `mepc`, `mtval`, exits 0xff) |
| `start.c` | `start()`: M-mode setup: `medeleg`/`mideleg`, `stvec`, `mtvec`, `mscratch`, the first timer interrupt, `mstatus.MPP = S`, `mepc` |
| `machine.s` | `machine_vec`, the M-mode trap handler: forwards the machine timer to S-mode as SSIP, serves machine calls (`MCALL_HALT`). `machine_halt()` makes that call from S-mode |
| `kernelvec.s` | `kernel_vec`, the S-mode trap vector: saves x1-x31 and the trap CSRs as a `struct trap_frame`, calls `kernel_trap`, restores, `sret`. Also `illegal_instruction()` for the panic tests |
| `trap.c` | `kernel_trap()`: counts timer ticks, resumes faulting memory probes, panics on anything else (cause name, `sepc`, `stval`, the PTE for a page fault, register dump); `trap_init()`, `panic()` |
| `kalloc.c` | physical page allocator: `kinit()` reads RAM's base and size from sysinfo and frees every page from `__kernel_end` to the end of RAM; `kalloc()`, `kzalloc()`, `kfree()`, `kfree_pages()` |
| `vm.c` | Sv32 page tables: `walk()`, `map_pages()`, `unmap_pages()`, `pagetable_create()`/`pagetable_free()`; `kvminit()` builds the kernel page table, `kvminithart()` turns paging on |
| `vmtest.c` | the memory self-tests: allocate and free every page, a second mapping of one page, store page faults on text and rodata, building and freeing a page table |
| `probe.s` | `probe_load()`/`probe_store()`: a word access that returns the trap cause instead of panicking (`kernel_trap` resumes them at `probe_fault`) |
| `sysinfo.c` | reads Grail's sysinfo block: registers and the device table |
| `timer.c` | `timebase_hz()` (nominal 10 MHz, the one place to change once Grail reports it), `timer_interval()`, `timer_init()` |
| `riscv.s` | CSR accessors for C (`r_sstatus`, `w_mepc`, `w_satp`, ...), `sfence_vma()`, `intr_off()`/`intr_on()` and `wfi()` |
| `main.c` | `kmain()`: banner, `kprintf` self-test, ten ticks, the optional deliberate fault |
| `uart.c` | polling 16550 driver: `uart_putc`/`uart_puts` |
| `kprintf.c` | `kprintf`/`ksnprintf`: `%d %u %x %p %s %c %%`, `0` flag, width |
| `string.c` | `memset memcpy memmove memcmp strlen strcmp` (clang may call the `mem*` ones by itself) |
| `kernel.ld` | layout: `.text` at 0x8000_0000 (`_start` first), then page-aligned `.rodata` and `.data` (so each gets its own page permissions), `.bss`, 16 KiB stack, `__kernel_end` |
| `include/` | `types.h` (fixed-width types), `mmio.h` (`volatile` accessors), `varargs.h`, `riscv.h` (CSR bits, cause numbers), `clint.h`, `memlayout.h` (page size, linker symbols), `sysinfo.h` (register offsets, device types), and the headers for the above |
| `kernel.elf` | the prebuilt image, committed so `cargo test` and Grail need no LLVM |
| `kernel.elf.clang-version` | the clang that built `kernel.elf` |

## Building

Needs clang and ld.lld, e.g. from the official Windows LLVM installer (found on
`PATH` or in `C:\Program Files\LLVM\bin`):

```
cargo xtask kernel          # builds kernel/kernel.elf
cargo xtask kernel --run    # ... and runs it on Grail
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
ld.lld -T kernel.ld -o kernel.elf entry.s.o kalloc.c.o kernelvec.s.o kprintf.c.o machine.s.o \
       main.c.o probe.s.o riscv.s.o start.c.o string.c.o sysinfo.c.o timer.c.o trap.c.o uart.c.o \
       vm.c.o vmtest.c.o
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
