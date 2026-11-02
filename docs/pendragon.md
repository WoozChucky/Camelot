# Pendragon: the web console

Pendragon (`crates/pendragon`) serves a browser page for the Avalon platform: the VMs and virtual switches of an Avalon server and the disk images of a Lake server. It is a separate program and a client of both servers over the Avalon protocol ([`avalon-protocol.md`](avalon-protocol.md)), like Round Table, and every operation goes through the same client layer (`crates/avalon-client`): the two front ends accept the same syntax (`--ram`, `--clock`, disk sources and sizes) and cannot drift apart. Avalon and Lake stay headless, and Lake may run on another machine.

Everything is std only and written in the crate: a small HTTP/1.1 server on `std::net` (`http.rs`), JSON (`json.rs`) and base64 (`base64.rs`) for the API, and the page itself, plain HTML, JavaScript and CSS (`src/ui/`) built into the binary with `include_str!`. There is no frontend build step.

```
pendragon [--listen ADDR] [--avalon ADDR] [--lake ADDR] [--avalon-max-ram MiB] [--allow-host NAME]...
```

| Option | Default | Meaning |
|---|---|---|
| `--listen` | `127.0.0.1:7402` | where to serve the page (a host alone means port 7402) |
| `--avalon` | `127.0.0.1:7400` | Avalon's address (a host alone means port 7400) |
| `--lake` | `127.0.0.1:7401` | Lake's address (a host alone means port 7401) |
| `--avalon-max-ram` | 4096 | Avalon's RAM budget in MiB (its `--max-ram`), for the overview: the protocol does not report it |
| `--allow-host` | | another `Host` the page may be reached by (repeatable; see below) |

Pendragon prints the URL to open on stdout, alone on its line: `http://127.0.0.1:7402/?token=<64 hex digits>`. `cargo xtask up` starts it with Avalon and Lake, and prints the URL again once all three are up (README, *Quick start*).

## 1. Security

Pendragon has no user accounts in v0. It listens on loopback by default, but a browser on the same machine opens other web pages too, and any of them can send requests to loopback. So:

1. **`Host` is checked on every request** (403 otherwise). A page on `evil.example` can make that name resolve to 127.0.0.1 (DNS rebinding) and then read the responses as its own origin; its requests still carry `Host: evil.example`. Accepted are the listening address with its port and, for a loopback or unspecified address, `localhost`, `127.0.0.1` and `[::1]` with the port, plus each `--allow-host` name (with the listening port unless it gives one). Port 80 may also be left out, as browsers do.
2. **Every API request carries the launch's token** in the `X-Pendragon-Token` header (403 `no token` or `wrong token` otherwise). The token is 32 bytes from the operating system's random source, through hand-written FFI to `BCryptGenRandom` on Windows and from `/dev/urandom` elsewhere, made at each launch and compared in constant time. Only someone who saw the printed URL knows it, and a page on another origin cannot send a custom header without a CORS preflight, which Pendragon never grants. The page takes the token from its URL, keeps it in the tab's `sessionStorage` and removes it from the address bar.
3. **Requests that change something** (anything but `GET` and `HEAD`) must not come from another origin: an `Origin` header, when the browser sends one, must be `http://` and an accepted host. Their bodies must be `application/json`, which a plain HTML form cannot send.

These checks are made on the request line and headers alone, before Pendragon reads the body or makes room for it: a refused request costs its head, whatever `Content-Length` it announces. A body that passes is read into a buffer that grows as the bytes arrive, not one made full size up front.
4. **Output is inert.** The page's script builds every element with the DOM and sets data only as text (`textContent`), never as markup; the JSON encoder also escapes `<`, `>`, `&`, U+2028 and U+2029. Every response has `Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` (no inline script or style at all), `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer` and `Cache-Control: no-store`.

The static page (`/`, `/app.js`, `/style.css`) needs no token: it is the same for everyone and holds no data.

Listening on another interface (`--listen 0.0.0.0`) takes `--allow-host` for the names the page is reached by, and Pendragon warns at startup: the token is then all that stands between the network and the VMs, over plain HTTP.

## 2. HTTP

One request per connection; every response says `Connection: close`. The parser is strict, so that it never guesses differently from another program:

