//! The message catalogue (protocol doc, "Messages"): every message as a [`Message`]
//! variant, its type code, and its payload layout.
//!
//! Type codes are grouped by their high byte: `0x00` session, `0x01` VM control, `0x02`
//! console, `0x03` screen and input (keyboard and pointer), `0x04` block I/O, `0x05`
//! disk management, `0x06` virtual network (Avalon's switches), `0x07` debugging (a VM's
//! GDB stub, tunnelled). Every message is a
//! request (answered by exactly one reply), a reply, or an event (never answered); see
//! [`MessageKind`].

use crate::wire::{Reader, Writer};
use crate::{Error, Result, MAGIC, MAX_BLOCK_SECTORS, MAX_FORWARDS, SECTOR_SIZE};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};

/// Message type codes.
pub mod ty {
    // Session.
    pub const HELLO: u16 = 0x0001;
    pub const WELCOME: u16 = 0x0002;
    pub const ERROR: u16 = 0x0003;
    pub const OK: u16 = 0x0004;
    pub const PING: u16 = 0x0005;
    pub const PONG: u16 = 0x0006;
    // VM control.
    pub const CREATE_VM: u16 = 0x0101;
    pub const VM_CREATED: u16 = 0x0102;
    pub const LIST_VMS: u16 = 0x0103;
    pub const VM_LIST: u16 = 0x0104;
    pub const GET_VM_INFO: u16 = 0x0105;
    pub const VM_INFO: u16 = 0x0106;
    pub const START_VM: u16 = 0x0107;
    pub const PAUSE_VM: u16 = 0x0108;
    pub const RESUME_VM: u16 = 0x0109;
    pub const STOP_VM: u16 = 0x010a;
    pub const DESTROY_VM: u16 = 0x010b;
    pub const VM_STATE_CHANGED: u16 = 0x010c;
    pub const DETACHED: u16 = 0x010d;
    // Console.
    pub const CONSOLE_ATTACH: u16 = 0x0201;
    pub const CONSOLE_ATTACHED: u16 = 0x0202;
    pub const CONSOLE_INPUT: u16 = 0x0203;
    pub const CONSOLE_OUTPUT: u16 = 0x0204;
    pub const CONSOLE_DETACH: u16 = 0x0205;
    // Screen and input.
    pub const SCREEN_ATTACH: u16 = 0x0301;
    pub const SCREEN_ATTACHED: u16 = 0x0302;
    pub const SCREEN_FRAME: u16 = 0x0303;
    pub const KEY_INPUT: u16 = 0x0304;
    pub const SCREEN_DETACH: u16 = 0x0305;
    pub const POINTER_INPUT: u16 = 0x0306;
    pub const SCREEN_RESIZED: u16 = 0x0307;
    pub const CURSOR_SHAPE: u16 = 0x0308;
    pub const CURSOR_POSITION: u16 = 0x0309;
    // Block I/O.
    pub const BLOCK_OPEN: u16 = 0x0401;
    pub const BLOCK_OPENED: u16 = 0x0402;
    pub const BLOCK_READ: u16 = 0x0403;
    pub const BLOCK_DATA: u16 = 0x0404;
    pub const BLOCK_WRITE: u16 = 0x0405;
    pub const BLOCK_FLUSH: u16 = 0x0406;
    pub const BLOCK_SIZE: u16 = 0x0407;
    pub const BLOCK_CAPACITY: u16 = 0x0408;
    pub const BLOCK_CLOSE: u16 = 0x0409;
    // Disk management.
    pub const DISK_CREATE: u16 = 0x0501;
    pub const LIST_DISKS: u16 = 0x0502;
    pub const DISK_LIST: u16 = 0x0503;
    pub const DISK_DELETE: u16 = 0x0504;
    // Virtual network.
    pub const CREATE_SWITCH: u16 = 0x0601;
    pub const DELETE_SWITCH: u16 = 0x0602;
    pub const LIST_SWITCHES: u16 = 0x0603;
    pub const SWITCH_LIST: u16 = 0x0604;
    pub const ATTACH_CARD: u16 = 0x0605;
    pub const DETACH_CARD: u16 = 0x0606;
    pub const CAPTURE_SWITCH: u16 = 0x0607;
    pub const SET_SWITCH_NAT: u16 = 0x0608;
    // Debugging.
    pub const DEBUG_ATTACH: u16 = 0x0701;
    pub const DEBUG_ATTACHED: u16 = 0x0702;
    pub const DEBUG_INPUT: u16 = 0x0703;
    pub const DEBUG_OUTPUT: u16 = 0x0704;
    pub const DEBUG_DETACH: u16 = 0x0705;
}

/// Every message type, with its name and kind, in code order. The protocol doc's
/// tables follow this list; tests check that every entry round-trips.
pub const CATALOGUE: &[(u16, &str, MessageKind)] = {
    use MessageKind::{Event, Reply, Request};
    &[
        (ty::HELLO, "Hello", Request),
        (ty::WELCOME, "Welcome", Reply),
        (ty::ERROR, "Error", Reply),
        (ty::OK, "Ok", Reply),
        (ty::PING, "Ping", Request),
        (ty::PONG, "Pong", Reply),
        (ty::CREATE_VM, "CreateVm", Request),
        (ty::VM_CREATED, "VmCreated", Reply),
        (ty::LIST_VMS, "ListVms", Request),
        (ty::VM_LIST, "VmList", Reply),
        (ty::GET_VM_INFO, "GetVmInfo", Request),
        (ty::VM_INFO, "VmInfo", Reply),
        (ty::START_VM, "StartVm", Request),
        (ty::PAUSE_VM, "PauseVm", Request),
        (ty::RESUME_VM, "ResumeVm", Request),
        (ty::STOP_VM, "StopVm", Request),
        (ty::DESTROY_VM, "DestroyVm", Request),
        (ty::VM_STATE_CHANGED, "VmStateChanged", Event),
        (ty::DETACHED, "Detached", Event),
        (ty::CONSOLE_ATTACH, "ConsoleAttach", Request),
        (ty::CONSOLE_ATTACHED, "ConsoleAttached", Reply),
        (ty::CONSOLE_INPUT, "ConsoleInput", Event),
        (ty::CONSOLE_OUTPUT, "ConsoleOutput", Event),
        (ty::CONSOLE_DETACH, "ConsoleDetach", Request),
        (ty::SCREEN_ATTACH, "ScreenAttach", Request),
        (ty::SCREEN_ATTACHED, "ScreenAttached", Reply),
        (ty::SCREEN_FRAME, "ScreenFrame", Event),
        (ty::KEY_INPUT, "KeyInput", Event),
        (ty::SCREEN_DETACH, "ScreenDetach", Request),
        (ty::POINTER_INPUT, "PointerInput", Event),
        (ty::SCREEN_RESIZED, "ScreenResized", Event),
        (ty::CURSOR_SHAPE, "CursorShape", Event),
        (ty::CURSOR_POSITION, "CursorPosition", Event),
        (ty::BLOCK_OPEN, "BlockOpen", Request),
        (ty::BLOCK_OPENED, "BlockOpened", Reply),
        (ty::BLOCK_READ, "BlockRead", Request),
        (ty::BLOCK_DATA, "BlockData", Reply),
        (ty::BLOCK_WRITE, "BlockWrite", Request),
        (ty::BLOCK_FLUSH, "BlockFlush", Request),
        (ty::BLOCK_SIZE, "BlockSize", Request),
        (ty::BLOCK_CAPACITY, "BlockCapacity", Reply),
        (ty::BLOCK_CLOSE, "BlockClose", Request),
        (ty::DISK_CREATE, "DiskCreate", Request),
        (ty::LIST_DISKS, "ListDisks", Request),
        (ty::DISK_LIST, "DiskList", Reply),
        (ty::DISK_DELETE, "DiskDelete", Request),
        (ty::CREATE_SWITCH, "CreateSwitch", Request),
        (ty::DELETE_SWITCH, "DeleteSwitch", Request),
        (ty::LIST_SWITCHES, "ListSwitches", Request),
        (ty::SWITCH_LIST, "SwitchList", Reply),
        (ty::ATTACH_CARD, "AttachCard", Request),
        (ty::DETACH_CARD, "DetachCard", Request),
        (ty::CAPTURE_SWITCH, "CaptureSwitch", Request),
        (ty::SET_SWITCH_NAT, "SetSwitchNat", Request),
        (ty::DEBUG_ATTACH, "DebugAttach", Request),
        (ty::DEBUG_ATTACHED, "DebugAttached", Reply),
        (ty::DEBUG_INPUT, "DebugInput", Event),
        (ty::DEBUG_OUTPUT, "DebugOutput", Event),
        (ty::DEBUG_DETACH, "DebugDetach", Request),
    ]
};

