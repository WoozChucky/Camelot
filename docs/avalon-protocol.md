# Avalon Protocol v5

The wire protocol of the Avalon platform. Every program in it speaks this protocol:

| Program | Role | Serves / uses |
|---|---|---|
| **Avalon** | hypervisor server: runs N Grail VMs | serves session, VM control, console, screen and input, virtual network |
| **Round Table** | control CLI | uses VM control and virtual network (and disk management against Lake) |
| **Pendragon** | web console ([`pendragon.md`](pendragon.md)) | uses VM control, console and virtual network (Avalon), disk management (Lake) |
| **Herald** | attaches to one VM | uses console, screen and input |
| **Lake** | storage server: serves disk images | serves session, block I/O, disk management |

The implementation is the `avalon-proto` crate (`crates/avalon-proto`): the messages as Rust types, `encode`/`decode`, and a blocking `Connection` over any `Read + Write`. This document and the crate must agree; `crates/avalon-proto/tests/spec.rs` checks the type codes, the error codes and one layout of every message byte for byte against the tables below.

## 1. Transport

- TCP, through `std::net`. One connection is one session; either side ends it by closing the socket.
- Servers listen on loopback by default: Avalon on `127.0.0.1:7400`, Lake on `127.0.0.1:7401`. Listening on any other interface takes an explicit command-line flag.
- **No authentication and no encryption** (versions 1 to 5). Anyone who can reach the port can create, control and read VMs and disks. That is why servers bind to loopback unless told otherwise. Authentication and TLS will be a later protocol version (and std-only, like everything else).
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
| `i8`, `i32` | two's complement, little-endian |
| `bool` | `u8`: 0 = false, 1 = true; anything else is invalid |
| `bytes` | `u32` length, then that many bytes |
| `mac` | 6 bytes, a MAC address in wire order (`02 ca 00 00 00 01` is `02:ca:00:00:00:01`) |
| `ipv4` | 4 bytes, an IPv4 address in wire order (`0a 00 02 0f` is 10.0.2.15) |
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
- `PointerInput`: button bits other than 0-2 clear.
- `BlockRead`: `count` from 1 to `MAX_BLOCK_SECTORS` (2048).
- `BlockData`, `BlockWrite`: `data` a non-zero whole number of 512-byte sectors, at most 2048 sectors (1 MiB).
- `Hello`: the payload starts with the magic `"AVLN"`.
- `Error`: the crate's `ErrorCode::Other(n)` stands for a code this version has no name for, so it may not carry a named code's number (it would decode as the named code).
- `PortForward` (in `CreateSwitch`, `SetSwitchNat` and `SwitchList`): `guest_port` not 0, and `guest_addr` a guest address: in 10.0.2.0/24, and not .0, .2 (the gateway), .3 (the DNS server) or .255.

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
| `harts` | `u32` | number of harts, 1 to 8 (Excalibur spec section 7, "Harts"; Grail's default: 1). The clock is per hart |
| `devices` | `list<DeviceSpec>` | optional hardware beyond the fixed machine, in the order the guest should see it |

The guest reads these values from its sysinfo block (Excalibur spec section 9.5). The wire accepts any values; the server validates them as Grail does and answers `InvalidProfile`.

**DeviceSpec**:

| Tag | Device | Fields |
|---|---|---|
| 1 | block storage disk | `source: DiskSource`, `read_only: bool` |
| 2 | network card (Excalibur spec section 9.8) | `switch: option<string>`, `mac: option<mac>` |

A network card's cable is plugged into one of the server's virtual switches (section 6.7). In `CreateVm`, `switch` is the switch to plug it into when the VM is created (none: unplugged, its link down) and `mac` its address (none: the server makes one up, section 6.7). In `VmList` and `VmInfo` both are what they are at that moment: the switch the card is plugged into now (none once it has been unplugged, or once its VM's machine has ended), and the address it has, made-up or not. A VM has at most 4 cards (Grail's `MAX_NETS`); they are numbered from 0 in the order the profile lists them, among its cards only (disks do not count). A card's address must be an individual (not group), locally administered one (bit 1 of the first byte set, as in `02:..`). More cards, or another address, get `InvalidProfile`.

**DiskSource**:

| Tag | Source | Fields |
|---|---|---|
| 1 | image file on the Avalon host | `path: string` |
| 2 | image on a Lake server (`lake://addr/name` on the command line) | `addr: string` (`host:port`), `name: string` |

On a command line a disk is one argument (the crate's `DiskSource::parse`): `lake://addr/name` is image `name` on the Lake server at `addr`, which is `host:port`, or a host alone for Lake's default port 7401 (IPv6 addresses in brackets: `lake://[::1]:7401/root`); `name` must be a valid disk name (see **Names**). Anything else is the path of an image file.

**VmSummary**: `id: u32`, `name: string`, `state: VmState`, `profile: HardwareProfile`.

**DeviceEntry**, one entry of the guest's sysinfo device table (Excalibur spec section 9.5): `kind: u32` (the device type code), `base: u32`, `size: u32`, `irq: u32` (PLIC source, 0 = none). The type code is passed through raw, so new device types need no protocol change.

**KeyEvent**, a key for the guest's keyboard device (Excalibur spec section 9.4): `code: u8` (a key code from the spec's table, never 0), `flags: u8` (bit 0 release, bit 1 Shift, bit 2 Ctrl; other bits must be 0).

**PointerChange**, one change of a viewer's pointer for the guest's pointer device (Excalibur spec section 9.7): `motion: PointerMotion`, `buttons: u8` (the buttons held after the change: bit 0 left, bit 1 right, bit 2 middle, as in the device's `PTR_BUTTONS`; other bits must be 0), `wheel: i8` (notches turned, positive away from the user).

**PointerMotion**:

| Tag | Motion | Fields |
|---|---|---|
| 0 | none (a button or wheel change only) | |
| 1 | to a pixel of the screen (absolute, like a tablet) | `x: u32`, `y: u32` |
| 2 | by a distance (relative) | `dx: i32`, `dy: i32` |

A position past the screen's edges is clamped to them by the server (the device clamps it), so a viewer may pass on where a drag that left its window went. The guest's device reports positions or distances, whichever mode it chose, from either kind of motion.

**DiskInfo**: `name: string`, `sectors: u64` (512-byte sectors), `readers: u32` (read-only attachments now), `writer: bool` (a writer has it attached).

**SwitchInfo**, one virtual switch: `name: string`, `capture: option<string>` (the file on the server's host it is being captured into, if it is), `nat: option<NatInfo>` (its NAT gateway, if it has one), `ports: list<SwitchPort>` (in the order they were plugged in, the gateway's among them).

**SwitchPort**, one port of a switch and its counters: `owner: PortOwner`, `mac: mac` (the address of what is plugged in), `in_frames: u64`, `in_bytes: u64` (what the switch received on the port: what the card or gateway sent), `out_frames: u64`, `out_bytes: u64` (what it queued for the port, for the card or gateway to receive), `dropped: u64` (frames for this port lost because its queue was full). Counters start at 0 when the card or gateway is plugged in.

**PortOwner**, what is plugged into a port:

| Tag | Owner | Fields |
|---|---|---|
| 1 | a VM's network card | `vm: u32` (id), `card: u8` (its number) |
| 2 | the switch's NAT gateway (section 6.7, **NAT**) | |

**NatConfig**, the NAT gateway a request asks for: `hostfwd: list<PortForward>` (host ports to forward to guests, in order; may be empty; at most 64, `MAX_FORWARDS`: more is malformed).

**NatInfo**, a switch's NAT gateway as it is: `hostfwd: list<PortForward>` (the rules as bound, in the order given: a host port 0 shows the port the system picked), `leases: list<Lease>` (the addresses its DHCP server has given, in address order).

**PortForward**, one forwarded port: `proto: u8` (1 TCP, 2 UDP; other values are invalid), `host_addr: ipv4`, `host_port: u16` (where the gateway listens on the server's host; port 0 lets the system pick), `guest_addr: ipv4`, `guest_port: u16` (where it passes each connection or datagram, from 10.0.2.2). Round Table and Pendragon write it as QEMU's `hostfwd` rules: `tcp:127.0.0.1:8080-10.0.2.15:80`.

**Lease**: `mac: mac` (a card's address), `addr: ipv4` (its address on the switch's network).

**Names**: VM, disk and switch names (`validate_name`) are 1 to 64 bytes of lowercase ASCII letters, digits, `.`, `_` and `-`, and

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
- **Event**, never answered: console and screen streams (`ConsoleOutput`, `ScreenFrame`), client input (`ConsoleInput`, `KeyInput`, `PointerInput`), and notifications (`VmStateChanged`, `Detached`). Server events can arrive at any time between replies, so a client reading a reply sets aside the events it reads first (the crate's `Connection::request` queues them for `recv`).

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
| 19 | `NoSuchSwitch` | no virtual switch with that name |
| 20 | `NoSuchCard` | the VM has no network card with that number |

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
| 0x0306 | `PointerInput` | Ev | C→S | |
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
| **0x06xx** | **virtual network** (Avalon) | | | |
| 0x0601 | `CreateSwitch` | Req | C→S | `Ok` |
| 0x0602 | `DeleteSwitch` | Req | C→S | `Ok` |
| 0x0603 | `ListSwitches` | Req | C→S | `SwitchList` |
| 0x0604 | `SwitchList` | Rep | S→C | |
| 0x0605 | `AttachCard` | Req | C→S | `Ok` |
| 0x0606 | `DetachCard` | Req | C→S | `Ok` |
| 0x0607 | `CaptureSwitch` | Req | C→S | `Ok` |
| 0x0608 | `SetSwitchNat` | Req | C→S | `Ok` |

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

Example, `Hello` version 5 from agent `"rt"`:

```
0e 00 00 00  01 00  41 56 4c 4e  05 00  02 00 00 00  72 74
length 14    Hello  "AVLN"       v5     agent "rt"
```

### 6.2 VM control

| Message | Payload | Notes |
|---|---|---|
| `CreateVm` | `name: string`, `profile: HardwareProfile`, `program: bytes` | `program` is an Excalibur ELF executable. The VM is loaded and left in state created, its network cards plugged into the switches the profile names. Errors: `InvalidName`, `AlreadyExists`, `InvalidProfile` (also a disk that cannot be opened or attached, see below, and a bad network card), `NoSuchSwitch` (a card names a switch that does not exist), `AlreadyExists` (a card's address is on its switch already), `InvalidProgram`, `OverBudget`. A create that fails plugs in nothing |
| `VmCreated` | `id: u32` | ids are never reused while the server runs |
| `ListVms` | empty | |
| `VmList` | `vms: list<VmSummary>` | in id order |
| `GetVmInfo` | `vm: VmRef` | |
| `VmInfo` | `vm: VmSummary`, `devices: list<DeviceEntry>`, `steps: u64` | `devices` is the guest's sysinfo device table; `steps` the instructions executed so far |
| `StartVm` | `vm: VmRef` | created → running |
| `PauseVm` | `vm: VmRef` | running → paused |
| `ResumeVm` | `vm: VmRef` | paused → running |
| `StopVm` | `vm: VmRef` | created, running or paused → stopped. A stopped or exited VM cannot be restarted; destroy it and create a new one |
| `DestroyVm` | `vm: VmRef` | any state; frees the VM, unplugs its network cards, detaches its clients (`Detached`), and its name can be reused |
| `VmStateChanged` | `vm: u32`, `state: VmState` | sent to every client attached to the VM (console or screen; once to a client attached to both) when its state changes, including when its program exits |
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

The screen is the VM's framebuffer (Excalibur spec section 9.3); the input goes to its keyboard (section 9.4) and pointer (section 9.7) devices. A client attached to a VM's screen is a **viewer** of it.

| Message | Payload | Notes |
|---|---|---|
| `ScreenAttach` | `vm: VmRef` | Errors: `NoSuchVm` |
| `ScreenAttached` | `vm: u32`, `width: u32`, `height: u32`, `format: u32` | the VM's id and its screen. Avalon: 640, 480, format 0. A full `ScreenFrame` follows, then frames whenever the screen changes. Attaching again resends the full frame; the stream is not doubled |
| `ScreenFrame` | `vm: u32`, `x: u32`, `y: u32`, `width: u32`, `height: u32`, `pixels: bytes` | the rectangle at (`x`, `y`), rows top to bottom, `width × height × 4` bytes in the attachment's format. It lies inside the screen; a viewer treats one that does not as a broken server |
| `KeyInput` | `vm: u32`, `key: KeyEvent` | for the VM's keyboard device, as if typed into Grail's window |
| `ScreenDetach` | `vm: u32` | the VM keeps running. Errors: `NoSuchVm`, `InvalidState` (this connection is not attached to it) |
| `PointerInput` | `vm: u32`, `pointer: PointerChange` | for the VM's pointer device, as if the mouse moved over Grail's window |

Format 0 is XRGB8888: each pixel a little-endian word `0x00RRGGBB` (bytes B, G, R, X), the same as Grail's VRAM. A display the guest has turned off (`FB_CTRL.ENABLE` clear) is black, as in Grail's window.

**What is sent.** After the full frame, a viewer gets only what changed: frames are rectangles, and applying them in order to the screen it has gives the VM's screen. An unchanged screen sends no frames at all. The crate's `Screen` is both ends of this: a server keeps one per VM, exactly what its viewers were last sent, and `Screen::update` compares the framebuffer with it in tiles of 32 × 32 pixels and returns the changed area as rectangles (consecutive changed tiles of a tile row make one, which grows downwards while the rows below change over the same columns, so a whole new screen is one rectangle); a viewer starts from black and applies each frame with `Screen::apply_frame`.

**Rate.** However often the guest draws, Avalon looks at a VM's screen at most once every 33 ms (`SCREEN_INTERVAL`, about 30 updates a second), and only while someone is attached and the VM has run since the last look; what changed in between goes out together, as its latest pixels. A guest's `FB_PRESENT` does not send a frame by itself: viewers see the framebuffer, presented or not, as Grail's window does. When the program exits, or the VM is stopped, its last frame is sent before the `VmStateChanged` that says so, so a viewer that ends on that event has the VM's last screen; a viewer attaching to a stopped or exited VM gets that last frame.

**Backlog.** Frames go through the client's queue like every event (section 8), so a viewer on a slow link must not fall behind without limit: one with more than 2 MiB queued and not yet written (`SCREEN_BACKLOG`, a little over one full frame) is sent no updates; once its queue has drained it is sent one full frame, which brings it level with the other viewers. A slow viewer sees fewer frames rather than older ones, and is not cut off for falling 8 MiB behind.

**Input.** `KeyInput` and `PointerInput` are taken only from a viewer of the VM (they are events: from anyone else they are dropped, unanswered), and only while it can still run (created, running or paused). Keys reach the keyboard, and pointer changes the pointer, each in the order the server receives them, between two slices of the VM's run, as Grail's window hands over its input (the two are separate devices, with a FIFO each: the order between a key and a pointer change that arrive for the same slice is not kept, here or in Grail's window); input sent before the VM starts, or while it is paused, waits for it, up to 256 keys and 256 pointer changes per VM, past which more are dropped (as the devices' own FIFOs drop events when full). A viewer that reports a mouse's many moves may send only the last of a run of moves with the same buttons held: only where the pointer ends up matters. A button change (a press or a release, which Grail's window reports as a move to where it happened) is not part of the run before or after it: it is sent where it happened, so that a drag starts and ends where it did.

Several clients may view one VM: all of them get its frames, and input from any of them reaches it. One connection may view several VMs, and view and attach to the console of the same VM; the `vm` field says which stream a frame belongs to.

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

There is no message to attach a disk to an existing VM or detach one from it: a VM's disks are part of its hardware profile (`DeviceSpec`), fixed when it is created, so a client attaches a Lake image by naming it in `CreateVm` (Round Table's `create --disk lake://ADDR/NAME`; Pendragon's *attach* opens its new-VM form with the image, and its *detach* stops the VMs that hold one). The VM's server opens the image (through `BlockOpen`) when it builds the VM's machine, as part of `CreateVm`, and holds the attachment as long as that machine exists. The machine ends, and the server closes the attachment (`BlockClose`), when the VM is stopped (`StopVm`), when its program exits, or when it is destroyed, whichever comes first; a stopped or exited VM cannot run again (section 6.2), so it has no use for its disks. Such a VM still names the disks in its profile (`VmList`, `VmInfo`), since that is the hardware it was created with, but no longer holds them: the images can be attached by another VM or deleted. Image files on the server's own host (`DiskSource::File`) follow the same lifetime. `DiskList` shows each image's current attachments. Hot-plugging disks would be a new message group and a version bump.

The Avalon server does not open Lake disks yet: until Lake (#39) lands, a `CreateVm` whose profile names one is refused with `InvalidProfile`.

### 6.7 Virtual network

Avalon's VMs talk to each other over virtual Ethernet switches, kept by the server. A VM's network card (`DeviceSpec` tag 2, Excalibur spec section 9.8) is plugged into at most one switch at a time; frames its guest sends go into that switch, and the frames the switch passes to the card's port are what the guest receives. Switches on one server are separate networks: a frame never crosses from one switch to another.

| Message | Payload | Notes |
|---|---|---|
| `CreateSwitch` | `name: string`, `nat: option<NatConfig>` | a new switch with no cards plugged in, and with a NAT gateway (below) if `nat` is given. Errors: `InvalidName` (switch names follow the rules for names, section 3), `AlreadyExists`, `OverBudget` (Avalon keeps at most 64 switches), `Io` (a forwarded host port cannot be bound: in use, or an address the host does not have; the message names the rule, and no switch is created) |
| `DeleteSwitch` | `name: string` | unplugs every card on it (their links go down: their guests see the link-up bit clear, and send into nothing), stops its capture, ends its NAT gateway and frees its name. The gateway's host sockets are closed before the `Ok`, so its forwarded ports can be bound again at once. Errors: `NoSuchSwitch` |
| `ListSwitches` | empty | |
| `SwitchList` | `switches: list<SwitchInfo>` | in name order, each with its ports and their counters |
| `AttachCard` | `vm: VmRef`, `card: u8`, `switch: string` | plugs card `card` of the VM into the switch, link up. Errors: `NoSuchVm`, `NoSuchCard`, `NoSuchSwitch`, `Busy` (the card is plugged in already, into this switch or another: detach it first), `AlreadyExists` (a card with its address is on that switch), `InvalidState` (the VM's machine has ended) |
| `DetachCard` | `vm: VmRef`, `card: u8` | unplugs the card, link down. Errors: `NoSuchVm`, `NoSuchCard`, `InvalidState` (it is not plugged in) |
| `CaptureSwitch` | `switch: string`, `path: option<string>` | with a path, starts capturing every frame that enters the switch into a new pcap file at `path` on the server's host (replacing any file there); with none, stops and closes the file. Errors: `NoSuchSwitch`, `Busy` (it is being captured already), `Io` (the file cannot be created; when stopping, writing it failed, see **Captures**), `InvalidState` (stopping a switch that is not being captured) |
| `SetSwitchNat` | `switch: string`, `nat: option<NatConfig>` | with a `NatConfig`, gives the switch a NAT gateway (below); with none, takes its gateway away: its port disappears, its host sockets are closed before the `Ok` (the host ends of its flows see the connection closed), its guests' flows are dropped without a word (no reset is sent to them), and its leases are forgotten. Errors: `NoSuchSwitch`, `AlreadyExists` (it has a gateway already: take it away first; or a card with the gateway's address `52:55:0a:00:02:02` is plugged in), `Io` (as for `CreateSwitch`), `InvalidState` (taking away a gateway it does not have) |

**Forwarding.** A switch is a learning Ethernet switch. When a frame comes in on a port, the switch learns that its source address is reachable through that port, then sends it on: to the one port its destination address was learned on; to every other port (**flooding**) if the destination has not been learned, or is a broadcast or multicast (group) address; nowhere (**filtered**) if the destination was learned on the port the frame came in on. Group source addresses are never learned. A learned address is forgotten when no frame from it has come in for 300 seconds (**ageing**), so a card that moves to another port or switch is found again by flooding, and when its card is unplugged. A switch learns at most 4096 addresses: when its table is full of fresh entries, frames from new addresses are still forwarded, but their addresses are not learned. Frames shorter than an Ethernet header (14 bytes) are counted and captured, but not forwarded; so are a card's frames whose source is the address of another port on the switch (another card's, or the NAT gateway's), which would otherwise move that address to the sender's port and bring it the frames meant for the other.

**Queues and drops.** Each port has a queue of at most 256 frames waiting for its card. A frame for a full queue is dropped and counted in that port's `dropped`: a card that does not take its frames (its VM paused, stopped, slow, or never started) loses its own frames, never anybody else's, and the VM that sends is never held up by the one that does not receive. Frames still queued when a card is unplugged are lost. A card takes frames as its guest receives them; a VM idling in `wfi` is woken when a frame arrives for it.

**Addresses.** A card whose profile gives no address gets `02` followed by five bytes of a hash of the VM's name and the card's number (64-bit FNV-1a over the name's bytes, a 0 byte and the card number; then the hash's five most significant bytes, most significant first: VM `web`'s card 0 is `02:66:b7:0f:2e:09`). The same VM name always gets the same addresses, across re-creations and server restarts, so guests' ARP caches stay valid. Two ports of one switch never have the same address: plugging in a card whose address is on the switch already is refused with `AlreadyExists`.

**Life time.** A switch exists until `DeleteSwitch`, or until the server stops. A card stays plugged in while its VM's machine exists; when the machine ends (the VM is stopped, its program exits, or the VM is destroyed) the card is unplugged, its port disappears from `SwitchList`, and it cannot be plugged in again (`InvalidState`). The card is unplugged before the VM's new state is told to anyone: once a client has `StopVm`'s `Ok`, a `VmStateChanged` to stopped or exited, or a `VmList` or `VmInfo` showing that state, the card is off its switch. The VM's profile in `VmList` and `VmInfo` keeps the card, with its address and no switch. A card that is unplugged sends into nothing: its guest's frames are counted nowhere and lost, as on a card with no cable.

**NAT.** A switch can have one NAT gateway, given when it is created or later (`SetSwitchNat`): the user-mode gateway Grail's `--net user` uses (Excalibur spec section 9.8, "User networking"), which gives the VMs on the switch internet and host access with nothing installed on the host. It is plugged into a port of its own, owner `Gateway`, with the gateway's address `52:55:0a:00:02:02`, and that port behaves like any other: a queue of at most 256 frames (the frames past it dropped and counted), counters, its frames in the switch's capture; and a VM that sends to it is never held up by it, since the gateway runs on a thread of the server's own. The VMs on the switch see the network 10.0.2.0/24 with the gateway at 10.0.2.2 (router, DHCP server, and the host's 127.0.0.1: connecting to 10.0.2.2 reaches a service on the server's host) and a DNS server at 10.0.2.3. One gateway serves every card on the switch, each as a machine of its own:

- **Leases**: its DHCP server gives each card (each MAC address) its own address, from 10.0.2.15 up in the order the cards first ask, and the card keeps it while the gateway lives, across the VM's restarts on the same switch (a VM's generated address follows its name, section 6.7 **Addresses**). `NatInfo.leases` lists them. The pool has 240 addresses; a gateway that has given them all gives no more until it is taken away (`SetSwitchNat` with none) or its switch deleted.
- **Limits**: the gateway's limits on flows (TCP connections, UDP ports and forwarded UDP clients) count per MAC address: 256 per card, 1024 for the whole gateway. The switch passes the gateway only frames whose source address is the address of the card that sent them, so a guest that makes up source addresses cannot pose as more machines (or take more leases) than it has cards.
- **Forwarded ports** (`hostfwd`): each `PortForward` binds a host port on the server's host when the gateway is made (by default on 127.0.0.1, so only programs on that host can connect; 0.0.0.0 opens it to the network), and passes each connection or datagram to `guest_addr:guest_port`, from 10.0.2.2. A rule that cannot be bound fails the request (`Io`), which then leaves the switch as it was. `guest_addr` is 10.0.2.15 by default, the first lease. To reach a particular VM, forward to the address its card leases: leases go from 10.0.2.15 up in the order the cards first ask, so the first VM started on a new switch gets 10.0.2.15 and the second 10.0.2.16, and `NatInfo.leases` shows which card has which. A rule may name an address nobody has yet; connections to it wait for a guest to take it (a forwarded TCP connection whose guest does not answer ARP within a second is closed). The rules are fixed while the gateway lives: changing them takes the gateway away and gives a new one, which starts with no leases.
- Deleting the switch, or taking its gateway away, ends the gateway: its host sockets are closed (before the reply, so the same host ports can be bound again at once; the host ends of its TCP connections see an orderly close, not a reset), the guests' ends of its flows are abandoned (nothing is sent to them: a guest finds out when it next sends and nobody answers), and its leases are forgotten.

**Captures.** A capture writes the classic libpcap format that Wireshark and tcpdump read, exactly as Grail's `--pcap FILE` does (Excalibur spec section 9.8): one record per frame that enters the switch, from any port, before it is forwarded. Frames are written by a thread of the server's own; if it falls more than 1024 frames behind (a slow disk), the frames past that are left out of the capture, but still forwarded. If writing the file fails (a full disk, say), the capture ends there: the file keeps the frames written before the failure, and later frames are not captured. Such a capture is still listed in `SwitchInfo` until it is stopped, and stopping it gets `Io` with the error (it is stopped all the same, so a new one can be started). Deleting the switch ends its capture.

## 7. Versioning

- The protocol version is a `u16`, currently **5** (`PROTOCOL_VERSION`). Both sides must speak the same version; there is no negotiation.
- Any change to the catalogue bumps the version: a new message type, a new field, a new enum variant or tag, a changed layout or a changed rule. (Unknown codes and tags are errors in the decoder, so an old peer could not read them anyway.)
- Three things never change, in any version, so that peers of different versions can always tell each other so: the frame header (`u32` length, `u16` type), the `Hello` and `Welcome` messages (codes 0x0001 and 0x0002 and their layouts) and the `Error` message (code 0x0003 and its layout). Error codes are never reused or renumbered; new ones may be added with a version bump.
- Things that are deliberately open-ended and need no new version: `DeviceEntry.kind` (sysinfo device type codes come from the Excalibur spec), `ScreenAttached.format` values a server chooses to send, free-text fields.
- This document describes version 5. When the version changes, the change is described here and the crate's round-trip, layout and fuzz tests are updated with it.

History:

| Version | Change |
|---|---|
| 1 | the first version |
| 2 | `PointerInput` (0x0306) with the `PointerChange` and `PointerMotion` structures, and the `i8` and `i32` field types, for the guest's pointer device; Avalon serves the screen and input group (section 6.4), which version 1 servers answered with `Unsupported` |
| 3 | the virtual network group 0x06xx (section 6.7: `CreateSwitch`, `DeleteSwitch`, `ListSwitches`, `SwitchList`, `AttachCard`, `DetachCard`, `CaptureSwitch`) with the `SwitchInfo`, `SwitchPort` and `PortOwner` structures and the `mac` field type; network cards in hardware profiles (`DeviceSpec` tag 2); error codes 19 `NoSuchSwitch` and 20 `NoSuchCard` |
| 4 | `HardwareProfile.harts` (`u32`, after `nominal_hz`), for multi-hart machines |
| 5 | NAT gateways on switches (section 6.7, **NAT**): `SetSwitchNat` (0x0608), `CreateSwitch`'s `nat` field, `SwitchInfo`'s `nat` field, `PortOwner` tag 2 (`Gateway`), the `NatConfig`, `NatInfo`, `PortForward` and `Lease` structures (at most `MAX_FORWARDS` 64 forwarded ports per gateway) and the `ipv4` field type |

## 8. Limits

| Constant | Value | Where |
|---|---|---|
| `MAX_FRAME_LEN` | 16 MiB | largest `length` field |
| `MAX_BLOCK_SECTORS` | 2048 (1 MiB) | per `BlockRead`/`BlockWrite` |
| `SECTOR_SIZE` | 512 | block I/O |
| `MAX_NAME_LEN` | 64 | VM, disk and switch names |
| network cards | 4 per VM | `CreateVm` (`InvalidProfile`) |
| switches | 64 per Avalon server | `CreateSwitch` (`OverBudget`) |
| port queue | 256 frames | per switch port (the gateway's too); more are dropped and counted |
| NAT gateways | 1 per switch | `CreateSwitch`, `SetSwitchNat` (`AlreadyExists`) |
| `MAX_FORWARDS` | 64 forwarded ports per gateway | `NatConfig` and `NatInfo` (more is malformed) |
| gateway flows | 256 per card, 1024 per gateway | section 6.7, **NAT** |
| DHCP leases | 240 per gateway (10.0.2.15 to 10.0.2.254) | |
| learned addresses | 4096 per switch, forgotten after 300 s | |
| screen updates | at most one look every 33 ms per VM | Avalon (`SCREEN_INTERVAL`) |
| viewer backlog | 2 MiB queued, then no screen updates until it drains | Avalon (`SCREEN_BACKLOG`) |
| input waiting | 256 keys and 256 pointer changes per VM | Avalon (`MAX_INPUT_QUEUE`) |
| default Avalon address | `127.0.0.1:7400` | |
| default Lake address | `127.0.0.1:7401` | |

Servers may set lower limits of their own (a RAM budget, a maximum image size) and answer with `OverBudget`. A server may also close the connection of a client that does not read what is sent to it, rather than hold unbounded output for it: Avalon does once 8 MiB of output (events and replies) are waiting for one client.