| Request | Answer |
|---|---|
| request line and headers over 16 KiB (every CRLF counted, the blank line that ends them too), or more than 64 header lines | 431 |
| a bare LF, a folded header, whitespace before a header's colon, control or non-ASCII bytes in the head, a target that is not a path or has a fragment, a bad `%` escape | 400 |
| HTTP/1.1 without exactly one `Host` | 400 |
| `Transfer-Encoding` (chunked bodies) | 501 |
| two `Content-Length` headers, or one that is not a number | 400 |
| a body longer than its route allows (64 KiB; 24 MiB for creating a VM, whose program travels in base64) | 413, before any of the body is read |
| `Expect` other than `100-continue` | 417 |
| an HTTP version other than 1.0 and 1.1 | 505 |
| the whole request not in within 30 seconds, however slowly it trickles in | 408 |
| more than 64 connections at once | 503 (answered off the accepting thread, at most 16 at a time; past that the connection is closed) |

One empty line (CRLF) before the request line is ignored, as RFC 9112 section 2.2 asks. After its response, a connection reads and drops what the client still sends for at most a second (and 4 MiB) before it closes, so that closing does not reset the connection before the client has read the response; a client that keeps sending cannot keep the connection longer.

## 3. API

Every route is under `/api/`, takes and returns JSON (`Content-Type: application/json`), and needs the token. Errors are `{"error": "<message for a person>", "code": "<protocol error name, or null>"}`; the status follows the protocol's error code:

| Status | When |
|---|---|
| 400 | the request itself is wrong: bad JSON, a missing or unknown field, a value Round Table would refuse too (`ram: 0 MiB is out of range`) |
| 403 | the checks of section 1 |
| 404 | `NoSuchVm`, `NoSuchDisk`, `NoSuchSwitch`, `NoSuchCard`, an unknown route |
| 405 | a route that does not take that method |
| 409 | `AlreadyExists`, `Busy`, `InvalidState`, `OverBudget`, a detach with nothing to detach, an unplug from a switch the card is not on |
| 422 | `InvalidName`, `InvalidProfile`, `InvalidProgram` |
| 502 | a server that cannot be reached, or another protocol error (`Io`: a gateway's host port that cannot be bound, say) |

A VM in a path (`{vm}`) is its id (all digits) or its name, as on Round Table's command line.

| Route | What |
|---|---|
| `GET /api/overview` | both servers' health (`ok`, `server` name or `error`), VM counts by state, RAM in use (the sum of every VM's RAM, which Avalon holds from create to destroy) against the budget, disk count, attached count and total size, and `consoles`: how many consoles Pendragon has open |
| `GET /api/vms` | every VM: `id`, `name`, `state` (`created`, `running`, `paused`, `stopped`, `exited`), `state_text`, `exit_code`, `ram`, `clock_hz` (null: full speed), `nominal_hz`, their `*_text` forms, `harts`, `disks` (`source`, `read_only`, `lake`: `{addr, name}` or null, `held`: whether the VM still holds it), `nets` (`card` number, `mac`, `switch` it is plugged into or null, as they are now) and the `actions` its state allows |
| `POST /api/vms` | create: `{"name", "ram", "clock", "harts", "disks": [{"source", "read_only"}], "nets": [{"switch", "mac"}], "camelot", "program", "start"}`. `ram`, `clock` and `harts` are text as `--ram`, `--clock` and `--harts` take them (MiB; `10MHz`, `500kHz`, `5Hz`; 1 to 8; empty for the defaults), each disk source a path on Avalon's host or `lake://ADDR/NAME`, each network card's switch a name (empty or null: unplugged) and its `mac` `xx:xx:xx:xx:xx:xx` (empty or null: Avalon makes one up), as `--net SWITCH,mac=...` takes them; the cards come after the disks in the profile. `program` is the ELF file in base64; it may be left out when there is a disk, and the VM boots from the first. `camelot: true` makes a Camelot VM instead, as `round-table create --camelot` does: Lake copies its image `camelot` to `<name>-root` (see below), which goes before the other disks, and the VM boots from it; it takes no `program` (400). 201 `{"id", "name", "started", "disk"}`, `disk` the Camelot VM's copy or null |
| `GET /api/vms/{vm}` | one VM, as in the list, plus `steps` and `devices` (the guest's sysinfo table) |
| `POST /api/vms/{vm}/{action}` | `start`, `pause`, `resume`, `stop` or `destroy` |
| `GET /api/vms/{vm}/console?from=N&tap=T&wait=MS` | console output from byte offset `N` (below) |
| `POST /api/vms/{vm}/console` | `{"data": "text"}`: bytes for the VM's UART |
| `GET /api/disks` | `lake` (Pendragon's Lake), `camelot_image` (the image a Camelot VM's disk is a copy of: `camelot`), and every Lake image: `name`, `sectors`, `bytes`, `size_text`, `readers`, `writer`, `attached`, and `attached_to`: the VMs on Pendragon's Avalon that hold it (`id`, `name`, `read_only`; each VM once, read-only only if every disk of it that names the image is), or null when Avalon cannot be asked (`vms_error` says why) |
| `POST /api/disks` | create: `{"name", "size"}`, the size as `disk create --size` takes it (`64MiB`, `512K`, `1G`, `2048s`). 201 |
| `DELETE /api/disks/{name}` | delete an image nobody has attached (409 `busy` otherwise) |
| `POST /api/disks/{name}/detach` | stop the VMs on Pendragon's Avalon that hold the image (see below); `{"stopped": [{"id", "name"}], "released": bool}` |
| `GET /api/switches` | every switch of Avalon, in name order: `name`, `capture` (the pcap file or null), `nat` (null, or the gateway: `network`, `gateway`, `dns`, `hostfwd`: its rules as bound, `tcp:127.0.0.1:8080-10.0.2.15:80`, and `leases`: `addr`, `mac` and the `vm` (`{id, name}` or null) whose card has that address), and `ports`: `kind` (`card` or `gateway`), `vm` (`{id, name}`, the name null if the VM is not in the list), `card`, `mac`, `in_frames`, `in_bytes`, `out_frames`, `out_bytes`, `dropped`. `vms_error` says why the VMs are unknown, when they are |
| `POST /api/switches` | create: `{"name", "nat": bool, "hostfwd": ["RULE", ...]}`, each rule as `switch create --hostfwd` takes it (`[tcp\|udp]:[HOSTADDR]:HOSTPORT-[GUESTADDR]:GUESTPORT`; the host address defaults to 127.0.0.1, the guest to 10.0.2.15; host port 0 lets the system pick). Forwarding ports needs `nat` (400 otherwise). 201 `{"name", "nat"}`, `nat` as in the list with the ports as bound |
| `DELETE /api/switches/{name}` | delete a switch: its cards are unplugged, its gateway ends and its host ports are closed before the reply |
| `POST /api/switches/{name}/nat` | give the switch a NAT gateway: `{"hostfwd": [...]}`; 409 `already exists` if it has one. `{"name", "nat"}` |
| `DELETE /api/switches/{name}/nat` | take its gateway away (409 `invalid state` if it has none), closing its connections and host ports |
| `POST /api/switches/{name}/attach` | plug a VM's card in: `{"vm": id or name, "card": N}` (card 0 if left out); a card on another switch moves |
| `POST /api/switches/{name}/detach` | unplug it: the same body; 404 for a switch or a card that does not exist, 409 for a card on another switch or none |

**Attaching and detaching disks.** A VM's disks are part of its hardware profile, fixed when it is created; the protocol has no hot-plug (protocol doc, section 6.6). So attaching a disk is creating a VM with it (the disk page's *attach* opens the new-VM form with the image filled in), and a VM lets go of its disks when its machine ends: detaching stops the VMs that hold the image, then waits up to 5 seconds for Lake to show it released (`released`). A stopped VM cannot be started again, which the page says before it detaches. A VM holds a Lake image when it is created, running or paused and its profile names the image on a server that is Pendragon's Lake: the same address, or one that resolves to the same socket address (`localhost:7401` and `127.0.0.1:7401`). An image attached by a client Pendragon does not manage (another Avalon) shows as attached with no VM, and cannot be detached here.