/// A message type's name (`"CreateVm"`), or its code in hex if it is not in the
/// catalogue.
pub fn type_name(code: u16) -> String {
    match CATALOGUE.iter().find(|(c, _, _)| *c == code) {
        Some((_, name, _)) => (*name).to_string(),
        None => format!("{code:#06x}"),
    }
}

/// The kind of message type `code`, or `None` if it is not in the catalogue. Useful
/// when a frame fails to decode: a server answers a bad request with an `Error` reply
/// but never answers a bad event, and a client waiting for a reply can tell a bad event
/// (set aside) from a bad reply (the answer to its request).
pub fn kind_of(code: u16) -> Option<MessageKind> {
    CATALOGUE
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, _, kind)| *kind)
}

/// How a message is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Client to server; the server answers with exactly one reply, in order.
    Request,
    /// Server to client: `Ok`, `Error` or a request-specific result.
    Reply,
    /// Either direction, never answered: console and screen streams, input, and state
    /// change notifications.
    Event,
}

/// Pixel format of `ScreenFrame`: little-endian words `0x00RRGGBB` (bytes B, G, R, X),
/// the same as Grail's VRAM (`FB_FORMAT` 0, Excalibur spec section 9.3).
pub const PIXEL_FORMAT_XRGB8888: u32 = 0;

/// Bytes a `ScreenFrame`'s length field counts besides its pixels: the message type (2),
/// `vm`, `x`, `y`, `width` and `height` (4 each) and the pixels' length (4). A rectangle
/// of `n` pixel bytes fits in one frame if `n + SCREEN_FRAME_OVERHEAD` is at most
/// [`crate::MAX_FRAME_LEN`].
pub const SCREEN_FRAME_OVERHEAD: usize = 2 + 5 * 4 + 4;

/// The largest cursor image `CursorShape` carries, in pixels each way: 256, four times
/// Grail's 64 × 64 (some display controllers' cursor planes, Intel's among them, go up
/// to 256 × 256).
pub const MAX_CURSOR_SIZE: u32 = 256;

/// RAM size of [`HardwareProfile::default`], as for `grail::DEFAULT_RAM_SIZE`.
pub const DEFAULT_RAM_SIZE: u64 = 4 * 1024 * 1024;
/// Nominal clock of [`HardwareProfile::default`], as for Grail's default.
pub const DEFAULT_NOMINAL_HZ: u32 = 10_000_000;
/// Harts of [`HardwareProfile::default`], as for Grail's default.
pub const DEFAULT_HARTS: u32 = 1;

/// The paging modes a VM's harts implement, as `grail::Paging`: QEMU's `-cpu rv64`
/// (Sv48 too, the default) or `-cpu rv64,sv48=off`. On the wire a `u8`, the widest
/// mode as `satp.MODE` numbers it (privileged spec, "satp"): 9 for Sv48 (and Sv39),
/// 8 for Sv39 alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Paging {
    /// Sv48 and Sv39 (and Bare): `satp.MODE` 9 is the widest.
    #[default]
    Sv48,
    /// Sv39 (and Bare) only, as on most RV64 boards: a write of MODE 9 to `satp` is
    /// refused (WARL), which is how the guest finds out.
    Sv39,
}

impl Paging {
    /// The widest mode's `satp.MODE` value, 9 or 8: what the wire carries.
    pub fn satp_mode(self) -> u8 {
        match self {
            Paging::Sv48 => 9,
            Paging::Sv39 => 8,
        }
    }

    /// The paging of `mode`, if it is 9 (Sv48) or 8 (Sv39).
    pub fn from_satp_mode(mode: u8) -> Option<Paging> {
        match mode {
            9 => Some(Paging::Sv48),
            8 => Some(Paging::Sv39),
            _ => None,
        }
    }
}

impl fmt::Display for Paging {
    /// `sv48` or `sv39`, as Grail's `--paging` takes it.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Paging::Sv48 => "sv48",
            Paging::Sv39 => "sv39",
        })
    }
}

/// A VM's hardware, mirroring `grail::MachineConfig` field for field (a test in this
/// crate fails to compile when `MachineConfig` gains a field this does not have).
/// The server validates it as Grail does and answers `InvalidProfile` if it is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareProfile {
    /// RAM size in bytes: a non-zero multiple of 4 KiB, at most 1 TiB.
    pub ram_size: u64,
    /// Real-time clock in instructions per second, or `None` to run at full speed.
    pub clock_hz: Option<u32>,
    /// The clock reported to the guest when `clock_hz` is `None`.
    pub nominal_hz: u32,
    /// Harts (cores), each running at the clock: 1 to `grail::MAX_HARTS` (8).
    pub harts: u32,
    /// The paging modes the harts implement. (Every VM is RV64: until version 14 the
    /// profile had a register width too, for RV32 VMs.)
    pub paging: Paging,
    /// The firmware the VM boots, as QEMU's `-bios`: none (the program runs in
    /// M-mode), Camelot's, or one in a file on the server's host.
    pub bios: Bios,
    /// Optional hardware beyond the fixed machine (spec section 7), in the order the
    /// guest should see it.
    pub devices: Vec<DeviceSpec>,
}

impl Default for HardwareProfile {
    fn default() -> Self {
        HardwareProfile {
            ram_size: DEFAULT_RAM_SIZE,
            clock_hz: None,
            nominal_hz: DEFAULT_NOMINAL_HZ,
            harts: DEFAULT_HARTS,
            paging: Paging::default(),
            bios: Bios::default(),
            devices: Vec::new(),
        }
    }
}

/// A VM's firmware, as `grail::Bios`: QEMU's `-bios none`, `-bios default` or
/// `-bios FILE` (Excalibur spec section 7, "Firmware"). On the wire a `u8` tag, 0, 1
/// or 2, the last followed by the file's path as a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Bios {
    /// No firmware: the boot ROM starts the program, or the disk's boot block, in
    /// M-mode.
    #[default]
    None,
    /// Camelot's firmware, which Grail carries: an SBI in M-mode that starts the
    /// program, or the disk's boot block, in S-mode.
    Default,
    /// The firmware in this file on the server's host (an ELF, or a raw binary loaded
    /// at the start of RAM), as a disk's `File` source is.
    File(String),
}

impl Bios {
    /// The firmware a `--bios` value names: `none`, `default`, or a file's path.
    pub fn parse(s: &str) -> std::result::Result<Bios, String> {
        match s {
            "" => Err("give none, default or a firmware file".into()),
            "none" => Ok(Bios::None),
            "default" => Ok(Bios::Default),
            path => Ok(Bios::File(path.into())),
        }
    }
}

impl fmt::Display for Bios {
    /// `none`, `default` or the file's path, as `--bios` takes it.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Bios::None => f.write_str("none"),
            Bios::Default => f.write_str("default"),
            Bios::File(path) => f.write_str(path),
        }
    }
}

impl HardwareProfile {
    /// The default profile with `ram_size` bytes of RAM.
    pub fn with_ram(ram_size: u64) -> Self {
        HardwareProfile {
            ram_size,
            ..HardwareProfile::default()
        }
    }

    /// CPU (and timebase) clock the guest sees.
    pub fn cpu_hz(&self) -> u32 {
        self.clock_hz.unwrap_or(self.nominal_hz)
    }
}

/// An optional device in a hardware profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSpec {
    /// A block storage device.
    Disk(DiskSpec),
    /// A network card (Excalibur spec section 9.8).
    Net(NetSpec),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSpec {
    pub source: DiskSource,
    /// Attach read-only: the guest's writes fail, and Lake allows other readers.
    pub read_only: bool,
}

/// A network card in a hardware profile: the virtual switch its cable is plugged into
/// on the server, and its MAC address.
///
/// In `CreateVm`, `switch` is the switch to plug the card into when the VM is created
/// (`None`: unplugged, link down) and `mac` the card's address (`None`: the server
/// makes one up). In `VmList` and `VmInfo` both are what they are now: the switch the
/// card is plugged into at the moment, and the address it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetSpec {
    pub switch: Option<String>,
    /// In wire order. A card's address must be an individual, locally administered
    /// one (`02:..`); the server checks.
    pub mac: Option<[u8; 6]>,
}

/// Where a disk's blocks live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskSource {
    /// An image file on the Avalon host.
    File(String),
    /// A named image on a Lake server (`lake://addr/name`).
    Lake { addr: String, name: String },
}

/// Names a VM in a request: by the id Avalon gave it, or by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmRef {
    Id(u32),
    Name(String),
}

