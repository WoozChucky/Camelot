# Camelot's user programs

Every `X.c` or `X.s` here is a program called `X`. They run in U-mode on
the system calls of `docs/camelot-syscalls.md`, declared in `user.h` with
the C library of `lib/` (see `lib/README.md`). Built by merlin, merlin-as
and merlin-ld (`cargo xtask kernel`, which embeds them in the kernel for
when there is no disk; `cargo xtask fs`, which installs them in `/bin` of
the root image). The shell runs `NAME` as `/bin/NAME`.

## The basic programs

Each one reports a problem as `NAME: WHAT: REASON` on stderr (the shell
then prints `NAME: exit STATUS`) and exits with a non-zero status: 1, or 2
for `grep`, whose 1 means "no line matched".

| Program | Usage | What it does |
|---|---|---|
| `sh` | `sh` | the shell: commands, `<`, `>`, `\|`, `&`, and the built-ins `cd`, `wait`, `exit`, `help` |
| `ls` | `ls [PATH...]` | lists directories (the working directory without arguments): type, inode, links, size, name |
| `cat` | `cat [FILE...]` | copies files, or its input, to its output |
| `echo` | `echo [-n] [ARG...]` | prints its arguments, with a newline unless `-n` |
| `mkdir` | `mkdir DIR...` | makes directories |
| `rm` | `rm PATH...` | removes names: files, and empty directories |
| `ln` | `ln OLD NEW` | gives a file a second name |
| `cp` | `cp SRC DST`, `cp SRC... DIR` | copies files (made or emptied first), or into a directory under their own names |
| `mv` | `mv SRC DST`, `mv SRC... DIR` | renames files (`link`, then `unlink`; a file already called DST is replaced), or moves them into a directory; directories can't be moved |
| `wc` | `wc [-l] [-w] [-c] [FILE...]` | counts lines, words and bytes, of files or its input; the flags pick the counts |
| `grep` | `grep [-v] [-c] [-n] PATTERN [FILE...]` | prints the lines that contain a fixed string; `-v` those that don't, `-c` how many, `-n` with their numbers |
| `ps` | `ps` | lists the processes (`procs`): pid, parent, state, size, name |
| `kill` | `kill PID...` | ends processes (`kill`): they exit with status -1 |
| `sleep` | `sleep SECONDS` | waits, `0.5` for half a second (the clock ticks 10 times a second) |

So, on the root image:

```
$ cat /README | grep Camelot | wc -l
1
$ cp /README /r
$ mv /r /notes
$ sleep 100 &
[12]
$ ps
  PID  PPID STATE      SIZE NAME
    6     0 sleep     36864 sh
   12     6 sleep     20480 sleep
   13     6 run       20480 ps
$ kill 12
$ wait
[12] sleep: exit -1
```

## Tests and demonstrations

| Program | What it does |
|---|---|
| `hello` | prints a line and exits |
| `ticks` | prints the timer ticks since boot |
| `ping`, `pong` | print four numbered lines half a tick apart: run together, they take turns |
| `regs` | fills every register, spins for three ticks and exits with 0 only if none changed |
| `sysregs` | fills every register, makes system calls and exits with 0 only if none but a0 changed |
| `data` | exits with 0 only if its stack and `.bss` start zeroed and its `.data` holds its initial value and takes stores |
| `badptr` | hands the system calls bad pointers, which must all fail with `-EFAULT` |
| `systest` | checks every system call, its results and its errors (`systest: N checks, 0 failed`) |
| `filetest` | checks the file system calls on a writable root filesystem (`filetest: N checks, 0 failed`) |
| `libtest` | checks the C library (`libtest: N checks, 0 failed`) |
| `badload` | reads kernel memory: killed by a load page fault |
| `badstore` | writes to its own read-only data: killed by a store page fault |
| `badhalt` | executes `xcal.halt`, which is M-mode only: killed by an illegal instruction |

`crates/xtask/tests/coreutils.rs` boots Camelot with the root image and
runs each basic program, its errors included, from the shell, and checks
what it printed and what it left on the disk.
