# Camelot's user programs

Every `X.c` or `X.s` here is a program called `X`, and so is every
subdirectory `X/` other than `lib/` and `gui/` (a program of several files: `as/`),
except `cc/`, which is `cc1`, the compiler proper (`userprogs::RENAMED`). They run in U-mode on
the system calls of `docs/camelot-syscalls.md`, declared in `user.h` with
the C library of `lib/` (see `lib/README.md`); the programs that draw
also use the GUI library of `gui/`, declared in `gui.h` (see
`gui/README.md`). Built by merlin, merlin-as
and merlin-ld (`cargo xtask kernel`, which embeds them in the kernel for
when there is no disk; `cargo xtask fs`, which installs them in `/bin` of
the root image). The shell runs `NAME` as `/bin/NAME`.

Every program is RV64 code, an ELF64 executable (LP64: `long`, `size_t`
and pointers 8 bytes), and the root image's `/bin` (`cargo xtask fs`'s
`target/camelot-fs.img`) holds them all, `as` and `ld` writing RV64 objects
and executables there. Each is linked as a C program is on Linux, the start
code `crt0.o` first and the C library `libc.a` (`lib/`, an archive) last,
after the GUI library `libgui.a` (`gui/`, an archive too: a program takes
only the members it uses, so one that draws nothing is the same without
it), and the root image's `/usr` holds what they are built from: `user.h`
and `gui.h` in `/usr/include`, and `crt0.o`, `libc.a`, `libgui.a` and
`user.ld` in `/usr/lib`. Assembly that goes through the C preprocessor is a
`.S` file, with `include/asm.h` for the names Linux's assembly gives a
register's worth and a pointer (`REG_L`, `SZREG`).
A program starts as Linux starts one: `main(argc, argv, envp)`, with the
auxiliary vector after `envp` (docs/camelot-syscalls.md, "Starting a
program"); crt0 makes `envp` the C library's `environ`, which `getenv`
looks in and a program passes on to `spawn` and `exec`. The shell starts
with the kernel's environment for the first process, Linux's for init
(`HOME=/`, `TERM=linux`), and passes its exported variables on.

## The basic programs

Each one reports a problem as `NAME: WHAT: REASON` on stderr (the shell
then prints `NAME: exit STATUS`) and exits with a non-zero status: 1, or 2
for `grep`, whose 1 means "no line matched".

| Program | Usage | What it does |
|---|---|---|
| `init` | `init` (the kernel's, as pid 1) | the first process: starts the services `/etc/rc` lists, `once PATH [ARG...]` or `respawn PATH [ARG...]` a line, restarts the `respawn` ones as they end (but one that exits 78, `EX_CONFIG`, within a second of starting, or ends within a second five times in a row), ends the machine once the `once` ones have ended, and collects orphans; without `/etc/rc` it runs `respawn /bin/login` if `/etc/passwd` exists, else execs `/bin/sh` (excalibur-spec section 10, "Init and the shell") |
| `sh` | `sh` | the shell: commands, `<`, `>`, `\|`, `&`, `NAME=VALUE` assignments, and the built-ins `cd`, `wait`, `exit`, `exec`, `export`, `unset`, `help`; it reads the console a line at a time, any other input (a connection from `shd`, a pipe, a file) a byte at a time, up to each newline |
| `ls` | `ls [-l] [PATH...]` | lists directories (the working directory without arguments): type, inode, links, size, name; with `-l` the mode (`drwxr-xr-x`), links, owner, group, size and name |
| `login` | `login` | asks `login: ` and `Password: ` (the echo off), checks `/etc/shadow`, and becomes the user: `setgid`, `setuid`, the home directory, `HOME`, `USER`, `LOGNAME`, `SHELL`, then execs the account's shell; `Login incorrect` after a second for a wrong one; without `/etc/passwd`, execs `/bin/sh` as root (spec section 10, "Users") |
| `su` | `su [USER]` | a shell as USER (root by default), after USER's password unless the caller is root; set-user-ID root |
| `passwd` | `passwd [USER]` | changes a password, the caller's own (after the current one) or, for root, anyone's: a new salt, the line of `/etc/shadow` rewritten; set-user-ID root |
| `id` | `id` | `uid=1000(user) gid=1000(user)`, and `euid=`/`egid=` when they differ |
| `whoami` | `whoami` | the effective user's name |
| `chmod` | `chmod MODE FILE...` | sets files' permission bits (octal, at most 7777); the owner's or root's |
| `chown` | `chown USER[:GROUP] FILE...` | gives files to USER (and GROUP): names or numbers, `USER:` USER's login group; root's alone |
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
| `harts` | `harts` | lists the harts (`harts`): id, the process running on it (`-` while idle), the ticks it has taken, how many of them it was idle and the processes it has switched to |
| `stty` | `stty`, `stty size`, `stty rows N cols N` | a terminal's size, standard input's, as Linux's `stty` shows (`rows R; columns C;`, `size`: `R C`) and sets it (`TIOCGWINSZ`, `TIOCSWINSZ`) |
| `env` | `env [-i] [-u NAME]... [NAME=VALUE]... [PROG [ARG...]]` | runs PROG with its environment emptied (`-i`), without NAME (`-u`) or with NAME set, or without PROG prints the environment; 125 if env fails, 127 if there is no PROG, 126 if it can't be run |
| `printenv` | `printenv [NAME...]` | prints the value of each NAME (`getenv`), a line each, or the whole environment without a NAME; exits 1 if a NAME has no value |
| `kill` | `kill PID...` | ends processes (`kill`): they exit with status -1; not the shell, whose end is Camelot's |
| `sleep` | `sleep SECONDS` | waits, `0.5` for half a second (the clock ticks 10 times a second) |
| `date` | `date [-s]` | prints the date and time in UTC as `date -u` does (`Thu Jan  1 00:00:05 UTC 2026`), from the real-time clock (`CLOCK_REALTIME`); `-s` the seconds since 1970 |

So, on the root image:

```
# cat /README | grep Camelot | wc -l
1
# cp /README /r
# mv /r /notes
# sleep 100 &
[12]
# ps
  PID  PPID STATE      SIZE NAME
    6     0 sleep     36864 sh
   12     6 sleep     20480 sleep
   13     6 run       20480 ps
# kill 12
# wait
[12] sleep: exit -1
```

## Tools

| Program | Usage | What it does |
|---|---|---|
| `as` | `as [-march=ISA] [-o OUT.o] FILE.s` | the assembler, merlin-as rewritten in C (`as/`): the same language, the same object byte for byte (RV64, ELF64; `-march` takes `rv64ima_zicsr_zifencei`, the default), the same error messages (`FILE:LINE: error: ...`), and the same line tables from `.file`/`.loc` (merlin `-g` output). The first step of self-hosting: section 10, "Self-hosting", of the spec. Only in `/bin` of the root image: the kernel does not build it in (it needs files) |
| `ld` | `ld [-T SCRIPT] [-e ENTRY] [-o OUT] [-S] [-L DIR] FILE.o ... [LIB.a] [-lNAME] [--start-lib LIB.o ... --end-lib] [@ARGS]` | the linker, merlin-ld rewritten in C (`ld/`): the same scripts (`AT` load addresses and `ADDR` included), archives and libraries (`-lNAME` is `libNAME.a` in the first `-L` directory that has it, its members linked only when needed, in ld.lld's order) and messages, the same executable byte for byte (ELF64; an ELF32 object is refused, as merlin-ld refuses it); `@ARGS` reads more arguments from a file, for link lines longer than the shell takes. `as` then `ld` builds a program on Camelot itself, as the host does: `ld -T /usr/lib/user.ld -o PROG /usr/lib/crt0.o PROG.o -L/usr/lib -lc`. Only in `/bin` of the root image, as `as` |
| `ar` | `ar [-]{r,t,x}[c][s][S][D] ARCHIVE [FILE...] [@ARGS]` | the archiver (`ar/`): `r` puts files in an archive, made if missing (replacing members of their names in place, the others at the end), `t` lists the members, `x` writes them to files; `s` (the default) writes the symbol table, `S` does not, `c` makes the archive without saying so. The archive llvm-ar writes for the same members, byte for byte (the GNU format, deterministic: times and ids 0, mode 644). `ar rcs libx.a a.o b.o`, then `ld ... -L. -lx`. Only in `/bin` of the root image, as `as` |
| `cc1` | `cc1 [-g [-fdebug-compilation-dir=DIR]] [-I DIR] [-D NAME[=VALUE]] [-U NAME] [-x LANG] [-o OUT.s] FILE.c...`, `cc1 -E [-P] ...`, `cc1 --dump-ast ...` | the C compiler proper, merlin rewritten in C (`cc/`), landing in stages (#241). It compiles C to the assembly `merlin` writes, byte for byte (`cc1 prog.c` writes `prog.s`, for `as`), with merlin's diagnostics and warnings (`FILE:LINE:COL: error: ...`, clang's words, positions and notes) and exit status; `cc1 -E` writes what `merlin -E` writes, for C and for assembly (a `.S` file, or `-x assembler-with-cpp`: line markers, or none with `-P`), and `--dump-ast` merlin's tree. It takes every option merlin takes: with `-g` its assembly has merlin's line table (`.file`, with each file's MD5, and `.loc`) and DWARF compile unit, the names relative to `-fdebug-compilation-dir`, by default the directory it runs in. Only in `/bin` of the root image, as `as` |

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
| `shd` | `shd [-p PORT] [-n MAX] [-v]` | the shell server: each TCP connection to PORT (23) gets a shell of its own, `/bin/sh` with the connection as its input, output and error, at most MAX (4, up to 8) at once; `-v` logs each session; runs until killed (see below); exits 78, `EX_CONFIG`, on a machine with no network card, so that init leaves it |

So, with `grail --disk target/camelot-fs.img --net user,hostfwd=tcp::8080-:80`:

```
# ping 10.0.2.2 1
PING 10.0.2.2: 56 data bytes
64 bytes from 10.0.2.2: seq=1 ttl=64 time=1.211 ms
1 sent, 1 received
# fetch http://10.0.2.2:8000/notes.txt > notes.txt
# httpd -n 1 &
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
busy session never holds up another, nor the console's. The root image's
init starts it at boot (its `/etc/rc`, `respawn /bin/shd`); on a disk
without init, start it in the background from the console, with the
machine's port 23 forwarded from the host:

```
# shd &
[5]
#
```

(`cargo xtask up --demo` forwards 127.0.0.1:2323 to its VM's, and types
`shd &` on an old disk whose init does not start it; with Grail, `--net
user,hostfwd=tcp::2323-:23`.) Then, on the
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
  a session runs takes one of the kernel's 32 process slots too.
- **Security: for local use only.** Each connection starts with `login`
  (excalibur-spec section 10, "Users"), so a shell needs an account's
  password, but the passwords travel in clear text (there is no TLS), the
  image's defaults are documented, and the hash is a teaching system's.
  Forward its port from the host's loopback only (127.0.0.1, as `up --demo`
  and `hostfwd`'s default do), never from an address other machines can
  reach. On a disk without accounts, `login` runs a root shell at once.

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
| `permtest` | checks users and permissions on a writable root filesystem (`permtest: N checks, 0 failed`): root's bypass, then as uid 1000 every refusal, set-user-ID programs, `chmod`, `chown`, `setuid`, `kill`, `netconf`, a 0700 local socket, the devices, and with a network card port 80 |
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
| `guitest` | `guitest SCENE [DIR]`: draws a scene of the GUI library into memory and writes it to `DIR/SCENE.ppm` (`draw`, `text`, `image`; `widgets` writes `widgets.ppm` and `input.ppm`, after input, and prints the actions), prints what the parts that draw nothing did (`damage`, `keys`, `utf8`, `wire`), or, `wayland`, is a scripted display server on `/run/wayland-0` that runs `guidemo`, prints its requests and writes the frames it commits. Only in `/bin` of the desktop's image |
| `guidemo` | the GUI library's demonstration, a Wayland client: a window with menus, a text field, a list of knights, a status line and buttons. Only in `/bin` of the desktop's image |
| `compositor` | `compositor [-v] [-l LAUNCHER] [-s SOCKET]`: the display server (the directory `compositor/`, a program of several files; spec section 10, "The display server"): takes `/dev/fb0` and the input devices, serves Wayland clients at `/run/wayland-0`, composites their windows and manages them; the logo key starts LAUNCHER. The desktop's `/etc/rc` starts it. Only in `/bin` of the desktop's image |
| `panel` | `panel [-v] [PROGRAM ARGS...]`: the desktop's panel (spec section 10, "The desktop shell"), a bar along the top of the screen: the launcher's menu of `/usr/share/applications`, a button for each window (a click activates it), the harts' load and the clock; starts PROGRAM when no window is shown. The desktop's `/etc/rc` starts it with `/bin/term`. Only in `/bin` of the desktop's image, as the four below |
| `term` | `term [-v] [-T TITLE] [PROGRAM ARGS...]`: the terminal emulator, a window with PROGRAM (`/bin/sh`) on a pseudo-terminal: the VT100 and ANSI sequences the framebuffer console takes too (`kernel/vt.c`), scrollback, a selection copied to the clipboard and pasted, resizing (`TIOCSWINSZ`) |
| `files` | `files [DIRECTORY]`: the file browser, a directory's entries in a list, and a file's text |
| `sysmon` | `sysmon`: the system monitor, the harts' load, the memory and the processes, every second |
| `gears` | `gears`: three gears turning, glxgears' in fixed point; prints its frame rate every 5 seconds |
| `wltest` | the display server's test client (`crates/xtask/tests/display.rs`): `wltest window [-n] [-f] [-m] NAME RRGGBB W H` opens a window of one colour and prints every event it gets (`-n` won't close, `-f` flashes every frame, `-m` asks to be moved every frame as a client shouldn't); `wltest probe` prints the globals; `wltest stale` makes a window over the wire itself and asks to be moved with a stale serial, then the right one; `wltest bad KIND` breaks the protocol one way and prints the error it gets. Only in `/bin` of the desktop's image |

`crates/xtask/tests/coreutils.rs` boots Camelot with the root image and
runs each basic program, its errors included, from the shell, and checks
what it printed and what it left on the disk. The network programs' tests
are `crates/grail/tests/kernel_net.rs`, `kernel_tcp.rs`,
`kernel_sockets.rs` and `kernel_shd.rs` (shd: sessions side by side, one
asleep while the others and the console work, hang-ups, the limit, nothing
left behind), and `crates/avalon/tests/sockets.rs`; the threads' are
`crates/xtask/tests/threads.rs`.
