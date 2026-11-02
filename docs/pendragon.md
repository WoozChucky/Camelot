# Pendragon: the web console

Pendragon (`crates/pendragon`) serves a browser page for the Avalon platform: the VMs of an Avalon server and the disk images of a Lake server. It is a separate program and a client of both servers over the Avalon protocol ([`avalon-protocol.md`](avalon-protocol.md)), like Round Table, and every operation goes through the same client layer (`crates/avalon-client`): the two front ends accept the same syntax (`--ram`, `--clock`, disk sources and sizes) and cannot drift apart. Avalon and Lake stay headless, and Lake may run on another machine.

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

Pendragon prints the URL to open on stdout, alone on its line: `http://127.0.0.1:7402/?token=<64 hex digits>`.

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
| 404 | `NoSuchVm`, `NoSuchDisk`, an unknown route |
| 405 | a route that does not take that method |
| 409 | `AlreadyExists`, `Busy`, `InvalidState`, `OverBudget`, a detach with nothing to detach |
| 422 | `InvalidName`, `InvalidProfile`, `InvalidProgram` |
| 502 | a server that cannot be reached, or another protocol error |

A VM in a path (`{vm}`) is its id (all digits) or its name, as on Round Table's command line.

| Route | What |
|---|---|
| `GET /api/overview` | both servers' health (`ok`, `server` name or `error`), VM counts by state, RAM in use (the sum of every VM's RAM, which Avalon holds from create to destroy) against the budget, disk count, attached count and total size, and `consoles`: how many consoles Pendragon has open |
| `GET /api/vms` | every VM: `id`, `name`, `state` (`created`, `running`, `paused`, `stopped`, `exited`), `state_text`, `exit_code`, `ram`, `clock_hz` (null: full speed), `nominal_hz`, their `*_text` forms, `disks` (`source`, `read_only`, `lake`: `{addr, name}` or null, `held`: whether the VM still holds it) and the `actions` its state allows |
| `POST /api/vms` | create: `{"name", "ram", "clock", "disks": [{"source", "read_only"}], "program", "start"}`. `ram` and `clock` are text as `--ram` and `--clock` take them (MiB; `10MHz`, `500kHz`, `5Hz`; empty for the defaults), each disk source a path on Avalon's host or `lake://ADDR/NAME`, `program` the ELF file in base64. 201 `{"id", "name", "started"}` |
| `GET /api/vms/{vm}` | one VM, as in the list, plus `steps` and `devices` (the guest's sysinfo table) |
| `POST /api/vms/{vm}/{action}` | `start`, `pause`, `resume`, `stop` or `destroy` |
| `GET /api/vms/{vm}/console?from=N&tap=T&wait=MS` | console output from byte offset `N` (below) |
| `POST /api/vms/{vm}/console` | `{"data": "text"}`: bytes for the VM's UART |
| `GET /api/disks` | every Lake image: `name`, `sectors`, `bytes`, `size_text`, `readers`, `writer`, `attached`, and `attached_to`: the VMs on Pendragon's Avalon that hold it (`id`, `name`, `read_only`; each VM once, read-only only if every disk of it that names the image is), or null when Avalon cannot be asked (`vms_error` says why) |
| `POST /api/disks` | create: `{"name", "size"}`, the size as `disk create --size` takes it (`64MiB`, `512K`, `1G`, `2048s`). 201 |
| `DELETE /api/disks/{name}` | delete an image nobody has attached (409 `busy` otherwise) |
| `POST /api/disks/{name}/detach` | stop the VMs on Pendragon's Avalon that hold the image (see below); `{"stopped": [{"id", "name"}], "released": bool}` |

**Attaching and detaching disks.** A VM's disks are part of its hardware profile, fixed when it is created; the protocol has no hot-plug (protocol doc, section 6.6). So attaching a disk is creating a VM with it (the disk page's *attach* opens the new-VM form with the image filled in), and a VM lets go of its disks when its machine ends: detaching stops the VMs that hold the image, then waits up to 5 seconds for Lake to show it released (`released`). A stopped VM cannot be started again, which the page says before it detaches. A VM holds a Lake image when it is created, running or paused and its profile names the image on a server that is Pendragon's Lake: the same address, or one that resolves to the same socket address (`localhost:7401` and `127.0.0.1:7401`). An image attached by a client Pendragon does not manage (another Avalon) shows as attached with no VM, and cannot be detached here.

**Consoles.** A browser cannot speak the Avalon protocol, so Pendragon attaches to a VM's console for it: a *tap* is one protocol session attached to one VM (`ConsoleAttach`), with a thread reading its output into a buffer of the last 64 KiB. Every page showing the VM shares the VM's tap; a tap nobody has read or written for a minute is closed (a thread looks every few seconds, whether or not anyone opens another console), and at most 32 are open (409 `over budget` past that). A read returns `{"vm", "tap", "from", "next", "data", "reset", "ended"}`: the output from offset `from` (counted from the start of the tap, the scrollback Avalon sent on attaching at 0) to `next`, in base64 (`data`), so console bytes that are not UTF-8 arrive as they are. The page asks again from `next`. A read with nothing new waits up to `wait` milliseconds (at most 20 seconds) for output, so text shows up as soon as the guest writes it (long polling). A read with another tap's number (Pendragon restarted, or the tap was closed) starts again from the oldest byte with `reset` set; a `from` that is older than the buffer gets what is left. `ended` says why the session ended (the VM was destroyed, Avalon went away). Input goes to the VM as `ConsoleInput`, in pieces of at most 4 KiB.

The VM's screen in a `<canvas>`, with keyboard and mouse, is for later, on top of Herald's screen work (#40).

## 4. Tests

`crates/pendragon` has unit tests for the HTTP parser (good requests, every malformed shape above, oversize heads and bodies, `100-continue`, a request split into single bytes, a slow client against the deadline), JSON (the grammar's corners, escapes, surrogates, depth, duplicate keys, exact numbers), base64, the random source, the guard (hosts, tokens, origins, `--allow-host`), the request parsing of the API and the command line. `tests/api.rs` runs Avalon and Lake in process and drives every operation over HTTP: disks created, listed and deleted; a VM created on a Lake disk with an uploaded program, listed, inspected by id and by name, taken through its states; the disk shown attached to it, refused for deletion and for a second writer, then detached; read-only images shared by two VMs; the console's output, input, long polling and reset; refusals with their codes; servers that are away. `tests/security.rs` checks the page's headers and script, wrong `Host`s, missing and wrong tokens, foreign origins and form bodies, escaped output, malformed, oversize and slow requests, and the connection limit. `tests/cli.rs` runs the binary and uses the URL it prints.
