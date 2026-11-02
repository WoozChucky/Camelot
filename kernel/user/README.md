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
| `cat` | `cat [FILE...]` | copies files, or its input, to its output; refuses a directory |
| `echo` | `echo [-n] [ARG...]` | prints its arguments, with a newline unless `-n` |
| `mkdir` | `mkdir DIR...` | makes directories |
| `rm` | `rm PATH...` | removes names: files, and empty directories |
| `ln` | `ln OLD NEW` | gives a file a second name |
| `cp` | `cp SRC DST`, `cp SRC... DIR` | copies files (made or emptied first), or into a directory under their own names |
| `mv` | `mv SRC DST`, `mv SRC... DIR` | renames files (`link`, then `unlink`; a file already called DST is replaced, and if DST is a hard link to SRC only SRC goes), or moves them into a directory; directories can't be moved |
| `wc` | `wc [-l] [-w] [-c] [FILE...]` | counts lines, words and bytes, of files or its input; the flags pick the counts; refuses a directory |
| `grep` | `grep [-v] [-c] [-n] PATTERN [FILE...]` | prints the lines that contain a fixed string; `-v` those that don't, `-c` how many, `-n` with their numbers |
| `ps` | `ps` | lists the processes (`procs`): pid, parent, state, size, name |
| `harts` | `harts` | lists the harts (`harts`): id, the process running on it (`-` while idle), the ticks it has taken and the processes it has switched to |
| `kill` | `kill PID...` | ends processes (`kill`): they exit with status -1; not the shell, whose end is Camelot's |
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

## Network programs

On the socket system calls (`docs/camelot-syscalls.md`, "Sockets") and
the network's own (`netinfo`, `netconf`, `ping`); `docs/camelot-net.md`,
"Network programs", says more. A HOST is an address or a name, which the
library's resolver asks the DNS server for. Each reports a problem as
`NAME: REASON` and exits with 1.

| Program | Usage | What it does |
|---|---|---|
| `ifconfig` | `ifconfig [ADDR [MASK [GATEWAY]]]` | prints the interface, its configuration and its counters; with ADDR, gives it a static address (MASK 255.255.255.0 and no gateway if left out) |
| `ping` | `ping HOST [N]` | sends N ICMP echo requests (4 if left out), a second apart, and prints each reply's round trip; exits 0 if one came |
| `nc` | `nc [-u] [-v] HOST PORT`, `nc -l [-u] [-v] PORT` | netcat: connects to HOST, or listens for one connection (UDP: the first datagram), and copies its input to the other end and what comes back to its output, both at once; TCP by default, UDP with `-u`, saying what it does with `-v` |
| `fetch` | `fetch http://HOST[:PORT][/PATH]` | downloads with HTTP/1.0 to its output; any status but 2xx is an error |
| `httpd` | `httpd [-p PORT] [-n COUNT] [-t SECONDS] [DIR]` | serves DIR's files (index.html or a listing for a directory) over HTTP/1.0 on PORT (80), a log line per request, for COUNT connections or until killed |

So, with `grail --disk target/camelot-fs.img --net user,hostfwd=tcp::8080-:80`:

```
$ ping 10.0.2.2 1
PING 10.0.2.2: 56 data bytes
64 bytes from 10.0.2.2: seq=1 ttl=64 time=1.211 ms
1 sent, 1 received
$ fetch http://10.0.2.2:8000/notes.txt > notes.txt
$ httpd -n 1 &
[9]
httpd: serving / on port 80
10.0.2.2:49153 GET /README 200 1325
```

the last line once `http://127.0.0.1:8080/README` was opened on the host.

## Tests and demonstrations

| Program | What it does |
|---|---|
| `hello` | prints a line and exits |
| `ticks` | prints the timer ticks since boot |
| `ding`, `dong` | print four numbered lines half a tick apart: run together, they take turns |
| `regs` | fills every register, spins for three ticks and exits with 0 only if none changed |
| `sysregs` | fills every register, makes system calls and exits with 0 only if none but a0 changed |
| `data` | exits with 0 only if its stack and `.bss` start zeroed and its `.data` holds its initial value and takes stores |
| `badptr` | hands the system calls bad pointers, which must all fail with `-EFAULT` |
| `systest` | checks every system call, its results and its errors (`systest: N checks, 0 failed`) |
| `filetest` | checks the file system calls on a writable root filesystem (`filetest: N checks, 0 failed`) |
| `stress` | many processes at once: pages, the console, a shared pipe and files under timer preemption (`stress: W workers, R rounds, N bytes: ok`) |
| `threads` | `threads [THREADS [ROUNDS]]`: THREADS threads (4, at most 8) of one process each add 1 to a counter ROUNDS (500) times under a mutex, and to another under a spinlock, by reading, working and writing back, which loses updates without the lock; then joins them and checks both totals (`threads: T threads, R rounds: counter N, spun N: ok`) |
| `threadtest` | checks the thread and futex calls, their results and their errors, the library's locks and `malloc` from several threads, and processes with threads that `exit`, `exec`, fault or are killed (`threadtest: N checks, 0 failed`); `threadtest fs`, on the root filesystem, the same while threads are in file calls (`threadtest fs: N checks, 0 failed`) |
| `primes` | `primes [WORKERS [LIMIT]]`: counts the primes below LIMIT (20000) with WORKERS (4, at most 6) processes at once, one slice of the range each, to keep several harts busy (`primes: N below LIMIT, W workers`) |
| `libtest` | checks the C library (`libtest: N checks, 0 failed`) |
| `nettest` | moves known bytes through TCP: `nettest connect ADDR PORT BYTES [TIMES]` sends a pattern and reads to the end, `nettest serve PORT CONNECTIONS [BACKLOG]` echoes what each connection sends; each prints the bytes' counts and FNV-1a hash (the TCP tests' program) |
| `socktest` | checks the socket calls, their results and their errors, over loopback (`socktest: N checks, 0 failed`; needs a network card) |
| `badload` | reads kernel memory: killed by a load page fault |
| `badstore` | writes to its own read-only data: killed by a store page fault |
| `badhalt` | executes `xcal.halt`, which is M-mode only: killed by an illegal instruction |

`crates/xtask/tests/coreutils.rs` boots Camelot with the root image and
runs each basic program, its errors included, from the shell, and checks
what it printed and what it left on the disk. The network programs' tests
are `crates/grail/tests/kernel_net.rs`, `kernel_tcp.rs` and
`kernel_sockets.rs`, and `crates/avalon/tests/sockets.rs`; the threads'
are `crates/xtask/tests/threads.rs`.