impl VmRef {
    /// Parses a command-line VM argument: all digits is an id, anything else a name.
    /// Valid names are never all digits ([`validate_name`]), so every VM can be named
    /// this way.
    ///
    /// [`validate_name`]: crate::validate_name
    pub fn parse(s: &str) -> VmRef {
        match s.parse() {
            Ok(id) if s.bytes().all(|b| b.is_ascii_digit()) => VmRef::Id(id),
            _ => VmRef::Name(s.to_string()),
        }
    }
}

impl fmt::Display for VmRef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VmRef::Id(id) => write!(f, "{id}"),
            VmRef::Name(name) => write!(f, "{name}"),
        }
    }
}

/// A VM's life cycle: created, then running, paused and resumed any number of times,
/// and stopped by its debugger and resumed by it, until it is stopped or its program
/// exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Created,
    Running,
    Paused,
    /// Stopped by a client before the program exited.
    Stopped,
    /// The program stopped the machine with this exit code.
    Exited(u32),
    /// Held by its debugger: stopped at a breakpoint, a watchpoint, a step or an
    /// interrupt, until the debugger resumes it (QEMU's run state `debug`).
    Debug,
}

impl fmt::Display for VmState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VmState::Created => write!(f, "created"),
            VmState::Running => write!(f, "running"),
            VmState::Paused => write!(f, "paused"),
            VmState::Stopped => write!(f, "stopped"),
            VmState::Exited(code) => write!(f, "exited({code})"),
            VmState::Debug => write!(f, "debug"),
        }
    }
}

/// One VM in a `VmList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSummary {
    pub id: u32,
    pub name: String,
    pub state: VmState,
    pub profile: HardwareProfile,
}

/// Everything `VmInfo` reports about one VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmDetails {
    pub vm: VmSummary,
    /// The guest's sysinfo device table (Excalibur spec section 9.5), as it reads it.
    pub devices: Vec<DeviceEntry>,
    /// Instructions executed so far.
    pub steps: u64,
}

/// One sysinfo device table entry (Excalibur spec section 9.5). `kind` is the raw type
/// code, so new device types need no protocol change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEntry {
    pub kind: u32,
    pub base: u32,
    pub size: u32,
    /// PLIC source, 0 = none.
    pub irq: u32,
}

/// Which stream an attachment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachKind {
    Console,
    Screen,
    Debug,
}

/// A key press or release for the guest's keyboard device (Excalibur spec section
/// 9.4), in both of the device's forms: a viewer does not know which mode the guest
/// chose, so it sends the character typed and the physical key, and the device keeps
/// the one of its mode. At least one of `code` and `usage` is not 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// ASCII mode's key code, from the spec's table; 0 for a key that types nothing
    /// (a modifier alone, Home, an accented letter).
    pub code: u8,
    pub release: bool,
    /// Shift was held (for ASCII mode).
    pub shift: bool,
    /// Ctrl was held (for ASCII mode).
    pub ctrl: bool,
    /// A press of a key already down (the viewer's auto-repeat): another press in
    /// ASCII mode, nothing in raw mode. Never set on a release.
    pub repeat: bool,
    /// Raw mode's USB HID usage of the physical key (Keyboard/Keypad page); 0 if the
    /// viewer does not know it.
    pub usage: u16,
}

const KEY_RELEASE: u8 = 1 << 0;
const KEY_SHIFT: u8 = 1 << 1;
const KEY_CTRL: u8 = 1 << 2;
const KEY_REPEAT: u8 = 1 << 3;
/// Every key flag bit; the others must be 0.
const KEY_FLAGS: u8 = KEY_RELEASE | KEY_SHIFT | KEY_CTRL | KEY_REPEAT;

/// Pointer button bits of [`PointerChange::buttons`], as in the guest's pointer device
/// (Excalibur spec section 9.7).
pub const POINTER_LEFT: u8 = 1 << 0;
pub const POINTER_RIGHT: u8 = 1 << 1;
pub const POINTER_MIDDLE: u8 = 1 << 2;
/// Every pointer button bit; the others must be 0.
pub const POINTER_BUTTONS: u8 = POINTER_LEFT | POINTER_RIGHT | POINTER_MIDDLE;

/// How a viewer's pointer moved, in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerMotion {
    /// It did not: a button or wheel change only.
    #[default]
    None,
    /// To this pixel of the screen (absolute, like a tablet). The server clamps it to
    /// the screen.
    To { x: u32, y: u32 },
    /// By this many pixels (relative, for a viewer that only has deltas).
    By { dx: i32, dy: i32 },
}

/// One change of a viewer's pointer, for the guest's pointer device (Excalibur spec
/// section 9.7): how it moved, the buttons held after the change, and the wheel
/// notches turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PointerChange {
    pub motion: PointerMotion,
    /// [`POINTER_LEFT`], [`POINTER_RIGHT`], [`POINTER_MIDDLE`]; other bits must be 0.
    pub buttons: u8,
    /// Notches, positive away from the user (up).
    pub wheel: i8,
}

/// One image in a `DiskList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskInfo {
    pub name: String,
    /// Size in 512-byte sectors.
    pub sectors: u64,
    /// Read-only attachments right now.
    pub readers: u32,
    /// Whether a writer has it attached (exclusive).
    pub writer: bool,
}

/// One virtual switch in a `SwitchList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchInfo {
    pub name: String,
    /// The file on the server's host this switch is being captured into, if it is.
    pub capture: Option<String>,
    /// Its NAT gateway, if it has one.
    pub nat: Option<NatInfo>,
    /// Its ports, in the order they were plugged in (the gateway's among them).
    pub ports: Vec<SwitchPort>,
}

/// A NAT gateway for a switch (protocol doc, section 6.7, "NAT"): what `CreateSwitch`
/// and `SetSwitchNat` ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NatConfig {
    /// Host ports forwarded to guests, in order.
    pub hostfwd: Vec<PortForward>,
}

/// A switch's NAT gateway as it is now, in `SwitchInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NatInfo {
    /// The forwarding rules as bound: a host port 0 shows the port the system picked.
    pub hostfwd: Vec<PortForward>,
    /// The addresses the gateway's DHCP server has given out, in address order.
    pub leases: Vec<Lease>,
}

/// One DHCP lease of a switch's gateway: a card's MAC address and its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub mac: [u8; 6],
    pub addr: Ipv4Addr,
}

/// Which transport a forwarded port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FwdProto {
    Tcp,
    Udp,
}

/// One host port forwarded to a guest by a switch's gateway: the gateway listens on
/// `host` and passes each connection (TCP) or datagram (UDP) to `guest`, from
/// 10.0.2.2. Shown as QEMU writes `hostfwd` rules (`tcp:127.0.0.1:8080-10.0.2.15:80`),
/// which is also how Round Table and Pendragon read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortForward {
    pub proto: FwdProto,
    /// Where the host side listens; port 0 lets the system pick.
    pub host: SocketAddrV4,
    /// A guest address (see [`is_guest_addr`]) and a port other than 0.
    pub guest: SocketAddrV4,
}

impl fmt::Display for PortForward {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let proto = match self.proto {
            FwdProto::Tcp => "tcp",
            FwdProto::Udp => "udp",
        };
        write!(f, "{proto}:{}-{}", self.host, self.guest)
    }
}

/// Whether a guest of a switch's gateway may have `ip`: in 10.0.2.0/24, and not the
/// network (.0), the gateway (.2), the DNS server (.3) or the broadcast address (.255).
/// The gateway crate's rule (`gateway::is_guest_addr`), which a test checks this
/// against: the protocol crate depends on nothing.
pub fn is_guest_addr(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[..3] == [10, 0, 2] && ![0, 2, 3, 255].contains(&o[3])
}

/// One port of a virtual switch and its counters. "In" is what the switch received on
/// the port (what the card sent), "out" what it passed on to the port (for the card to
/// receive); `dropped` counts frames for this port that were lost because its queue
/// was full (the card did not take them fast enough).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchPort {
    pub owner: PortOwner,
    /// The MAC address of what is plugged in.
    pub mac: [u8; 6],
    pub in_frames: u64,
    pub in_bytes: u64,
    pub out_frames: u64,
    pub out_bytes: u64,
    pub dropped: u64,
}

/// What is plugged into a switch port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortOwner {
    /// Network card number `card` (from 0, the order of the profile's cards) of VM `vm`.
    Card { vm: u32, card: u8 },
    /// The switch's NAT gateway.
    Gateway,
}