**Camelot VMs.** The new-VM form chooses what the VM boots: *Camelot (boot from disk)* or *Program (ELF)*. Camelot is chosen when Lake has the image `camelot` (which `cargo xtask up` puts there) and the form was not opened to attach a disk; without the image the form says to run `cargo xtask up`. A Camelot VM's create is two steps on two servers, through `avalon_client::create_camelot_vm`, the function Round Table's `create --camelot` uses: Lake copies `camelot` to `<name>-root` (a `DiskCreate` with a source, protocol section 6.6), then Avalon creates the VM with `lake://<Pendragon's Lake>/<name>-root` as disk 0 and no program, so it boots from the disk. Lake allows one writer per image, so every Camelot VM needs a copy of its own, and two of them run side by side. A missing image is 404 (`no such disk`, with the hint), a copy whose name is taken 409 (`already exists`), an image another VM writes 409 (`busy`); when Avalon refuses the VM (its name is taken, the RAM budget), the copy is deleted again and the status is Avalon's. A name too long to leave room for `-root` in a disk name is 400. The request is a change like any other: same token, `Origin`, `Host` and JSON-body checks (section 1).

**Switches.** The switches page lists Avalon's switches with each port's counters, creates one (with a NAT gateway and its forwarded ports, one rule per line), gives a switch a gateway or takes it away, deletes one, and plugs a VM's card in or unplugs it. Every one of these is a change, under the same token, `Origin` and JSON-body checks as the rest (section 1): forwarding a host port opens a socket on Avalon's host, so another site must not be able to ask for one. A forwarded port is bound on 127.0.0.1 unless its rule names another address. The new-VM form adds network cards (a switch and an optional address each).

**Consoles.** A browser cannot speak the Avalon protocol, so Pendragon attaches to a VM's console for it: a *tap* is one protocol session attached to one VM (`ConsoleAttach`), with a thread reading its output into a buffer of the last 64 KiB. Every page showing the VM shares the VM's tap; a tap nobody has read or written for a minute is closed (a thread looks every few seconds, whether or not anyone opens another console), and at most 32 are open (409 `over budget` past that). A read returns `{"vm", "tap", "from", "next", "data", "reset", "ended"}`: the output from offset `from` (counted from the start of the tap, the scrollback Avalon sent on attaching at 0) to `next`, in base64 (`data`), so console bytes that are not UTF-8 arrive as they are. The page asks again from `next`. A read with nothing new waits up to `wait` milliseconds (at most 20 seconds) for output, so text shows up as soon as the guest writes it (long polling). A read with another tap's number (Pendragon restarted, or the tap was closed) starts again from the oldest byte with `reset` set; a `from` that is older than the buffer gets what is left. `ended` says why the session ended (the VM was destroyed, Avalon went away). Input goes to the VM as `ConsoleInput`, in pieces of at most 4 KiB.

The VM's screen in a `<canvas>`, with keyboard and mouse, is for later, on top of Herald's screen work (#40).

## 4. Tests

`crates/pendragon` has unit tests for the HTTP parser (good requests, every malformed shape above, oversize heads and bodies, `100-continue`, a request split into single bytes, a slow client against the deadline), JSON (the grammar's corners, escapes, surrogates, depth, duplicate keys, exact numbers), base64, the random source, the guard (hosts, tokens, origins, `--allow-host`), the request parsing of the API and the command line. `tests/api.rs` runs Avalon and Lake in process and drives every operation over HTTP: disks created, listed and deleted; a VM created on a Lake disk with an uploaded program, Camelot VMs created with no program (`tests/camelot.rs`: two at once on copies of their own, with extra disks, the refusals, and the page's form), listed, inspected by id and by name, taken through its states; the disk shown attached to it, refused for deletion and for a second writer, then detached; read-only images shared by two VMs; the console's output, input, long polling and reset; refusals with their codes; servers that are away; switches created with and without a NAT gateway, a VM with two cards running `examples/net/net-user` whose lease names it, cards unplugged and plugged back, a gateway given and taken away, a taken host port refused, and a deleted switch's forwarded port free again. `tests/security.rs` checks the page's headers and script, wrong `Host`s, missing and wrong tokens (on the switch routes too), foreign origins (one asking for a forwarded port among them) and form bodies, escaped output, malformed, oversize and slow requests, and the connection limit. `tests/cli.rs` runs the binary and uses the URL it prints.
