# Camelot kernel

The Camelot OS kernel, in C with a little assembly. For now it boots, prints a
banner and a `kprintf` self-test over the UART, and halts.

```
cargo run --release -p grail -- kernel/kernel.elf
```

```
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=80000044 a0 = 0x00000000 (0)
```

The exit code is `kmain()`'s return value (0 = all self-tests passed).

## Files

| File | What it does |
|---|---|
| `entry.s` | `_start`: trap vector, stack, zero `.bss`, `call kmain`, `xcal.halt` with its return value. Also `xcal_dbg`/`xcal_halt` for C and an early trap handler (prints `mcause`, `mepc`, `mtval`, exits 0xff) |
| `main.c` | `kmain()`: banner, `kprintf` self-test |
| `uart.c` | polling 16550 driver: `uart_putc`/`uart_puts` |
| `kprintf.c` | `kprintf`/`ksnprintf`: `%d %u %x %p %s %c %%`, `0` flag, width |
| `string.c` | `memset memcpy memmove memcmp strlen strcmp` (clang may call the `mem*` ones by itself) |
| `kernel.ld` | layout: `.text` at 0x8000_0000 (`_start` first), `.rodata`, `.data`, `.bss`, 16 KiB stack, `__kernel_end` |
| `include/` | `types.h` (fixed-width types), `mmio.h` (`volatile` accessors), `varargs.h`, and the headers for the above |
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
clang --target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -c entry.s -o entry.s.o
clang --target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -Iinclude -c main.c -o main.c.o        # and each other .c
ld.lld -T kernel.ld -o kernel.elf entry.s.o kprintf.c.o main.c.o string.c.o uart.c.o
```

The build is reproducible: the same clang gives byte-identical output wherever
the repository is checked out. **After changing anything here, run `cargo xtask
kernel` and commit `kernel.elf` with the sources.** When the installed clang
matches `kernel.elf.clang-version`, `cargo test` rebuilds the kernel and fails
if the committed image is stale. With any clang it checks that a fresh build
boots with the same output.

`cargo xtask kernel --as merlin` assembles the `.s` files with merlin-as
instead (bootstrapping step 1, spec section 10). It works once the merlin-as
encoder covers what `entry.s` uses; a test runs it automatically from then on.

## Rules for kernel code

The kernel will eventually be compiled by Merlin (#15), so it sticks to plain C:

- No libc: headers come from `include/` (`-nostdinc`), no `<stdint.h>`/`<stdarg.h>`.
- No floating point.
- No inline `asm()`: CSR access and custom instructions live in `.s` files.
- Assembly uses only what merlin-as understands (no `.insn`, no preprocessor);
  Xcal instructions are `.word` constants. A test checks this.
- Varargs (`kprintf`) use clang's `__builtin_va_*` through `include/varargs.h`;
  Merlin has to provide the same builtins.