/// Error codes carried by the `Error` message. Codes are never reused. Unknown codes
/// decode as [`ErrorCode::Other`] so a client can still show them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The frame or payload could not be decoded.
    Malformed,
    /// The message type is not in the catalogue.
    UnknownMessage,
    /// A valid message that is not allowed here: not `Hello` before the handshake, a
    /// reply sent by a client. (Events are never answered, even in the wrong
    /// direction.)
    Unexpected,
    /// This server does not serve that message group (Lake asked to create a VM).
    Unsupported,
    NoSuchVm,
    NoSuchDisk,
    /// Unknown or closed block handle.
    NoSuchHandle,
    /// A VM or disk with that name already exists.
    AlreadyExists,
    InvalidName,
    /// The hardware profile is not a machine Grail can build.
    InvalidProfile,
    /// The program image is not a loadable ELF.
    InvalidProgram,
    /// The VM is in the wrong state for the request (pausing a stopped VM, ...).
    InvalidState,
    /// The request would exceed a server resource limit (Avalon's RAM budget, ...).
    OverBudget,
    /// The disk is attached in a way that forbids the request (a second writer,
    /// deleting an attached image).
    Busy,
    /// A write through a read-only attachment.
    ReadOnly,
    /// A sector range past the end of the disk.
    OutOfRange,
    /// The server's host I/O failed.
    Io,
    /// A bug or unexpected condition in the server.
    Internal,
    /// No virtual switch with that name.
    NoSuchSwitch,
    /// The VM has no network card with that number.
    NoSuchCard,
    /// A code this version does not know. Never the number of a named code:
    /// [`Message::check`] refuses `Other(5)`, which would decode as `NoSuchVm`.
    Other(u16),
}

