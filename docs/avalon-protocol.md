# Avalon Protocol v1

The wire protocol of the Avalon platform. Every program in it speaks this protocol:

| Program | Role | Serves / uses |
|---|---|---|
| **Avalon** | hypervisor server: runs N Grail VMs | serves session, VM control, console, screen and input |
| **Round Table** | control CLI | uses VM control (and disk management against Lake) |
| **Herald** | attaches to one VM | uses console, screen and input |
| **Lake** | storage server: serves disk images | serves session, block I/O, disk management |

The implementation is the `avalon-proto` crate (`crates/avalon-proto`): the messages as Rust types, `encode`/`decode`, and a blocking `Connection` over any `Read + Write`. This document and the crate must agree; `crates/avalon-proto/tests/spec.rs` checks the type codes, the error codes and one layout of every message byte for byte against the tables below.

## 1. Transport

- TCP, through `std::net`. One connection is one session; either side ends it by closing the socket.
- Servers listen on loopback by default: Avalon on `127.0.0.1:7400`, Lake on `127.0.0.1:7401`. Listening on any other interface takes an explicit command-line flag.
- **No authentication and no encryption in v1.** Anyone who can reach the port can create, control and read VMs and disks. That is why servers bind to loopback unless told otherwise. Authentication and TLS will be a later protocol version (and std-only, like everything else).
- A client and a server are both just byte streams to the protocol; tests run it over in-memory buffers as well as sockets.

## 2. Framing

Every message travels in one frame:

```
+----------------+----------------+--------------------------+
| length: u32 LE | type: u16 LE   | payload: length - 2 bytes |
+----------------+----------------+--------------------------+
```

- `length` counts the type and the payload, not itself. It is at least 2 and at most **`MAX_FRAME_LEN` = 16 MiB** (16 777 216).
- `type` is the message type code (section 6).
- The payload is binary, in the layout of that message type. Console bytes, pixels and disk blocks are carried as they are: no escaping, no text encoding.
- The 6 bytes before the payload are the header (`HEADER_LEN`).

The maximum frame size bounds what a peer can make the other side allocate: a receiver checks the length before reading the body and never reserves more than the length says for it. Decoding the payload then takes memory in proportion to the payload, never to a count written in it (section 3). For scale: a full 640×480 screen is 1 228 800 bytes, a block request at most 1 MiB, and the rest is room for program images.

Receiving:

| Situation | Result | Connection afterwards |
|---|---|---|
| `length` > 16 MiB | `FrameTooLarge`; the body is not read | unusable: close it |
| `length` < 2 | `FrameTooShort`; those 0 or 1 bytes are consumed | in step |
| clean end of stream before a frame | `Closed` | ended |
| end of stream inside a frame | I/O error (`UnexpectedEof`) | ended |
| unknown `type` | `UnknownType`; the frame is consumed | in step |
| payload that does not decode (section 3) | `Truncated`, `TrailingBytes` or `Invalid`; the frame is consumed | in step |

Because a whole frame is read before its payload is decoded, a bad payload never desynchronises the stream: a server answers it with an `Error` reply (`Malformed` or `UnknownMessage`), unless its type is an event (section 4), and carries on. Only an oversize length, a broken transport or a failed handshake ends a connection.

Sending: a message that would make a frame larger than 16 MiB, or that breaks a rule of section 3, is refused by the encoder (nothing is written). A frame is written with a single write, then flushed.

## 3. Field types

Payloads are sequences of fields, in the order the message tables list them, with no padding or alignment.

| Type | Encoding |
|---|---|
| `u8`, `u16`, `u32`, `u64` | little-endian |
| `bool` | `u8`: 0 = false, 1 = true; anything else is invalid |
| `bytes` | `u32` length, then that many bytes |
| `string` | `bytes` holding UTF-8; invalid UTF-8 is invalid |
| `option<T>` | `u8` tag: 0 = none, 1 = followed by a `T`; other tags are invalid |
| `list<T>` | `u32` count, then that many `T`s |
| enums | a `u8` tag, then the fields of that variant (tables below); unknown tags are invalid |

