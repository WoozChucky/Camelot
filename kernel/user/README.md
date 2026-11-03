# Camelot's user programs

Every `X.c` or `X.s` here is a program called `X`, and so is every
subdirectory `X/` but `lib/` (a program of several files: `as/`). They run in U-mode on
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
| `sh` | `sh` | the shell: commands, `<`, `>`, `\|`, `&`, and the built-ins `cd`, `wait`, `exit`, `help`; it reads the console a line at a time, any other input (a connection from `shd`, a pipe, a file) a byte at a time, up to each newline |
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

## Tools

| Program | Usage | What it does |
|---|---|---|
| `as` | `as [-o OUT.o] FILE.s` | the assembler, merlin-as rewritten in C (`as/`): the same language, the same object byte for byte, the same error messages (`FILE:LINE: error: ...`); no line tables (`.file N`, `.loc`) yet. The first step of self-hosting: section 10, "Self-hosting", of the spec. Only in `/bin` of the root image: the kernel does not build it in (it needs files) |

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
| `shd` | `shd [-p PORT] [-n MAX] [-v]` | the shell server: each TCP connection to PORT (23) gets a shell of its own, `/bin/sh` with the connection as its input, output and error, at most MAX (4, up to 8) at once; `-v` logs each session; runs until killed (see below) |

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

### shd, a shell for each connection

The console is one for the whole machine: everyone attached to it (each
Herald, each Pendragon page) shares its input and output, so while its
shell runs `sleep 5000` nobody gets a prompt. `shd` gives each TCP
connection a shell of its own instead, as telnet and ssh servers do: one
busy session never holds up another, nor the console's. Start it in the
background from the console, with the machine's port 23 forwarded from the
host:

```
$ shd &
[5]
$
```

(`cargo xtask up --demo` does that for its VM, with 127.0.0.1:2323
forwarded; with Grail, `--net user,hostfwd=tcp::2323-:23`.) Then, on the
host, `herald --tcp 127.0.0.1:2323` gives a shell, once per window.

- **Lines are edited by the client.** A socket has no line discipline: the
  console's kernel driver edits and echoes what is typed, a connection does
  not. Herald (`--tcp`) edits and echoes the line locally and sends it on
  Enter, as a telnet client does; the shell reads the connection a byte at
  a time up to each newline (a `\r` before it is dropped), so the rest is
  left for the programs it starts, which read the same connection.
- **The session ends with the shell**: shd collects it and ends the
  connection, even if a background program still holds it. `exit` leaves
  background programs running (`sleep 60 &`, then `exit`: the sleep goes
  on, and ends by itself).
- **Closing the connection hangs up**, as with telnet, and so does the end
  of the input (Ctrl-D on an empty line in Herald, which closes Herald's
  side). Between commands, the shell reads the end of its input, kills
  every program it started, background ones included, and ends. In the
  middle of a command, once the shell has read all the client sent, shd
  kills every process the session started, `sleep 5000` included, and the
  shell then ends (a shell still there a second later is killed too). A
  script that sends commands and closes its side should end with `exit`,
  so its last command is not cut short.
- **Lines typed ahead keep a session going.** While a command runs, the
  lines typed after it wait unread, and after the client's FIN nothing
  tells a client that left from a script that only closed its sending side
  and still reads the output: the session carries on. A reset ends it at
  once, dropping what was unread, and the gateway (Grail's or a NAT
  switch's) resets the connection soon after the session sends anything
  to a client that is gone, or after a minute in which nothing moved.
- **At most MAX sessions** (`-n`, 4 by default): one more is told
  `shd: too many sessions (MAX), try again later` and closed. Every program
  a session runs takes one of the kernel's 8 process slots too.
- **Security: for local use only.** There are no users yet (issue #115),
  so every shell runs as whoever started shd, root, with no password asked.
  Forward its port from the host's loopback only (127.0.0.1, as `up --demo`
  and `hostfwd`'s default do), never from an address other machines can
  reach. Once Camelot has users, shd will run `login` for each connection.

With `-v`, shd logs on its output (the console) a line per event:

```
shd: listening on port 23
shd: [7] 10.0.2.2:49152 connected
shd: [8] 10.0.2.2:49153 connected
shd: [8] 10.0.2.2:49153 hung up
shd: [8] 10.0.2.2:49153 ended, exit -1
shd: 10.0.2.2:49154 turned away: too many sessions
shd: [7] 10.0.2.2:49152 ended, exit 0
```

(behind a NAT gateway, every client comes from 10.0.2.2).

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
are `crates/grail/tests/kernel_net.rs`, `kernel_tcp.rs`,
`kernel_sockets.rs` and `kernel_shd.rs` (shd: sessions side by side, one
asleep while the others and the console work, hang-ups, the limit, nothing
left behind), and `crates/avalon/tests/sockets.rs`; the threads' are
`crates/xtask/tests/threads.rs`.