impl ErrorCode {
    const TABLE: [(ErrorCode, u16, &'static str); 20] = [
        (ErrorCode::Malformed, 1, "malformed"),
        (ErrorCode::UnknownMessage, 2, "unknown message"),
        (ErrorCode::Unexpected, 3, "unexpected message"),
        (ErrorCode::Unsupported, 4, "unsupported"),
        (ErrorCode::NoSuchVm, 5, "no such VM"),
        (ErrorCode::NoSuchDisk, 6, "no such disk"),
        (ErrorCode::NoSuchHandle, 7, "no such handle"),
        (ErrorCode::AlreadyExists, 8, "already exists"),
        (ErrorCode::InvalidName, 9, "invalid name"),
        (ErrorCode::InvalidProfile, 10, "invalid hardware profile"),
        (ErrorCode::InvalidProgram, 11, "invalid program"),
        (ErrorCode::InvalidState, 12, "invalid state"),
        (ErrorCode::OverBudget, 13, "over budget"),
        (ErrorCode::Busy, 14, "busy"),
        (ErrorCode::ReadOnly, 15, "read-only"),
        (ErrorCode::OutOfRange, 16, "out of range"),
        (ErrorCode::Io, 17, "I/O error"),
        (ErrorCode::Internal, 18, "internal error"),
        (ErrorCode::NoSuchSwitch, 19, "no such switch"),
        (ErrorCode::NoSuchCard, 20, "no such network card"),
    ];

    /// Every named code, for tests and documentation.
    pub fn all() -> impl Iterator<Item = ErrorCode> {
        Self::TABLE.iter().map(|(c, _, _)| *c)
    }

    pub fn to_u16(self) -> u16 {
        match self {
            ErrorCode::Other(n) => n,
            code => {
                Self::TABLE
                    .iter()
                    .find(|(c, _, _)| *c == code)
                    .expect("named code")
                    .1
            }
        }
    }

    pub fn from_u16(n: u16) -> ErrorCode {
        match Self::TABLE.iter().find(|(_, v, _)| *v == n) {
            Some((c, _, _)) => *c,
            None => ErrorCode::Other(n),
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match Self::TABLE.iter().find(|(c, _, _)| c == self) {
            Some((_, _, name)) => write!(f, "{name}"),
            None => write!(f, "error {}", self.to_u16()),
        }
    }
}

/// Every message in the protocol. The doc comment of each variant says who sends it;
/// the protocol doc has the byte layouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    // ---- Session ----
    /// Client, first frame on a connection. The payload starts with [`MAGIC`].
    Hello {
        version: u16,
        agent: String,
    },
    /// Server's answer to `Hello`, always with its own version. The session goes on only
    /// if the versions are equal; otherwise the server closes the connection.
    Welcome {
        version: u16,
        server: String,
    },
    /// A request failed.
    Error {
        code: ErrorCode,
        message: String,
    },
    /// A request succeeded and has nothing else to return.
    Ok,
    /// Liveness check, answered by `Pong` with the same token.
    Ping {
        token: u32,
    },
    Pong {
        token: u32,
    },

    // ---- VM control (Avalon) ----
    /// Create a VM (state `Created`) from an ELF image. Answered by `VmCreated`.
    CreateVm {
        name: String,
        profile: HardwareProfile,
        program: Vec<u8>,
    },
    VmCreated {
        id: u32,
    },
    /// Answered by `VmList`.
    ListVms,
    VmList {
        vms: Vec<VmSummary>,
    },
    /// Answered by `VmInfo`.
    GetVmInfo {
        vm: VmRef,
    },
    VmInfo(VmDetails),
    /// Life-cycle requests, each answered by `Ok`.
    StartVm {
        vm: VmRef,
    },
    PauseVm {
        vm: VmRef,
    },
    ResumeVm {
        vm: VmRef,
    },
    StopVm {
        vm: VmRef,
    },
    DestroyVm {
        vm: VmRef,
    },
    /// Server, to every client attached to the VM, when its state changes.
    VmStateChanged {
        vm: u32,
        state: VmState,
    },
    /// Server: it ended an attachment (the VM was destroyed; a debugger's also when the
    /// VM's machine ended).
    Detached {
        vm: u32,
        kind: AttachKind,
    },

    // ---- Console (Avalon, Herald) ----
    /// Attach to a VM's serial console. Answered by `ConsoleAttached` with the recent
    /// output; `ConsoleOutput` events follow.
    ConsoleAttach {
        vm: VmRef,
    },
    ConsoleAttached {
        vm: u32,
        scrollback: Vec<u8>,
    },
    /// Client: bytes for the VM's UART receiver.
    ConsoleInput {
        vm: u32,
        data: Vec<u8>,
    },
    /// Server: bytes the VM wrote to its UART.
    ConsoleOutput {
        vm: u32,
        data: Vec<u8>,
    },
    /// Answered by `Ok`; the VM keeps running.
    ConsoleDetach {
        vm: u32,
    },

    // ---- Screen and input (Avalon, Herald) ----
    /// Attach to a VM's framebuffer. Answered by `ScreenAttached`, then a full
    /// `ScreenFrame`, then a frame whenever the screen changes.
    ScreenAttach {
        vm: VmRef,
    },
    ScreenAttached {
        vm: u32,
        width: u32,
        height: u32,
        format: u32,
    },
    /// Server: the pixels of the rectangle at (`x`, `y`), row by row, 4 bytes each.
    ScreenFrame {
        vm: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// Client: a key event for the VM's keyboard device.
    KeyInput {
        vm: u32,
        key: KeyEvent,
    },
    /// Answered by `Ok`.
    ScreenDetach {
        vm: u32,
    },
    /// Client: a change of its pointer for the VM's pointer device.
    PointerInput {
        vm: u32,
        pointer: PointerChange,
    },
    /// Server: the VM's screen is now `width` × `height` in `format` (the guest changed
    /// modes). The viewer's screen starts again from black at that size; a full
    /// `ScreenFrame` follows.
    ScreenResized {
        vm: u32,
        width: u32,
        height: u32,
        format: u32,
    },
    /// Server: the image of the VM's cursor (its framebuffer's cursor plane), from now
    /// on: `width` × `height` pixels of ARGB8888 with premultiplied alpha (bytes B, G, R,
    /// A), row by row, its hotspot at (`hot_x`, `hot_y`). Sent when the image changes,
    /// apart from where the cursor is (`CursorPosition`), as VNC's cursor
    /// pseudo-encoding and SPICE's cursor channel send it.
    CursorShape {
        vm: u32,
        width: u32,
        height: u32,
        hot_x: u32,
        hot_y: u32,
        pixels: Vec<u8>,
    },
    /// Server: where the VM's cursor is now, the image's top left pixel at (`x`, `y`) of
    /// the screen (either may be negative, or past the screen's size), or that it is
    /// hidden (`visible` false: `x` and `y` then mean nothing). Moving the cursor sends
    /// this, and no `ScreenFrame`.
    CursorPosition {
        vm: u32,
        visible: bool,
        x: i32,
        y: i32,
    },

    // ---- Block I/O (Lake) ----
    /// Attach a disk image. Answered by `BlockOpened` with a handle for the requests
    /// below. One writer or any number of read-only attachments at a time.
    BlockOpen {
        name: String,
        read_only: bool,
    },
    BlockOpened {
        handle: u32,
        sectors: u64,
        read_only: bool,
    },
    /// Read `count` sectors (1 to [`MAX_BLOCK_SECTORS`]). Answered by `BlockData`.
    BlockRead {
        handle: u32,
        sector: u64,
        count: u32,
    },
    BlockData {
        handle: u32,
        sector: u64,
        data: Vec<u8>,
    },
    /// Write whole sectors. Answered by `Ok`.
    BlockWrite {
        handle: u32,
        sector: u64,
        data: Vec<u8>,
    },
    /// Make earlier writes durable. Answered by `Ok`.
    BlockFlush {
        handle: u32,
    },
    /// Answered by `BlockCapacity`.
    BlockSize {
        handle: u32,
    },
    BlockCapacity {
        handle: u32,
        sectors: u64,
    },
    /// Detach. Answered by `Ok`.
    BlockClose {
        handle: u32,
    },

    // ---- Disk management (Lake) ----
    /// Create an image: zero-filled, or a copy of the image `source` (version 7).
    /// A copy is `sectors` long, zeros after the source's end, or as long as its
    /// source when `sectors` is 0. Answered by `Ok`.
    DiskCreate {
        name: String,
        sectors: u64,
        source: Option<String>,
    },
    /// Answered by `DiskList`.
    ListDisks,
    DiskList {
        disks: Vec<DiskInfo>,
    },
    /// Delete an image that nobody has attached. Answered by `Ok`.
    DiskDelete {
        name: String,
    },

    // ---- Virtual network (Avalon) ----
    /// Create a virtual switch with no cards plugged in, and a NAT gateway if `nat`
    /// asks for one. Answered by `Ok`.
    CreateSwitch {
        name: String,
        nat: Option<NatConfig>,
    },
    /// Delete a switch, unplugging every card from it. Answered by `Ok`.
    DeleteSwitch {
        name: String,
    },
    /// Answered by `SwitchList`.
    ListSwitches,
    SwitchList {
        switches: Vec<SwitchInfo>,
    },
    /// Plug card `card` of VM `vm` into switch `switch`. Answered by `Ok`.
    AttachCard {
        vm: VmRef,
        card: u8,
        switch: String,
    },
    /// Unplug card `card` of VM `vm` from its switch. Answered by `Ok`.
    DetachCard {
        vm: VmRef,
        card: u8,
    },
    /// Start capturing every frame that enters switch `switch` into the pcap file
    /// `path` on the server's host, or stop (`None`). Answered by `Ok`.
    CaptureSwitch {
        switch: String,
        path: Option<String>,
    },
    /// Give switch `switch` a NAT gateway, or take its gateway away (`None`). Answered
    /// by `Ok`.
    SetSwitchNat {
        switch: String,
        nat: Option<NatConfig>,
    },

    // ---- Debugging (Avalon) ----
    /// Attach a debugger to a VM's GDB stub: the VM stops for it (state debug).
    /// Answered by `DebugAttached`; the GDB remote serial protocol then travels in
    /// `DebugInput` and `DebugOutput`.
    DebugAttach {
        vm: VmRef,
    },
    DebugAttached {
        vm: u32,
    },
    /// Client: bytes of the GDB remote serial protocol for the VM's stub.
    DebugInput {
        vm: u32,
        data: Vec<u8>,
    },
    /// Server: bytes of the GDB remote serial protocol from the VM's stub.
    DebugOutput {
        vm: u32,
        data: Vec<u8>,
    },
    /// Answered by `Ok`; the VM runs on without breakpoints, as after GDB's `D`.
    DebugDetach {
        vm: u32,
    },
}

impl Message {
    /// The message's type code (see [`ty`]).
    pub fn type_code(&self) -> u16 {
        use Message::*;
        match self {
            Hello { .. } => ty::HELLO,
            Welcome { .. } => ty::WELCOME,
            Error { .. } => ty::ERROR,
            Ok => ty::OK,
            Ping { .. } => ty::PING,
            Pong { .. } => ty::PONG,
            CreateVm { .. } => ty::CREATE_VM,
            VmCreated { .. } => ty::VM_CREATED,
            ListVms => ty::LIST_VMS,
            VmList { .. } => ty::VM_LIST,
            GetVmInfo { .. } => ty::GET_VM_INFO,
            VmInfo(_) => ty::VM_INFO,
            StartVm { .. } => ty::START_VM,
            PauseVm { .. } => ty::PAUSE_VM,
            ResumeVm { .. } => ty::RESUME_VM,
            StopVm { .. } => ty::STOP_VM,
            DestroyVm { .. } => ty::DESTROY_VM,
            VmStateChanged { .. } => ty::VM_STATE_CHANGED,
            Detached { .. } => ty::DETACHED,
            ConsoleAttach { .. } => ty::CONSOLE_ATTACH,
            ConsoleAttached { .. } => ty::CONSOLE_ATTACHED,
            ConsoleInput { .. } => ty::CONSOLE_INPUT,
            ConsoleOutput { .. } => ty::CONSOLE_OUTPUT,
            ConsoleDetach { .. } => ty::CONSOLE_DETACH,
            ScreenAttach { .. } => ty::SCREEN_ATTACH,
            ScreenAttached { .. } => ty::SCREEN_ATTACHED,
            ScreenFrame { .. } => ty::SCREEN_FRAME,
            KeyInput { .. } => ty::KEY_INPUT,
            ScreenDetach { .. } => ty::SCREEN_DETACH,
            PointerInput { .. } => ty::POINTER_INPUT,
            ScreenResized { .. } => ty::SCREEN_RESIZED,
            CursorShape { .. } => ty::CURSOR_SHAPE,
            CursorPosition { .. } => ty::CURSOR_POSITION,
            BlockOpen { .. } => ty::BLOCK_OPEN,
            BlockOpened { .. } => ty::BLOCK_OPENED,
            BlockRead { .. } => ty::BLOCK_READ,
            BlockData { .. } => ty::BLOCK_DATA,
            BlockWrite { .. } => ty::BLOCK_WRITE,
            BlockFlush { .. } => ty::BLOCK_FLUSH,
            BlockSize { .. } => ty::BLOCK_SIZE,
            BlockCapacity { .. } => ty::BLOCK_CAPACITY,
            BlockClose { .. } => ty::BLOCK_CLOSE,
            DiskCreate { .. } => ty::DISK_CREATE,
            ListDisks => ty::LIST_DISKS,
            DiskList { .. } => ty::DISK_LIST,
            DiskDelete { .. } => ty::DISK_DELETE,
            CreateSwitch { .. } => ty::CREATE_SWITCH,
            DeleteSwitch { .. } => ty::DELETE_SWITCH,
            ListSwitches => ty::LIST_SWITCHES,
            SwitchList { .. } => ty::SWITCH_LIST,
            AttachCard { .. } => ty::ATTACH_CARD,
            DetachCard { .. } => ty::DETACH_CARD,
            CaptureSwitch { .. } => ty::CAPTURE_SWITCH,
            SetSwitchNat { .. } => ty::SET_SWITCH_NAT,
            DebugAttach { .. } => ty::DEBUG_ATTACH,
            DebugAttached { .. } => ty::DEBUG_ATTACHED,
            DebugInput { .. } => ty::DEBUG_INPUT,
            DebugOutput { .. } => ty::DEBUG_OUTPUT,
            DebugDetach { .. } => ty::DEBUG_DETACH,
        }
    }

    pub fn kind(&self) -> MessageKind {
        kind_of(self.type_code()).expect("every message is catalogued")
    }

    pub fn name(&self) -> &'static str {
        let code = self.type_code();
        CATALOGUE
            .iter()
            .find(|(c, _, _)| *c == code)
            .expect("every message is catalogued")
            .1
    }

    /// Checks the rules that the field types alone do not enforce. Both [`encode`]
    /// (so a sender cannot produce a frame the receiver would reject) and
    /// [`decode`] apply it.
    ///
    /// [`encode`]: crate::encode
    /// [`decode`]: crate::decode
    pub fn check(&self) -> Result<()> {
        let invalid = |why: String| Err(Error::Invalid(why));
        match self {
            Message::ScreenFrame {
                width,
                height,
                pixels,
                ..
            } => {
                if *width == 0 || *height == 0 {
                    return invalid(format!("empty {width}x{height} screen rectangle"));
                }
                // u128: 4 * (2^32 - 1)^2 does not fit in 64 bits.
                let want = *width as u128 * *height as u128 * 4;
                if pixels.len() as u128 != want {
                    return invalid(format!(
                        "{width}x{height} rectangle needs {want} pixel bytes, got {}",
                        pixels.len()
                    ));
                }
            }
            Message::CursorShape {
                width,
                height,
                hot_x,
                hot_y,
                pixels,
                ..
            } => {
                let size = 1..=MAX_CURSOR_SIZE;
                if !size.contains(width) || !size.contains(height) {
                    return invalid(format!(
                        "a {width}x{height} cursor (1 to {MAX_CURSOR_SIZE} pixels each way)"
                    ));
                }
                if hot_x >= width || hot_y >= height {
                    return invalid(format!(
                        "hotspot ({hot_x}, {hot_y}) outside a {width}x{height} cursor"
                    ));
                }
                let want = *width as usize * *height as usize * 4;
                if pixels.len() != want {
                    return invalid(format!(
                        "{width}x{height} cursor needs {want} pixel bytes, got {}",
                        pixels.len()
                    ));
                }
            }
            Message::Error {
                code: ErrorCode::Other(n),
                ..
            } if ErrorCode::from_u16(*n) != ErrorCode::Other(*n) => {
                return invalid(format!(
                    "error code Other({n}) is {:?}; use the named code",
                    ErrorCode::from_u16(*n)
                ));
            }
            Message::KeyInput { key, .. } if key.code == 0 && key.usage == 0 => {
                return invalid("key with code 0 and usage 0".into())
            }
            Message::KeyInput { key, .. } if key.repeat && key.release => {
                return invalid("a repeat is a press, not a release".into())
            }
            Message::PointerInput { pointer, .. } if pointer.buttons & !POINTER_BUTTONS != 0 => {
                return invalid(format!("reserved pointer buttons {:#04x}", pointer.buttons));
            }
            Message::BlockRead { count, .. } if !(1..=MAX_BLOCK_SECTORS).contains(count) => {
                return invalid(format!(
                    "block read of {count} sectors (1 to {MAX_BLOCK_SECTORS})"
                ));
            }
            Message::CreateSwitch { nat: Some(nat), .. } | Message::SetSwitchNat { nat: Some(nat), .. } => {
                check_forwards(&nat.hostfwd)?
            }
            Message::SwitchList { switches } => {
                for nat in switches.iter().filter_map(|s| s.nat.as_ref()) {
                    check_forwards(&nat.hostfwd)?;
                }
            }
            Message::BlockData { data, .. } | Message::BlockWrite { data, .. } => {
                let max = MAX_BLOCK_SECTORS as usize * SECTOR_SIZE;
                if data.is_empty() || data.len() % SECTOR_SIZE != 0 || data.len() > max {
                    return invalid(format!(
                        "block data of {} bytes (whole {SECTOR_SIZE}-byte sectors, at most {max} bytes)",
                        data.len()
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Appends the payload (everything after the type code) to `w`.
    pub(crate) fn put(&self, w: &mut Writer) {
        use Message::*;
        match self {
            Hello { version, agent } => {
                w.raw(&MAGIC);
                w.u16(*version);
                w.str(agent);
            }
            Welcome { version, server } => {
                w.u16(*version);
                w.str(server);
            }
            Error { code, message } => {
                w.u16(code.to_u16());
                w.str(message);
            }
            Ok | ListVms | ListDisks | ListSwitches => {}
            Ping { token } | Pong { token } => w.u32(*token),
            CreateVm {
                name,
                profile,
                program,
            } => {
                w.str(name);
                put_profile(w, profile);
                w.bytes(program);
            }
            VmCreated { id } => w.u32(*id),
            VmList { vms } => w.list(vms, put_summary),
            GetVmInfo { vm }
            | StartVm { vm }
            | PauseVm { vm }
            | ResumeVm { vm }
            | StopVm { vm }
            | DestroyVm { vm }
            | ConsoleAttach { vm }
            | ScreenAttach { vm }
            | DebugAttach { vm } => put_vm_ref(w, vm),
            VmInfo(d) => {
                put_summary(w, &d.vm);
                w.list(&d.devices, |w, e| {
                    w.u32(e.kind);
                    w.u32(e.base);
                    w.u32(e.size);
                    w.u32(e.irq);
                });
                w.u64(d.steps);
            }
            VmStateChanged { vm, state } => {
                w.u32(*vm);
                put_state(w, state);
            }
            Detached { vm, kind } => {
                w.u32(*vm);
                w.u8(match kind {
                    AttachKind::Console => 1,
                    AttachKind::Screen => 2,
                    AttachKind::Debug => 3,
                });
            }
            ConsoleAttached { vm, scrollback: data }
            | ConsoleInput { vm, data }
            | ConsoleOutput { vm, data }
            | DebugInput { vm, data }
            | DebugOutput { vm, data } => {
                w.u32(*vm);
                w.bytes(data);
            }
            ConsoleDetach { vm } | ScreenDetach { vm } | DebugAttached { vm } | DebugDetach { vm } => {
                w.u32(*vm)
            }
            ScreenAttached {
                vm,
                width,
                height,
                format,
            }
            | ScreenResized {
                vm,
                width,
                height,
                format,
            } => {
                w.u32(*vm);
                w.u32(*width);
                w.u32(*height);
                w.u32(*format);
            }
            ScreenFrame {
                vm,
                x,
                y,
                width,
                height,
                pixels,
            } => {
                w.u32(*vm);
                w.u32(*x);
                w.u32(*y);
                w.u32(*width);
                w.u32(*height);
                w.bytes(pixels);
            }
            CursorShape {
                vm,
                width,
                height,
                hot_x,
                hot_y,
                pixels,
            } => {
                w.u32(*vm);
                w.u32(*width);
                w.u32(*height);
                w.u32(*hot_x);
                w.u32(*hot_y);
                w.bytes(pixels);
            }
            CursorPosition { vm, visible, x, y } => {
                w.u32(*vm);
                w.bool(*visible);
                w.i32(*x);
                w.i32(*y);
            }
            KeyInput { vm, key } => {
                w.u32(*vm);
                w.u8(key.code);
                let mut flags = 0;
                for (on, bit) in [
                    (key.release, KEY_RELEASE),
                    (key.shift, KEY_SHIFT),
                    (key.ctrl, KEY_CTRL),
                    (key.repeat, KEY_REPEAT),
                ] {
                    if on {
                        flags |= bit;
                    }
                }
                w.u8(flags);
                w.u16(key.usage);
            }
            PointerInput { vm, pointer } => {
                w.u32(*vm);
                match pointer.motion {
                    PointerMotion::None => w.u8(0),
                    PointerMotion::To { x, y } => {
                        w.u8(1);
                        w.u32(x);
                        w.u32(y);
                    }
                    PointerMotion::By { dx, dy } => {
                        w.u8(2);
                        w.i32(dx);
                        w.i32(dy);
                    }
                }
                w.u8(pointer.buttons);
                w.i8(pointer.wheel);
            }
            BlockOpen { name, read_only } => {
                w.str(name);
                w.bool(*read_only);
            }
            BlockOpened {
                handle,
                sectors,
                read_only,
            } => {
                w.u32(*handle);
                w.u64(*sectors);
                w.bool(*read_only);
            }
            BlockRead {
                handle,
                sector,
                count,
            } => {
                w.u32(*handle);
                w.u64(*sector);
                w.u32(*count);
            }
            BlockData { handle, sector, data } | BlockWrite { handle, sector, data } => {
                w.u32(*handle);
                w.u64(*sector);
                w.bytes(data);
            }
            BlockFlush { handle } | BlockSize { handle } | BlockClose { handle } => w.u32(*handle),
            BlockCapacity { handle, sectors } => {
                w.u32(*handle);
                w.u64(*sectors);
            }
            DiskCreate {
                name,
                sectors,
                source,
            } => {
                w.str(name);
                w.u64(*sectors);
                w.option(source, |w, s| w.str(s));
            }
            DiskList { disks } => w.list(disks, |w, d| {
                w.str(&d.name);
                w.u64(d.sectors);
                w.u32(d.readers);
                w.bool(d.writer);
            }),
            DiskDelete { name } | DeleteSwitch { name } => w.str(name),
            CreateSwitch { name, nat } => {
                w.str(name);
                w.option(nat, put_nat_config);
            }
            SetSwitchNat { switch, nat } => {
                w.str(switch);
                w.option(nat, put_nat_config);
            }
            SwitchList { switches } => w.list(switches, |w, s| {
                w.str(&s.name);
                w.option(&s.capture, |w, path| w.str(path));
                w.option(&s.nat, put_nat_info);
                w.list(&s.ports, put_port);
            }),
            AttachCard { vm, card, switch } => {
                put_vm_ref(w, vm);
                w.u8(*card);
                w.str(switch);
            }
            DetachCard { vm, card } => {
                put_vm_ref(w, vm);
                w.u8(*card);
            }
            CaptureSwitch { switch, path } => {
                w.str(switch);
                w.option(path, |w, path| w.str(path));
            }
        }
    }

    /// Decodes the payload of a message of type `code`: the whole payload must be used,
    /// and the result must pass [`Message::check`].
    pub(crate) fn get(code: u16, payload: &[u8]) -> Result<Message> {
        use Message::*;
        let mut reader = Reader::new(payload);
        let r = &mut reader;
        let msg = match code {
            ty::HELLO => {
                let magic = r.magic()?;
                if magic != MAGIC {
                    return Err(crate::Error::BadMagic(magic));
                }
                Hello {
                    version: r.u16()?,
                    agent: r.string()?,
                }
            }
            ty::WELCOME => Welcome {
                version: r.u16()?,
                server: r.string()?,
            },
            ty::ERROR => Error {
                code: ErrorCode::from_u16(r.u16()?),
                message: r.string()?,
            },
            ty::OK => Ok,
            ty::PING => Ping { token: r.u32()? },
            ty::PONG => Pong { token: r.u32()? },
            ty::CREATE_VM => CreateVm {
                name: r.string()?,
                profile: get_profile(r)?,
                program: r.bytes()?,
            },
            ty::VM_CREATED => VmCreated { id: r.u32()? },
            ty::LIST_VMS => ListVms,
            ty::VM_LIST => VmList {
                vms: r.list(get_summary)?,
            },
            ty::GET_VM_INFO => GetVmInfo { vm: get_vm_ref(r)? },
            ty::VM_INFO => VmInfo(VmDetails {
                vm: get_summary(r)?,
                devices: r.list(|r| {
                    Result::Ok(DeviceEntry {
                        kind: r.u32()?,
                        base: r.u32()?,
                        size: r.u32()?,
                        irq: r.u32()?,
                    })
                })?,
                steps: r.u64()?,
            }),
            ty::START_VM => StartVm { vm: get_vm_ref(r)? },
            ty::PAUSE_VM => PauseVm { vm: get_vm_ref(r)? },
            ty::RESUME_VM => ResumeVm { vm: get_vm_ref(r)? },
            ty::STOP_VM => StopVm { vm: get_vm_ref(r)? },
            ty::DESTROY_VM => DestroyVm { vm: get_vm_ref(r)? },
            ty::VM_STATE_CHANGED => VmStateChanged {
                vm: r.u32()?,
                state: get_state(r)?,
            },
            ty::DETACHED => Detached {
                vm: r.u32()?,
                kind: match r.u8()? {
                    1 => AttachKind::Console,
                    2 => AttachKind::Screen,
                    3 => AttachKind::Debug,
                    t => return Err(bad_tag("attachment kind", t)),
                },
            },
            ty::CONSOLE_ATTACH => ConsoleAttach { vm: get_vm_ref(r)? },
            ty::CONSOLE_ATTACHED => ConsoleAttached {
                vm: r.u32()?,
                scrollback: r.bytes()?,
            },
            ty::CONSOLE_INPUT => ConsoleInput {
                vm: r.u32()?,
                data: r.bytes()?,
            },
            ty::CONSOLE_OUTPUT => ConsoleOutput {
                vm: r.u32()?,
                data: r.bytes()?,
            },
            ty::CONSOLE_DETACH => ConsoleDetach { vm: r.u32()? },
            ty::DEBUG_ATTACH => DebugAttach { vm: get_vm_ref(r)? },
            ty::DEBUG_ATTACHED => DebugAttached { vm: r.u32()? },
            ty::DEBUG_INPUT => DebugInput {
                vm: r.u32()?,
                data: r.bytes()?,
            },
            ty::DEBUG_OUTPUT => DebugOutput {
                vm: r.u32()?,
                data: r.bytes()?,
            },
            ty::DEBUG_DETACH => DebugDetach { vm: r.u32()? },
            ty::SCREEN_ATTACH => ScreenAttach { vm: get_vm_ref(r)? },
            ty::SCREEN_ATTACHED => ScreenAttached {
                vm: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
                format: r.u32()?,
            },
            ty::SCREEN_RESIZED => ScreenResized {
                vm: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
                format: r.u32()?,
            },
            ty::SCREEN_FRAME => ScreenFrame {
                vm: r.u32()?,
                x: r.u32()?,
                y: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
                pixels: r.bytes()?,
            },
            ty::CURSOR_SHAPE => CursorShape {
                vm: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
                hot_x: r.u32()?,
                hot_y: r.u32()?,
                pixels: r.bytes()?,
            },
            ty::CURSOR_POSITION => CursorPosition {
                vm: r.u32()?,
                visible: r.bool()?,
                x: r.i32()?,
                y: r.i32()?,
            },
            ty::KEY_INPUT => {
                let vm = r.u32()?;
                let code = r.u8()?;
                let flags = r.u8()?;
                if flags & !KEY_FLAGS != 0 {
                    return Err(crate::Error::Invalid(format!("reserved key flags {flags:#04x}")));
                }
                KeyInput {
                    vm,
                    key: KeyEvent {
                        code,
                        release: flags & KEY_RELEASE != 0,
                        shift: flags & KEY_SHIFT != 0,
                        ctrl: flags & KEY_CTRL != 0,
                        repeat: flags & KEY_REPEAT != 0,
                        usage: r.u16()?,
                    },
                }
            }
            ty::SCREEN_DETACH => ScreenDetach { vm: r.u32()? },
            ty::POINTER_INPUT => PointerInput {
                vm: r.u32()?,
                pointer: PointerChange {
                    motion: match r.u8()? {
                        0 => PointerMotion::None,
                        1 => PointerMotion::To {
                            x: r.u32()?,
                            y: r.u32()?,
                        },
                        2 => PointerMotion::By {
                            dx: r.i32()?,
                            dy: r.i32()?,
                        },
                        t => return Err(bad_tag("pointer motion", t)),
                    },
                    buttons: r.u8()?,
                    wheel: r.i8()?,
                },
            },
            ty::BLOCK_OPEN => BlockOpen {
                name: r.string()?,
                read_only: r.bool()?,
            },
            ty::BLOCK_OPENED => BlockOpened {
                handle: r.u32()?,
                sectors: r.u64()?,
                read_only: r.bool()?,
            },
            ty::BLOCK_READ => BlockRead {
                handle: r.u32()?,
                sector: r.u64()?,
                count: r.u32()?,
            },
            ty::BLOCK_DATA => BlockData {
                handle: r.u32()?,
                sector: r.u64()?,
                data: r.bytes()?,
            },
            ty::BLOCK_WRITE => BlockWrite {
                handle: r.u32()?,
                sector: r.u64()?,
                data: r.bytes()?,
            },
            ty::BLOCK_FLUSH => BlockFlush { handle: r.u32()? },
            ty::BLOCK_SIZE => BlockSize { handle: r.u32()? },
            ty::BLOCK_CAPACITY => BlockCapacity {
                handle: r.u32()?,
                sectors: r.u64()?,
            },
            ty::BLOCK_CLOSE => BlockClose { handle: r.u32()? },
            ty::DISK_CREATE => DiskCreate {
                name: r.string()?,
                sectors: r.u64()?,
                source: r.option(|r| r.string())?,
            },
            ty::LIST_DISKS => ListDisks,
            ty::DISK_LIST => DiskList {
                disks: r.list(|r| {
                    Result::Ok(DiskInfo {
                        name: r.string()?,
                        sectors: r.u64()?,
                        readers: r.u32()?,
                        writer: r.bool()?,
                    })
                })?,
            },
            ty::DISK_DELETE => DiskDelete { name: r.string()? },
            ty::CREATE_SWITCH => CreateSwitch {
                name: r.string()?,
                nat: r.option(get_nat_config)?,
            },
            ty::DELETE_SWITCH => DeleteSwitch { name: r.string()? },
            ty::LIST_SWITCHES => ListSwitches,
            ty::SWITCH_LIST => SwitchList {
                switches: r.list(|r| {
                    Result::Ok(SwitchInfo {
                        name: r.string()?,
                        capture: r.option(|r| r.string())?,
                        nat: r.option(get_nat_info)?,
                        ports: r.list(get_port)?,
                    })
                })?,
            },
            ty::ATTACH_CARD => AttachCard {
                vm: get_vm_ref(r)?,
                card: r.u8()?,
                switch: r.string()?,
            },
            ty::DETACH_CARD => DetachCard {
                vm: get_vm_ref(r)?,
                card: r.u8()?,
            },
            ty::CAPTURE_SWITCH => CaptureSwitch {
                switch: r.string()?,
                path: r.option(|r| r.string())?,
            },
            ty::SET_SWITCH_NAT => SetSwitchNat {
                switch: r.string()?,
                nat: r.option(get_nat_config)?,
            },
            _ => return Err(crate::Error::UnknownType(code)),
        };
        reader.finish()?;
        msg.check()?;
        Result::Ok(msg)
    }
}

fn bad_tag(what: &str, tag: u8) -> Error {
    Error::Invalid(format!("unknown {what} tag {tag}"))
}

fn put_vm_ref(w: &mut Writer, vm: &VmRef) {
    match vm {
        VmRef::Id(id) => {
            w.u8(0);
            w.u32(*id);
        }
        VmRef::Name(name) => {
            w.u8(1);
            w.str(name);
        }
    }
}

fn get_vm_ref(r: &mut Reader) -> Result<VmRef> {
    match r.u8()? {
        0 => Ok(VmRef::Id(r.u32()?)),
        1 => Ok(VmRef::Name(r.string()?)),
        t => Err(bad_tag("VM reference", t)),
    }
}

fn put_state(w: &mut Writer, state: &VmState) {
    match state {
        VmState::Created => w.u8(0),
        VmState::Running => w.u8(1),
        VmState::Paused => w.u8(2),
        VmState::Stopped => w.u8(3),
        VmState::Exited(code) => {
            w.u8(4);
            w.u32(*code);
        }
        VmState::Debug => w.u8(5),
    }
}

fn get_state(r: &mut Reader) -> Result<VmState> {
    Ok(match r.u8()? {
        0 => VmState::Created,
        1 => VmState::Running,
        2 => VmState::Paused,
        3 => VmState::Stopped,
        4 => VmState::Exited(r.u32()?),
        5 => VmState::Debug,
        t => return Err(bad_tag("VM state", t)),
    })
}

fn put_profile(w: &mut Writer, p: &HardwareProfile) {
    w.u64(p.ram_size);
    w.option(&p.clock_hz, |w, hz| w.u32(*hz));
    w.u32(p.nominal_hz);
    w.u32(p.harts);
    w.u8(p.paging.satp_mode());
    match &p.bios {
        Bios::None => w.u8(0),
        Bios::Default => w.u8(1),
        Bios::File(path) => {
            w.u8(2);
            w.str(path);
        }
    }
    w.list(&p.devices, |w, d| match d {
        DeviceSpec::Disk(disk) => {
            w.u8(1);
            match &disk.source {
                DiskSource::File(path) => {
                    w.u8(1);
                    w.str(path);
                }
                DiskSource::Lake { addr, name } => {
                    w.u8(2);
                    w.str(addr);
                    w.str(name);
                }
            }
            w.bool(disk.read_only);
        }
        DeviceSpec::Net(net) => {
            w.u8(2);
            w.option(&net.switch, |w, switch| w.str(switch));
            w.option(&net.mac, |w, mac| w.raw(mac));
        }
    });
}

fn get_profile(r: &mut Reader) -> Result<HardwareProfile> {
    Ok(HardwareProfile {
        ram_size: r.u64()?,
        clock_hz: r.option(|r| r.u32())?,
        nominal_hz: r.u32()?,
        harts: r.u32()?,
        paging: {
            let mode = r.u8()?;
            Paging::from_satp_mode(mode)
                .ok_or_else(|| Error::Invalid(format!("paging mode {mode}, not 9 (Sv48) or 8 (Sv39)")))?
        },
        bios: match r.u8()? {
            0 => Bios::None,
            1 => Bios::Default,
            2 => Bios::File(r.string()?),
            t => return Err(bad_tag("firmware", t)),
        },
        devices: r.list(|r| match r.u8()? {
            1 => {
                let source = match r.u8()? {
                    1 => DiskSource::File(r.string()?),
                    2 => DiskSource::Lake {
                        addr: r.string()?,
                        name: r.string()?,
                    },
                    t => return Err(bad_tag("disk source", t)),
                };
                Ok(DeviceSpec::Disk(DiskSpec {
                    source,
                    read_only: r.bool()?,
                }))
            }
            2 => Ok(DeviceSpec::Net(NetSpec {
                switch: r.option(|r| r.string())?,
                mac: r.option(|r| r.mac())?,
            })),
            t => Err(bad_tag("device", t)),
        })?,
    })
}

/// Refuses forwarding rules no gateway can have: more than [`MAX_FORWARDS`] of them (each
/// binds a host socket), a guest port 0, or a guest address outside the guest network.
fn check_forwards(rules: &[PortForward]) -> Result<()> {
    if rules.len() > MAX_FORWARDS {
        return Err(Error::Invalid(format!(
            "{} forwarding rules (at most {MAX_FORWARDS} per gateway)",
            rules.len()
        )));
    }
    for rule in rules {
        if rule.guest.port() == 0 {
            return Err(Error::Invalid(format!(
                "forwarding rule {rule}: the guest port is 0"
            )));
        }
        if !is_guest_addr(*rule.guest.ip()) {
            return Err(Error::Invalid(format!(
                "forwarding rule {rule}: {} is not a guest address in 10.0.2.0/24",
                rule.guest.ip()
            )));
        }
    }
    Ok(())
}

fn put_endpoint(w: &mut Writer, addr: &SocketAddrV4) {
    w.raw(&addr.ip().octets());
    w.u16(addr.port());
}

fn get_endpoint(r: &mut Reader) -> Result<SocketAddrV4> {
    let ip: [u8; 4] = r.array()?;
    Ok(SocketAddrV4::new(Ipv4Addr::from(ip), r.u16()?))
}

fn put_forward(w: &mut Writer, rule: &PortForward) {
    w.u8(match rule.proto {
        FwdProto::Tcp => 1,
        FwdProto::Udp => 2,
    });
    put_endpoint(w, &rule.host);
    put_endpoint(w, &rule.guest);
}

fn get_forward(r: &mut Reader) -> Result<PortForward> {
    let proto = match r.u8()? {
        1 => FwdProto::Tcp,
        2 => FwdProto::Udp,
        t => return Err(bad_tag("forwarding protocol", t)),
    };
    Ok(PortForward {
        proto,
        host: get_endpoint(r)?,
        guest: get_endpoint(r)?,
    })
}

fn put_nat_config(w: &mut Writer, nat: &NatConfig) {
    w.list(&nat.hostfwd, put_forward);
}

fn get_nat_config(r: &mut Reader) -> Result<NatConfig> {
    Ok(NatConfig {
        hostfwd: r.list(get_forward)?,
    })
}

fn put_nat_info(w: &mut Writer, nat: &NatInfo) {
    w.list(&nat.hostfwd, put_forward);
    w.list(&nat.leases, |w, lease| {
        w.raw(&lease.mac);
        w.raw(&lease.addr.octets());
    });
}

fn get_nat_info(r: &mut Reader) -> Result<NatInfo> {
    Ok(NatInfo {
        hostfwd: r.list(get_forward)?,
        leases: r.list(|r| {
            Ok(Lease {
                mac: r.mac()?,
                addr: Ipv4Addr::from(r.array::<4>()?),
            })
        })?,
    })
}

fn put_port(w: &mut Writer, p: &SwitchPort) {
    match p.owner {
        PortOwner::Card { vm, card } => {
            w.u8(1);
            w.u32(vm);
            w.u8(card);
        }
        PortOwner::Gateway => w.u8(2),
    }
    w.raw(&p.mac);
    for n in [p.in_frames, p.in_bytes, p.out_frames, p.out_bytes, p.dropped] {
        w.u64(n);
    }
}

fn get_port(r: &mut Reader) -> Result<SwitchPort> {
    let owner = match r.u8()? {
        1 => PortOwner::Card {
            vm: r.u32()?,
            card: r.u8()?,
        },
        2 => PortOwner::Gateway,
        t => return Err(bad_tag("port owner", t)),
    };
    Ok(SwitchPort {
        owner,
        mac: r.mac()?,
        in_frames: r.u64()?,
        in_bytes: r.u64()?,
        out_frames: r.u64()?,
        out_bytes: r.u64()?,
        dropped: r.u64()?,
    })
}

fn put_summary(w: &mut Writer, s: &VmSummary) {
    w.u32(s.id);
    w.str(&s.name);
    put_state(w, &s.state);
    put_profile(w, &s.profile);
}

fn get_summary(r: &mut Reader) -> Result<VmSummary> {
    Ok(VmSummary {
        id: r.u32()?,
        name: r.string()?,
        state: get_state(r)?,
        profile: get_profile(r)?,
    })
}