Decoding is strict:

- A payload that ends before its message does is **truncated**. Lengths and counts are checked against the bytes left before anything is read. An element of a list can take many times more memory than its bytes on the wire, so a count never sizes a memory reservation on its own (the crate reserves at most 1024 elements, and no more bytes than are left, before decoding them): a forged count of 4 billion costs nothing.
- Bytes left over after the message are **trailing bytes**, an error.
- With these rules the encoding is canonical: each message has exactly one encoding, and decoding then re-encoding gives back the same bytes (the fuzz tests check this).

Rules beyond the field types, enforced by both the encoder and the decoder (`Message::check`):

- `ScreenFrame`: `width` and `height` non-zero, `pixels` exactly `width × height × 4` bytes.
- `KeyInput`: key code not 0; flag bits other than 0-2 clear.
- `BlockRead`: `count` from 1 to `MAX_BLOCK_SECTORS` (2048).
- `BlockData`, `BlockWrite`: `data` a non-zero whole number of 512-byte sectors, at most 2048 sectors (1 MiB).
- `Hello`: the payload starts with the magic `"AVLN"`.
- `Error`: the crate's `ErrorCode::Other(n)` stands for a code this version has no name for, so it may not carry a named code's number (it would decode as the named code).

Other values (names, hardware profiles, ids, sector numbers) are checked by the server, which answers with a precise error code (section 5) rather than `Malformed`.

### Shared structures

**VmRef**, which VM a request means:

| Tag | Variant | Fields |
|---|---|---|
| 0 | by id | `id: u32` |
| 1 | by name | `name: string` |

Round Table and Herald take a VM argument that is all digits as an id, anything else as a name. Names are never all digits (see **Names**), so every VM can be named either way.

**VmState**:

| Tag | State | Fields |
|---|---|---|
| 0 | created (loaded, never started) | |
| 1 | running | |
| 2 | paused | |
| 3 | stopped (by a client) | |
| 4 | exited (the program stopped the machine) | `code: u32` |

**HardwareProfile**, the hardware of one VM. It mirrors Grail's `MachineConfig` field for field (`crates/avalon-proto/tests/machine_config.rs` stops compiling if `MachineConfig` gains a field the profile lacks):

| Field | Type | Meaning |
|---|---|---|
| `ram_size` | `u64` | RAM in bytes: a non-zero multiple of 4 KiB, at most 2 GiB (Excalibur spec section 7) |
| `clock_hz` | `option<u32>` | real-time clock in instructions per second; none = run at full speed |
| `nominal_hz` | `u32` | clock reported to the guest when `clock_hz` is none (Grail's default: 10 000 000) |
| `devices` | `list<DeviceSpec>` | optional hardware beyond the fixed machine, in the order the guest should see it |

The guest reads these values from its sysinfo block (Excalibur spec section 9.5). The wire accepts any values; the server validates them as Grail does and answers `InvalidProfile`.

**DeviceSpec**:

| Tag | Device | Fields |
|---|---|---|
| 1 | block storage disk | `source: DiskSource`, `read_only: bool` |

**DiskSource**:

| Tag | Source | Fields |
|---|---|---|
| 1 | image file on the Avalon host | `path: string` |
| 2 | image on a Lake server (`lake://addr/name` on the command line) | `addr: string` (`host:port`), `name: string` |

On a command line a disk is one argument (the crate's `DiskSource::parse`): `lake://addr/name` is image `name` on the Lake server at `addr`, which is `host:port`, or a host alone for Lake's default port 7401 (IPv6 addresses in brackets: `lake://[::1]:7401/root`); `name` must be a valid disk name (see **Names**). Anything else is the path of an image file.

**VmSummary**: `id: u32`, `name: string`, `state: VmState`, `profile: HardwareProfile`.

**DeviceEntry**, one entry of the guest's sysinfo device table (Excalibur spec section 9.5): `kind: u32` (the device type code), `base: u32`, `size: u32`, `irq: u32` (PLIC source, 0 = none). The type code is passed through raw, so new device types need no protocol change.

**KeyEvent**, a key for the guest's keyboard device (Excalibur spec section 9.4): `code: u8` (a key code from the spec's table, never 0), `flags: u8` (bit 0 release, bit 1 Shift, bit 2 Ctrl; other bits must be 0).

**DiskInfo**: `name: string`, `sectors: u64` (512-byte sectors), `readers: u32` (read-only attachments now), `writer: bool` (a writer has it attached).

**Names**: VM and disk names (`validate_name`) are 1 to 64 bytes of lowercase ASCII letters, digits, `.`, `_` and `-`, and

- do not start with `.` or `-`, and do not end with `.`;
- are not all digits (an all-digit argument is a VM id);
- are not a Windows device name, alone or before a `.`: `con`, `prn`, `aux`, `nul`, `com0` to `com9`, `lpt0` to `lpt9` (so `nul` and `com1.img` are refused).

Disk names are file names on Lake's host, and the rule makes each one a plain file of its own on Windows too: no path, no device, and no two names reaching the same file (Windows ignores case and trailing dots, so `disk`, `Disk` and `disk.` would all be one file). Names compare byte for byte. A request with any other name gets `InvalidName`.

## 4. Sessions, requests, replies and events

### Handshake

The first frame on a connection is the client's `Hello`; the server answers `Welcome` with its own version:

```
client                                server
  Hello { magic "AVLN", version, agent } -->
                         <-- Welcome { version, server }
```

- If the versions are equal, the session is open.
- If not, the server still sends `Welcome` (so the client learns which version it speaks) and then closes the connection. The client sees the different version in `Welcome` and reports a version mismatch.
- A first frame that is not a valid `Hello` (wrong magic, another message, garbage) gets an `Error` reply (`Malformed` or `Unexpected`) and the connection is closed. A stray HTTP client's `"GET "` reads as a 540 MB length and is refused before anything is read.

`agent` and `server` are free-form names for logs (`"round-table"`, `"herald"`, `"avalon 0.1.0"`).

### Message kinds

Every message type is one of three kinds (the catalogue in section 6 says which):

- **Request**, client to server. The server answers every request with exactly one **reply**, in the order the requests arrived, so a client may send several requests before reading the replies.
- **Reply**, server to client: `Ok` (success, nothing to return), `Error` (section 5), or a request-specific result (`VmCreated`, `VmList`, ...).
- **Event**, never answered: console and screen streams (`ConsoleOutput`, `ScreenFrame`), client input (`ConsoleInput`, `KeyInput`), and notifications (`VmStateChanged`, `Detached`). Server events can arrive at any time between replies, so a client reading a reply sets aside the events it reads first (the crate's `Connection::request` queues them for `recv`).

There are no request ids: replies match requests by order. So that order is never lost, these rules hold even for bad frames:

- **Events are never answered**, not even with `Error`. A server drops an event it cannot decode, an event that travels in the wrong direction (a client sending `ConsoleOutput`) and an event that names something that does not exist (input to a VM the client is not attached to); it may log them.
- **Every other frame a client sends gets exactly one reply**: a request, a reply (answered by `Error(Unexpected)`), a frame of unknown type (`Error(UnknownMessage)`) and a request that fails to decode (`Error(Malformed)`).
- A client waiting for a reply sets aside the events it reads, including those that fail to decode. A reply-type frame is the reply, even when it fails to decode. Any other frame (a request, an unknown type, a frame shorter than its type) cannot be placed: the client no longer knows which reply belongs to which request and closes the connection (the crate's `Error::OutOfStep`).

## 5. Error model

A failed request is answered by:

**Error** (type 0x0003): `code: u16`, `message: string` (human-readable, for showing to a person; clients act on the code).

| Code | Name | Meaning |
|---|---|---|
| 1 | `Malformed` | the frame or payload could not be decoded (section 3), including a too-short or too-long frame and a bad `Hello` magic |
| 2 | `UnknownMessage` | the type code is not in the catalogue |
| 3 | `Unexpected` | a valid message that is not allowed here: anything but `Hello` before the handshake, a reply sent by a client |
| 4 | `Unsupported` | this server does not serve that message group (Lake asked to create a VM, Avalon asked for a block) |
| 5 | `NoSuchVm` | no VM with that id or name |
| 6 | `NoSuchDisk` | no disk image with that name |
| 7 | `NoSuchHandle` | unknown or closed block handle |
| 8 | `AlreadyExists` | a VM or disk with that name already exists |
| 9 | `InvalidName` | the name breaks the naming rules (section 3) |
| 10 | `InvalidProfile` | the hardware profile is not a machine Grail can build (RAM size, zero clock, bad device) |
| 11 | `InvalidProgram` | the program image is not a loadable Excalibur ELF |
| 12 | `InvalidState` | the VM is in the wrong state for the request (pausing a stopped VM, starting a running one) |
| 13 | `OverBudget` | the request would exceed a server limit (Avalon's total RAM budget) |
| 14 | `Busy` | the disk is attached in a way that forbids the request: a second writer, a writer while readers are attached, deleting an attached image |
| 15 | `ReadOnly` | a write through a read-only attachment |
| 16 | `OutOfRange` | a sector range past the end of the disk |
| 17 | `Io` | the server's host I/O failed |
| 18 | `Internal` | a server bug or unexpected condition |

Codes are never reused. A client shows an unknown code as `error N`. A server never crashes on a bad request: everything a client can send maps to one of these codes.

## 6. Message catalogue

Codes are grouped by their high byte. Req = request, Rep = reply, Ev = event; C→S is client to server.

| Code | Message | Kind | Direction | Answered by |
|---|---|---|---|---|
| **0x00xx** | **session** | | | |
| 0x0001 | `Hello` | Req | C→S | `Welcome` |
| 0x0002 | `Welcome` | Rep | S→C | |
| 0x0003 | `Error` | Rep | S→C | |
| 0x0004 | `Ok` | Rep | S→C | |
| 0x0005 | `Ping` | Req | C→S | `Pong` |
| 0x0006 | `Pong` | Rep | S→C | |
| **0x01xx** | **VM control** (Avalon) | | | |
| 0x0101 | `CreateVm` | Req | C→S | `VmCreated` |
| 0x0102 | `VmCreated` | Rep | S→C | |
| 0x0103 | `ListVms` | Req | C→S | `VmList` |
| 0x0104 | `VmList` | Rep | S→C | |
| 0x0105 | `GetVmInfo` | Req | C→S | `VmInfo` |
| 0x0106 | `VmInfo` | Rep | S→C | |
| 0x0107 | `StartVm` | Req | C→S | `Ok` |
| 0x0108 | `PauseVm` | Req | C→S | `Ok` |
| 0x0109 | `ResumeVm` | Req | C→S | `Ok` |
| 0x010A | `StopVm` | Req | C→S | `Ok` |
| 0x010B | `DestroyVm` | Req | C→S | `Ok` |
| 0x010C | `VmStateChanged` | Ev | S→C | |
| 0x010D | `Detached` | Ev | S→C | |
| **0x02xx** | **console** (Avalon, Herald) | | | |
| 0x0201 | `ConsoleAttach` | Req | C→S | `ConsoleAttached` |
| 0x0202 | `ConsoleAttached` | Rep | S→C | |
| 0x0203 | `ConsoleInput` | Ev | C→S | |
| 0x0204 | `ConsoleOutput` | Ev | S→C | |
| 0x0205 | `ConsoleDetach` | Req | C→S | `Ok` |
| **0x03xx** | **screen and input** (Avalon, Herald) | | | |
| 0x0301 | `ScreenAttach` | Req | C→S | `ScreenAttached` |
| 0x0302 | `ScreenAttached` | Rep | S→C | |
| 0x0303 | `ScreenFrame` | Ev | S→C | |
| 0x0304 | `KeyInput` | Ev | C→S | |
| 0x0305 | `ScreenDetach` | Req | C→S | `Ok` |
| **0x04xx** | **block I/O** (Lake) | | | |
| 0x0401 | `BlockOpen` | Req | C→S | `BlockOpened` |
| 0x0402 | `BlockOpened` | Rep | S→C | |
| 0x0403 | `BlockRead` | Req | C→S | `BlockData` |
| 0x0404 | `BlockData` | Rep | S→C | |
| 0x0405 | `BlockWrite` | Req | C→S | `Ok` |
| 0x0406 | `BlockFlush` | Req | C→S | `Ok` |
| 0x0407 | `BlockSize` | Req | C→S | `BlockCapacity` |
| 0x0408 | `BlockCapacity` | Rep | S→C | |
| 0x0409 | `BlockClose` | Req | C→S | `Ok` |
| **0x05xx** | **disk management** (Lake) | | | |
| 0x0501 | `DiskCreate` | Req | C→S | `Ok` |
| 0x0502 | `ListDisks` | Req | C→S | `DiskList` |
| 0x0503 | `DiskList` | Rep | S→C | |
| 0x0504 | `DiskDelete` | Req | C→S | `Ok` |

Any request can also be answered by `Error`. A server answers requests from a group it does not serve with `Error(Unsupported)`.

### 6.1 Session

| Message | Payload |
|---|---|
| `Hello` | `magic: 4 bytes` = `"AVLN"`, `version: u16`, `agent: string` |
| `Welcome` | `version: u16`, `server: string` |
| `Error` | `code: u16`, `message: string` |
| `Ok` | empty |
| `Ping` | `token: u32` |
| `Pong` | `token: u32` (the `Ping`'s) |

Example, `Hello` version 1 from agent `"rt"`:

```
0e 00 00 00  01 00  41 56 4c 4e  01 00  02 00 00 00  72 74
length 14    Hello  "AVLN"       v1     agent "rt"
```

### 6.2 VM control

| Message | Payload | Notes |
|---|---|---|
| `CreateVm` | `name: string`, `profile: HardwareProfile`, `program: bytes` | `program` is an Excalibur ELF executable. The VM is loaded and left in state created. Errors: `InvalidName`, `AlreadyExists`, `InvalidProfile` (also a disk that cannot be opened or attached, see below), `InvalidProgram`, `OverBudget` |
| `VmCreated` | `id: u32` | ids are never reused while the server runs |
| `ListVms` | empty | |
| `VmList` | `vms: list<VmSummary>` | in id order |
| `GetVmInfo` | `vm: VmRef` | |
| `VmInfo` | `vm: VmSummary`, `devices: list<DeviceEntry>`, `steps: u64` | `devices` is the guest's sysinfo device table; `steps` the instructions executed so far |
| `StartVm` | `vm: VmRef` | created → running |
| `PauseVm` | `vm: VmRef` | running → paused |
| `ResumeVm` | `vm: VmRef` | paused → running |
| `StopVm` | `vm: VmRef` | created, running or paused → stopped. A stopped or exited VM cannot be restarted in v1; destroy it and create a new one |
| `DestroyVm` | `vm: VmRef` | any state; frees the VM, detaches its clients (`Detached`), and its name can be reused |
| `VmStateChanged` | `vm: u32`, `state: VmState` | sent to every client attached to the VM (console or screen) when its state changes, including when its program exits |
| `Detached` | `vm: u32`, `kind: u8` (1 console, 2 screen) | the server ended an attachment, because the VM was destroyed. Not sent in answer to `ConsoleDetach`/`ScreenDetach` |

A request for a transition from the wrong state gets `InvalidState`.

**Disks on a Lake server.** Avalon builds a VM's machine when the VM is created, and attaches each Lake disk of its profile then: it connects to the Lake server (an Avalon protocol client like any other, agent `grail <version>`) and sends `BlockOpen`, read-only if the profile says so (section 6.5). A disk Lake will not attach (no such disk, `Busy` because another VM writes it, a server that cannot be reached) fails the create with `InvalidProfile`, whose message carries Lake's error. The VM keeps its attachment as long as its machine exists, and releases it (`BlockClose`) when the machine goes: when the VM is destroyed (the image is free once `DestroyVm` is answered), and also when it is stopped or its program exits (shortly after the state change). The guest's reads, writes and flushes become `BlockRead`, `BlockWrite` and `BlockFlush` requests, answered before the guest runs on; any failure, Lake's error replies included, is `BLK_ERROR` 5 (I/O) for the guest. A Lake server that closes the connection or does not answer within 10 seconds fails that request and every later one on that disk.

Example, `StartVm` by id 5:

```
07 00 00 00  07 01  00  05 00 00 00
length 7     0x0107 id  5
```

### 6.3 Console

The console is the VM's UART (Excalibur spec section 7).

| Message | Payload | Notes |
|---|---|---|
| `ConsoleAttach` | `vm: VmRef` | |
| `ConsoleAttached` | `vm: u32`, `scrollback: bytes` | the VM's id and its recent output (Avalon keeps a ring buffer of the last 64 KiB per VM); `ConsoleOutput` events follow, starting right after the scrollback. Attaching again resends the scrollback; the stream is not doubled |
| `ConsoleInput` | `vm: u32`, `data: bytes` | bytes for the UART's receiver |
| `ConsoleOutput` | `vm: u32`, `data: bytes` | bytes the VM wrote to its UART |
| `ConsoleDetach` | `vm: u32` | the VM keeps running. Errors: `NoSuchVm`, `InvalidState` (this connection is not attached to it) |

Several clients may attach to one VM's console: all of them get the output, and input from any of them reaches the UART, in the order the server receives it. One connection may attach to several VMs; the `vm` field says which stream a frame belongs to.

### 6.4 Screen and input

The screen is the VM's framebuffer (Excalibur spec section 9.3), the input its keyboard device (section 9.4).

| Message | Payload | Notes |
|---|---|---|
| `ScreenAttach` | `vm: VmRef` | |
| `ScreenAttached` | `vm: u32`, `width: u32`, `height: u32`, `format: u32` | Grail: 640, 480, format 0. A full `ScreenFrame` follows, then a frame whenever the screen changes |
| `ScreenFrame` | `vm: u32`, `x: u32`, `y: u32`, `width: u32`, `height: u32`, `pixels: bytes` | the rectangle at (`x`, `y`), rows top to bottom, `width × height × 4` bytes in the attachment's format |
| `KeyInput` | `vm: u32`, `key: KeyEvent` | delivered to the VM's keyboard device as if typed into Grail's window |
| `ScreenDetach` | `vm: u32` | |

Format 0 is XRGB8888: each pixel a little-endian word `0x00RRGGBB` (bytes B, G, R, X), the same as Grail's VRAM. An unchanged screen sends no frames. v1 servers may send only full frames; the rectangle fields let later ones send just what changed without a protocol change.

### 6.5 Block I/O

Block I/O gives a client (Grail's network disk backend, through Avalon) sector access to one of Lake's images. Sectors are 512 bytes (`SECTOR_SIZE`).

| Message | Payload | Notes |
|---|---|---|
| `BlockOpen` | `name: string`, `read_only: bool` | attach an image. Errors: `InvalidName`, `NoSuchDisk`, `Busy`, `OverBudget` (Lake: more than 64 attachments on one connection) |
| `BlockOpened` | `handle: u32`, `sectors: u64`, `read_only: bool` | the handle names this attachment in the requests below, on this connection only. Handles are never reused on a connection; `read_only` is the request's |
| `BlockRead` | `handle: u32`, `sector: u64`, `count: u32` | `count` 1 to 2048 sectors. Errors: `NoSuchHandle`, `OutOfRange` |
| `BlockData` | `handle: u32`, `sector: u64`, `data: bytes` | `count × 512` bytes |
| `BlockWrite` | `handle: u32`, `sector: u64`, `data: bytes` | whole sectors, 1 to 2048 of them. Errors: `NoSuchHandle`, `ReadOnly`, `OutOfRange` |
| `BlockFlush` | `handle: u32` | answered once earlier writes are durable on the server's host. Succeeds on a read-only attachment |
| `BlockSize` | `handle: u32` | |
| `BlockCapacity` | `handle: u32`, `sectors: u64` | the image's size |
| `BlockClose` | `handle: u32` | detach. Closing the connection closes all its handles |

Concurrency: an image has at most one writer at a time, and a writer excludes readers (exclusive attach); any number of read-only attachments may share an image that has no writer. An attach that breaks this gets `Busy`, and so does a second attach of an image by the connection that writes it. A range that runs past the end of the image (`sector + count > sectors`, computed without overflow) gets `OutOfRange` and changes nothing. Checks come in the order handle, read-only, range. A write is visible to every later attachment once it is answered (a later reader, the next VM); `BlockFlush` is only about surviving a crash of the server's host.

### 6.6 Disk management

| Message | Payload | Notes |
|---|---|---|
| `DiskCreate` | `name: string`, `sectors: u64` | a new zero-filled image of 1 to the server's maximum sectors (Lake: 2^32 - 1, the most a Grail disk can address). Errors: `InvalidName`, `AlreadyExists`, `OverBudget` (0 sectors, or too many), `Io` |
| `ListDisks` | empty | |
| `DiskList` | `disks: list<DiskInfo>` | in name order |
| `DiskDelete` | `name: string` | Errors: `InvalidName`, `NoSuchDisk`, `Busy` (attached) |

Lake keeps each disk as a raw image file named after the disk, in the directory it serves (`lake --dir`): sector `n` is bytes 512·`n` to 512·`n` + 511, as in Grail's image files, so an image can be copied in and out of Lake's directory. A file there whose name is not a valid disk name, or whose size is not a whole number of sectors, is not a disk: it is not listed and cannot be attached or deleted, and its name cannot be used for a new disk (`AlreadyExists`). Disks are matched by their exact file name: where the filesystem ignores case (Windows, macOS), a file `ROOT` is not disk `root` (`NoSuchDisk`), though it still keeps the name `root` from being used for a new disk. Images never change size through the protocol.

A new image is a sparse file: it takes no space on the host until it is written. On Windows Lake marks it sparse explicitly (NTFS does not otherwise, and the first write near the end of a large non-sparse file would make the filesystem write zeros over everything before it, inside that one write, for longer than a Grail disk waits for an answer). On a filesystem without sparse files (FAT, exFAT), `DiskCreate` writes the zeros itself, so creating a large image there takes a while, but no later write does.

## 7. Versioning

- The protocol version is a `u16`, currently **1** (`PROTOCOL_VERSION`). Both sides must speak the same version; there is no negotiation.
- Any change to the catalogue bumps the version: a new message type, a new field, a new enum variant or tag, a changed layout or a changed rule. (Unknown codes and tags are errors in the decoder, so an old peer could not read them anyway.)
- Three things never change, in any version, so that peers of different versions can always tell each other so: the frame header (`u32` length, `u16` type), the `Hello` and `Welcome` messages (codes 0x0001 and 0x0002 and their layouts) and the `Error` message (code 0x0003 and its layout). Error codes are never reused or renumbered; new ones may be added with a version bump.
- Things that are deliberately open-ended and need no new version: `DeviceEntry.kind` (sysinfo device type codes come from the Excalibur spec), `ScreenAttached.format` values a server chooses to send, free-text fields.
- This document describes version 1. When the version changes, the change is described here and the crate's round-trip, layout and fuzz tests are updated with it.

## 8. Limits

| Constant | Value | Where |
|---|---|---|
| `MAX_FRAME_LEN` | 16 MiB | largest `length` field |
| `MAX_BLOCK_SECTORS` | 2048 (1 MiB) | per `BlockRead`/`BlockWrite` |
| `SECTOR_SIZE` | 512 | block I/O |
| `MAX_NAME_LEN` | 64 | VM and disk names |
| default Avalon address | `127.0.0.1:7400` | |
| default Lake address | `127.0.0.1:7401` | |

Servers may set lower limits of their own (a RAM budget, a maximum image size) and answer with `OverBudget`. A server may also close the connection of a client that does not read what is sent to it, rather than hold unbounded output for it: Avalon does once 8 MiB of output (events and replies) are waiting for one client.
