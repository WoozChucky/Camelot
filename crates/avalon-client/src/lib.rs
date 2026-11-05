//! The client side of the Avalon platform: the operations a person orders, over the
//! Avalon protocol (`docs/avalon-protocol.md`).
//!
//! - [`Client`]: one session with a server. VM control (create, list, info, start,
//!   pause, resume, stop, destroy) and the virtual network (switches, plugging cards
//!   into them, captures) talk to Avalon; disk management (create, list, delete) talks
//!   to Lake. Each operation is one request and its typed reply.
//! - [`parse`]: the text people type for addresses, RAM sizes, clocks, paging
//!   modes, disk sources, disk sizes and network cards, so every front end accepts
//!   the same syntax (`--ram`, `--clock` and `--paging` are Grail's own parsers).
//! - [`show`]: the same values as text again (sizes, frequencies, the architecture,
//!   device names, MAC addresses).
//! - [`VmAction::allowed_in`], [`holds_disks`] and [`lake_disks`]: what a VM's state
//!   and profile mean for what a person may do with it and its disks.
//! - [`create_camelot_vm`]: a VM that boots Camelot from its own copy of Lake's
//!   base image ([`BASE_IMAGE`]), with no program to upload.
//! - [`DebugTunnel`]: a debugger's end of a VM's GDB stub, the GDB remote serial
//!   protocol's bytes over a session (merlin-dbg's `--vm`).
//!
//! Round Table, the command-line client, is a thin layer over this crate, and so is
//! Pendragon, the web console: the two cannot drift apart.

use avalon_proto::{
    validate_name, Bios, Connection, DeviceSpec, DiskInfo, DiskSource, DiskSpec, ErrorCode, HardwareProfile,
    Message, NatConfig, SwitchInfo, VmDetails, VmRef, VmState, VmSummary,
};
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

pub mod parse;
pub mod show;

pub use parse::{
    parse_addr, parse_clock, parse_disk_size, parse_disk_source, parse_harts, parse_hostfwd, parse_net,
    parse_paging, parse_ram,
};
pub use show::{arch_text, device_name, disk_source_text, format_hz, format_size, mac_text};

/// How long [`Client::connect`] tries each address of the server before giving up.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long [`Client::connect`]'s session waits for a reply. Creating a VM allocates
/// its RAM and opens its disks, which can take a moment; anything longer means the
/// server is stuck, and the client reports it rather than hang.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// The time limits of a session ([`Client::connect_with`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// How long to try each address of the server.
    pub connect: Duration,
    /// How long to wait for any reply, the server's `Welcome` included.
    pub reply: Duration,
}

impl Default for Timeouts {
    /// [`CONNECT_TIMEOUT`] and [`REPLY_TIMEOUT`].
    fn default() -> Self {
        Timeouts {
            connect: CONNECT_TIMEOUT,
            reply: REPLY_TIMEOUT,
        }
    }
}

/// What went wrong with an operation.
#[derive(Debug)]
pub enum Error {
    /// The server could not be reached, or the handshake failed (a different protocol
    /// version, something that is not an Avalon server).
    Connect {
        addr: String,
        cause: avalon_proto::Error,
    },
    /// The request failed: the server answered with an `Error` (see [`Error::code`]),
    /// or the connection broke.
    Request(avalon_proto::Error),
}

impl Error {
    /// The protocol error code, when the server refused the request, or refused the
    /// session itself during the handshake (a server that answers `Hello` with an
    /// `Error`, such as `Busy`).
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Error::Request(avalon_proto::Error::Remote { code, .. })
            | Error::Connect {
                cause: avalon_proto::Error::Remote { code, .. },
                ..
            } => Some(*code),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Connect { addr, cause } => write!(f, "cannot connect to {addr}: {cause}"),
            Error::Request(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<avalon_proto::Error> for Error {
    fn from(e: avalon_proto::Error) -> Self {
        Error::Request(e)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The life-cycle requests on one VM, each answered by `Ok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmAction {
    /// created → running
    Start,
    /// running → paused
    Pause,
    /// paused → running
    Resume,
    /// created, running or paused → stopped
    Stop,
    /// any state → gone
    Destroy,
}

impl VmAction {
    pub const ALL: [VmAction; 5] = [
        VmAction::Start,
        VmAction::Pause,
        VmAction::Resume,
        VmAction::Stop,
        VmAction::Destroy,
    ];

    /// The action a command word names: `start`, `pause`, `resume`, `stop`, `destroy`.
    pub fn parse(word: &str) -> Option<VmAction> {
        VmAction::ALL.into_iter().find(|a| a.name() == word)
    }

    /// The command word.
    pub fn name(self) -> &'static str {
        match self {
            VmAction::Start => "start",
            VmAction::Pause => "pause",
            VmAction::Resume => "resume",
            VmAction::Stop => "stop",
            VmAction::Destroy => "destroy",
        }
    }

    /// What happened, for telling a person: `started`, `paused`, ...
    pub fn done(self) -> &'static str {
        match self {
            VmAction::Start => "started",
            VmAction::Pause => "paused",
            VmAction::Resume => "resumed",
            VmAction::Stop => "stopped",
            VmAction::Destroy => "destroyed",
        }
    }

    /// Whether the server accepts this action on a VM in `state` (protocol doc,
    /// section 6.2): front ends offer only these. A VM its debugger holds (paused or
    /// debug) is the debugger's to resume, and refuses `Resume` all the same.
    pub fn allowed_in(self, state: VmState) -> bool {
        use VmState::*;
        match self {
            VmAction::Start => state == Created,
            VmAction::Pause => state == Running,
            VmAction::Resume => matches!(state, Paused | Debug),
            VmAction::Stop => matches!(state, Created | Running | Paused | Debug),
            VmAction::Destroy => true,
        }
    }

    /// The request for this action on `vm`.
    pub fn request(self, vm: VmRef) -> Message {
        match self {
            VmAction::Start => Message::StartVm { vm },
            VmAction::Pause => Message::PauseVm { vm },
            VmAction::Resume => Message::ResumeVm { vm },
            VmAction::Stop => Message::StopVm { vm },
            VmAction::Destroy => Message::DestroyVm { vm },
        }
    }
}

/// Whether a VM in `state` still has its machine, and so holds its disks: a VM's disks
/// are attached when it is created and released when its machine ends, when it is
/// stopped or its program exits (protocol doc, section 6.6). A VM its debugger holds
/// (state debug) has its machine.
pub fn holds_disks(state: VmState) -> bool {
    matches!(
        state,
        VmState::Created | VmState::Running | VmState::Paused | VmState::Debug
    )
}

/// The Lake images in a hardware profile, as (server address, image name, read-only),
/// in the profile's order.
pub fn lake_disks(profile: &HardwareProfile) -> impl Iterator<Item = (&str, &str, bool)> {
    profile.devices.iter().filter_map(|device| match device {
        DeviceSpec::Disk(disk) => match &disk.source {
            DiskSource::Lake { addr, name } => Some((addr.as_str(), name.as_str(), disk.read_only)),
            DiskSource::File(_) => None,
        },
        DeviceSpec::Net(_) => None,
    })
}

/// The Lake image a Camelot VM's disk is a copy of: a bootable Camelot root
/// filesystem, the one `cargo xtask fs` builds. `cargo xtask up` puts it in Lake's
/// images directory. A neutral name, not the OS's: it is the base image of the VMs.
pub const BASE_IMAGE: &str = "os-base";

/// The firmware a Camelot VM boots unless its creator says otherwise: Camelot's own
/// (`Bios::Default`, as QEMU's `virt` runs OpenSBI by default), since the root
/// image's bootloader and kernel run in S-mode on an SBI.
pub fn camelot_bios() -> Bios {
    Bios::Default
}

/// The RAM a Camelot VM has unless its creator says otherwise: the platform's
/// default, 4 MiB ([`avalon_proto::DEFAULT_RAM_SIZE`]), and the 2 MiB its firmware
/// keeps at the start of RAM on top (Excalibur spec section 7, "Firmware"). Less
/// does not boot: Grail puts the device tree at the last 2 MiB boundary below the
/// end of RAM, as QEMU does, which must be above the kernel's boot block at
/// 0x8020_0000.
pub const CAMELOT_RAM: u64 = avalon_proto::DEFAULT_RAM_SIZE + CAMELOT_FIRMWARE_RAM;

/// The RAM Camelot's firmware keeps for itself: 0x8000_0000 to 0x801f_ffff
/// (`grail::firmware::FW_SIZE`).
pub const CAMELOT_FIRMWARE_RAM: u64 = 2 << 20;

/// The disk a Camelot VM named `vm` boots from: its own copy of
/// [`BASE_IMAGE`], `<vm>-disk0` (its disk 0).
pub fn vm_disk(vm: &str) -> String {
    format!("{vm}-disk0")
}

/// Why [`create_camelot_vm`] failed.
#[derive(Debug)]
pub enum CamelotError {
    /// The VM's name is not valid, or leaves no room for `-disk0` in a disk name.
    Name(String),
    /// The Lake server has no base image `image` ([`BASE_IMAGE`]).
    NoBaseImage { lake: String, image: &'static str },
    /// Lake refused to copy the image (its name is taken, the base image has a
    /// writer), or failed.
    Lake { disk: String, cause: Error },
    /// Avalon refused the VM (its name is taken, over the RAM budget, ...). The copy
    /// was deleted again.
    Avalon(Error),
}

impl CamelotError {
    /// The protocol error code, when a server refused.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            CamelotError::Name(_) => Some(ErrorCode::InvalidName),
            CamelotError::NoBaseImage { .. } => Some(ErrorCode::NoSuchDisk),
            CamelotError::Lake { cause, .. } | CamelotError::Avalon(cause) => cause.code(),
        }
    }
}

impl fmt::Display for CamelotError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CamelotError::Name(why) => write!(f, "{why}"),
            CamelotError::NoBaseImage { lake, image } => write!(
                f,
                "the Lake at {lake} has no '{image}' image to copy (start the platform \
                 with 'cargo xtask up', which puts it in Lake's images directory)"
            ),
            CamelotError::Lake { disk, cause } => write!(f, "cannot create disk {disk}: {cause}"),
            CamelotError::Avalon(cause) => write!(f, "{cause}"),
        }
    }
}

impl std::error::Error for CamelotError {}

/// Creates a VM that boots Camelot: copies the base image ([`BASE_IMAGE`]) on the
/// Lake server at `lake_addr` (the address Avalon will reach it at too) to
/// [`vm_disk`]`(name)`, puts that disk first in `profile` (disk 0, read-write) and
/// creates the VM with no program, so it boots from the disk (protocol doc,
/// `CreateVm`). The VM is left in state created. If Avalon refuses the VM, the copy is
/// deleted again. Returns the VM's id and the disk's name.
pub fn create_camelot_vm(
    avalon: &mut Client,
    lake: &mut Client,
    lake_addr: &str,
    name: &str,
    mut profile: HardwareProfile,
) -> Result<(u32, String), CamelotError> {
    validate_name(name).map_err(CamelotError::Name)?;
    let disk = vm_disk(name);
    validate_name(&disk).map_err(|_| {
        CamelotError::Name(format!(
            "'{name}' is too long for a Camelot VM: its disk '{disk}' would be over {} characters",
            avalon_proto::MAX_NAME_LEN
        ))
    })?;
    let source = DiskSource::parse(&format!("{}{lake_addr}/{disk}", avalon_proto::LAKE_SCHEME))
        .map_err(CamelotError::Name)?;
    let image = BASE_IMAGE;
    if let Err(e) = lake.clone_disk(&disk, image, None) {
        // Only the source can be missing in a clone.
        return Err(match e.code() {
            Some(ErrorCode::NoSuchDisk) => CamelotError::NoBaseImage {
                lake: lake_addr.to_string(),
                image,
            },
            _ => CamelotError::Lake { disk, cause: e },
        });
    }
    profile.devices.insert(
        0,
        DeviceSpec::Disk(DiskSpec {
            source,
            read_only: false,
        }),
    );
    match avalon.create_vm(name, profile, Vec::new()) {
        Ok(id) => Ok((id, disk)),
        Err(e) => {
            // Avalon released the disk with the VM it did not make.
            let _ = lake.delete_disk(&disk);
            Err(CamelotError::Avalon(e))
        }
    }
}

/// One session with an Avalon or Lake server.
///
/// Every method sends one request and waits for its reply; a refusal comes back as an
/// [`Error`] whose [`Error::code`] says why. A session that broke (the server went
/// away) fails every later request too: connect again.
pub struct Client<S = TcpStream> {
    conn: Connection<S>,
    server: String,
}

impl Client<TcpStream> {
    /// Connects to the server at `addr` (`host:port`, see [`parse_addr`]) and does the
    /// handshake as `agent` (`"round-table"`, `"pendragon"`, ...), with the default
    /// [`Timeouts`]: the session gives up on a reply after [`REPLY_TIMEOUT`].
    pub fn connect(addr: &str, agent: &str) -> Result<Self> {
        Client::connect_with(addr, agent, Timeouts::default())
    }

    /// [`Client::connect`] with other time limits.
    pub fn connect_with(addr: &str, agent: &str, timeouts: Timeouts) -> Result<Self> {
        let fail = |cause: avalon_proto::Error| Error::Connect {
            addr: addr.to_string(),
            cause,
        };
        let stream = open(addr, timeouts.connect).map_err(|e| fail(e.into()))?;
        // Requests are small frames; do not wait to batch them.
        stream.set_nodelay(true).map_err(|e| fail(e.into()))?;
        stream
            .set_read_timeout(Some(timeouts.reply))
            .map_err(|e| fail(e.into()))?;
        Client::handshake(stream, agent).map_err(|e| match e {
            Error::Request(cause) | Error::Connect { cause, .. } => fail(cause),
        })
    }
}

/// A TCP connection to the first address `addr` resolves to that answers within
/// `timeout`.
fn open(addr: &str, timeout: Duration) -> std::io::Result<TcpStream> {
    let mut last = None;
    for a in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "the address resolves to nothing")
    }))
}

impl<S: Read + Write> Client<S> {
    /// Starts a session over `stream` (any byte stream: a socket, or a pipe in tests):
    /// sends `Hello` as `agent` and reads the server's `Welcome`.
    pub fn handshake(stream: S, agent: &str) -> Result<Self> {
        let mut conn = Connection::new(stream);
        let server = conn.client_handshake(agent)?;
        Ok(Client { conn, server })
    }

    /// The name the server gave in its `Welcome` (`"avalon 0.1.0"`).
    pub fn server(&self) -> &str {
        &self.server
    }

    /// The underlying connection, for requests this crate has no method for.
    pub fn connection(&mut self) -> &mut Connection<S> {
        &mut self.conn
    }

    /// The session's connection, for a client that goes on with events of its own (a
    /// console attached with [`Client::attach_console`]).
    pub fn into_connection(self) -> Connection<S> {
        self.conn
    }

    /// Checks that the server answers.
    pub fn ping(&mut self) -> Result<()> {
        let token = 0x5254_0001;
        match self.conn.request(&Message::Ping { token })? {
            Message::Pong { token: t } if t == token => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    // ---- VM control (Avalon) ----

    /// Creates a VM named `name` with `profile`, loaded with the ELF executable
    /// `program` and left in state created. Returns its id.
    pub fn create_vm(&mut self, name: &str, profile: HardwareProfile, program: Vec<u8>) -> Result<u32> {
        let request = Message::CreateVm {
            name: name.to_string(),
            profile,
            program,
        };
        match self.conn.request(&request)? {
            Message::VmCreated { id } => Ok(id),
            other => Err(unexpected(&other)),
        }
    }

    /// Every VM, in id order.
    pub fn list_vms(&mut self) -> Result<Vec<VmSummary>> {
        match self.conn.request(&Message::ListVms)? {
            Message::VmList { vms } => Ok(vms),
            other => Err(unexpected(&other)),
        }
    }

    /// Everything the server reports about one VM.
    pub fn vm_info(&mut self, vm: &VmRef) -> Result<VmDetails> {
        match self.conn.request(&Message::GetVmInfo { vm: vm.clone() })? {
            Message::VmInfo(details) => Ok(details),
            other => Err(unexpected(&other)),
        }
    }

    /// Starts, pauses, resumes, stops or destroys a VM.
    pub fn vm_action(&mut self, action: VmAction, vm: &VmRef) -> Result<()> {
        Ok(self.conn.request_ok(&action.request(vm.clone()))?)
    }

    pub fn start_vm(&mut self, vm: &VmRef) -> Result<()> {
        self.vm_action(VmAction::Start, vm)
    }

    pub fn pause_vm(&mut self, vm: &VmRef) -> Result<()> {
        self.vm_action(VmAction::Pause, vm)
    }

    pub fn resume_vm(&mut self, vm: &VmRef) -> Result<()> {
        self.vm_action(VmAction::Resume, vm)
    }

    pub fn stop_vm(&mut self, vm: &VmRef) -> Result<()> {
        self.vm_action(VmAction::Stop, vm)
    }

    pub fn destroy_vm(&mut self, vm: &VmRef) -> Result<()> {
        self.vm_action(VmAction::Destroy, vm)
    }

    // ---- Console (Avalon) ----

    /// Attaches this session to a VM's console: returns the VM's id and its
    /// scrollback (its recent output). `ConsoleOutput` events follow on the
    /// connection, read with [`Client::connection`]'s `recv`, until the session ends
    /// or detaches; `ConsoleInput` events sent on it reach the VM's UART.
    pub fn attach_console(&mut self, vm: &VmRef) -> Result<(u32, Vec<u8>)> {
        match self.conn.request(&Message::ConsoleAttach { vm: vm.clone() })? {
            Message::ConsoleAttached { vm, scrollback } => Ok((vm, scrollback)),
            other => Err(unexpected(&other)),
        }
    }

    // ---- Debugging (Avalon) ----

    /// Makes this session the debugger of a VM (protocol doc, section 6.8), which stops
    /// for it: returns the VM's id. `DebugOutput` events follow on the connection, and
    /// `DebugInput` events sent on it reach the VM's GDB stub; [`DebugTunnel`] makes the
    /// two a byte stream.
    pub fn attach_debugger(&mut self, vm: &VmRef) -> Result<u32> {
        match self.conn.request(&Message::DebugAttach { vm: vm.clone() })? {
            Message::DebugAttached { vm } => Ok(vm),
            other => Err(unexpected(&other)),
        }
    }

    /// Stops being VM `vm`'s debugger, as GDB's `D` does: its breakpoints go, and it
    /// runs on.
    pub fn detach_debugger(&mut self, vm: u32) -> Result<()> {
        Ok(self.conn.request_ok(&Message::DebugDetach { vm })?)
    }

    // ---- Virtual network (Avalon) ----

    /// Creates a virtual switch with no cards plugged in.
    pub fn create_switch(&mut self, name: &str) -> Result<()> {
        self.create_switch_with(name, None)
    }

    /// Creates a virtual switch with no cards plugged in, and a NAT gateway if `nat`
    /// asks for one (which fails the request if a forwarded port cannot be bound).
    pub fn create_switch_with(&mut self, name: &str, nat: Option<NatConfig>) -> Result<()> {
        Ok(self.conn.request_ok(&Message::CreateSwitch {
            name: name.to_string(),
            nat,
        })?)
    }

    /// Gives `switch` a NAT gateway (`Some`), or takes its gateway away (`None`).
    pub fn set_switch_nat(&mut self, switch: &str, nat: Option<NatConfig>) -> Result<()> {
        Ok(self.conn.request_ok(&Message::SetSwitchNat {
            switch: switch.to_string(),
            nat,
        })?)
    }

    /// Deletes a switch, unplugging every card from it.
    pub fn delete_switch(&mut self, name: &str) -> Result<()> {
        Ok(self.conn.request_ok(&Message::DeleteSwitch {
            name: name.to_string(),
        })?)
    }

    /// Every switch, in name order, with its ports and their counters.
    pub fn list_switches(&mut self) -> Result<Vec<SwitchInfo>> {
        match self.conn.request(&Message::ListSwitches)? {
            Message::SwitchList { switches } => Ok(switches),
            other => Err(unexpected(&other)),
        }
    }

    /// Plugs network card `card` of `vm` into `switch`.
    pub fn attach_card(&mut self, vm: &VmRef, card: u8, switch: &str) -> Result<()> {
        Ok(self.conn.request_ok(&Message::AttachCard {
            vm: vm.clone(),
            card,
            switch: switch.to_string(),
        })?)
    }

    /// Unplugs network card `card` of `vm` from its switch.
    pub fn detach_card(&mut self, vm: &VmRef, card: u8) -> Result<()> {
        Ok(self
            .conn
            .request_ok(&Message::DetachCard { vm: vm.clone(), card })?)
    }

    /// Starts capturing `switch` into the pcap file `path` on the server's host, or
    /// stops (`None`).
    pub fn capture_switch(&mut self, switch: &str, path: Option<&str>) -> Result<()> {
        Ok(self.conn.request_ok(&Message::CaptureSwitch {
            switch: switch.to_string(),
            path: path.map(str::to_string),
        })?)
    }

    // ---- Disk management (Lake) ----

    /// Creates a zero-filled image of `sectors` 512-byte sectors.
    pub fn create_disk(&mut self, name: &str, sectors: u64) -> Result<()> {
        Ok(self.conn.request_ok(&Message::DiskCreate {
            name: name.to_string(),
            sectors,
            source: None,
        })?)
    }

    /// Creates an image that is a copy of image `source`: as long as it, or `sectors`
    /// long (no fewer than the source's; zeros after its end). The source must not have
    /// a writer. Answered once the copy is complete.
    pub fn clone_disk(&mut self, name: &str, source: &str, sectors: Option<u64>) -> Result<()> {
        Ok(self.conn.request_ok(&Message::DiskCreate {
            name: name.to_string(),
            sectors: sectors.unwrap_or(0),
            source: Some(source.to_string()),
        })?)
    }

    /// Every image, in name order, with who has it attached.
    pub fn list_disks(&mut self) -> Result<Vec<DiskInfo>> {
        match self.conn.request(&Message::ListDisks)? {
            Message::DiskList { disks } => Ok(disks),
            other => Err(unexpected(&other)),
        }
    }

    /// Deletes an image nobody has attached.
    pub fn delete_disk(&mut self, name: &str) -> Result<()> {
        Ok(self.conn.request_ok(&Message::DiskDelete {
            name: name.to_string(),
        })?)
    }
}

/// A reply of the wrong type for the request.
fn unexpected(reply: &Message) -> Error {
    Error::Request(avalon_proto::Error::Unexpected(reply.type_code()))
}

/// A debugger's end of a VM's GDB stub (protocol doc, section 6.8): the GDB remote
/// serial protocol's bytes, carried in `DebugInput` and `DebugOutput` over a session
/// attached as the VM's debugger, as a byte stream a GDB client reads and writes as it
/// would a socket. A thread reads the session; [`DebugTunnel::read_timeout`] takes
/// what it has read.
///
/// The stream ends when the server ends the attachment (`Detached`: the VM's machine
/// ended) or the session breaks. Dropping the tunnel closes the session: the server
/// leaves the VM as it is, stopped or running, breakpoints and all, as QEMU leaves a VM
/// whose GDB connection closes. A debugger detaches with GDB's `D` first.
pub struct DebugTunnel {
    conn: Connection<TcpStream>,
    vm: u32,
    rx: Receiver<Tunnelled>,
    /// Bytes read from the session that the reader has not taken yet.
    pending: VecDeque<u8>,
    /// Why the stream ended, once it has.
    ended: Option<String>,
}

/// What the tunnel's thread reads.
enum Tunnelled {
    Bytes(Vec<u8>),
    End(String),
}

impl DebugTunnel {
    /// Attaches `client`'s session to VM `vm` as its debugger (the VM stops) and opens
    /// the tunnel.
    pub fn open(mut client: Client<TcpStream>, vm: &VmRef) -> Result<DebugTunnel> {
        let id = client.attach_debugger(vm)?;
        let mut conn = client.into_connection();
        let mut reader = conn.try_clone()?;
        reader
            .get_ref()
            .set_read_timeout(None)
            .map_err(avalon_proto::Error::from)?;
        let (tx, rx) = mpsc::channel();
        // Whatever came with the reply (a state change) is not the stub's.
        while conn.queued_events() > 0 {
            if let Ok(Message::DebugOutput { vm, data }) = conn.recv() {
                if vm == id {
                    let _ = tx.send(Tunnelled::Bytes(data));
                }
            }
        }
        std::thread::spawn(move || loop {
            let sent = match reader.recv() {
                Ok(Message::DebugOutput { vm, data }) if vm == id => tx.send(Tunnelled::Bytes(data)),
                Ok(Message::Detached {
                    vm,
                    kind: avalon_proto::AttachKind::Debug,
                }) if vm == id => {
                    let _ = tx.send(Tunnelled::End(format!("VM {id}'s machine has ended")));
                    return;
                }
                // State changes and the like: the stub says what a debugger needs.
                Ok(_) => Ok(()),
                Err(e) => {
                    let _ = tx.send(Tunnelled::End(e.to_string()));
                    return;
                }
            };
            if sent.is_err() {
                return;
            }
        });
        Ok(DebugTunnel {
            conn,
            vm: id,
            rx,
            pending: VecDeque::new(),
            ended: None,
        })
    }

    /// The VM's id.
    pub fn vm(&self) -> u32 {
        self.vm
    }

    /// Reads what the stub has sent into `buf`, waiting at most `timeout` for it
    /// (forever with none). `ErrorKind::TimedOut` if nothing came in time;
    /// `UnexpectedEof` once the stream has ended.
    pub fn read_timeout(&mut self, buf: &mut [u8], timeout: Option<Duration>) -> io::Result<usize> {
        if self.pending.is_empty() && self.ended.is_none() {
            let got = match timeout {
                Some(t) => self.rx.recv_timeout(t),
                None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match got {
                Ok(Tunnelled::Bytes(b)) => self.pending.extend(b),
                Ok(Tunnelled::End(why)) => self.ended = Some(why),
                Err(RecvTimeoutError::Timeout) => return Err(io::ErrorKind::TimedOut.into()),
                Err(RecvTimeoutError::Disconnected) => self.ended = Some("the session ended".into()),
            }
        }
        if self.pending.is_empty() {
            let why = self.ended.clone().unwrap_or_default();
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, why));
        }
        let n = buf.len().min(self.pending.len());
        for (b, p) in buf.iter_mut().zip(self.pending.drain(..n)) {
            *b = p;
        }
        Ok(n)
    }

    /// Sends `data` to the stub.
    pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        let msg = Message::DebugInput {
            vm: self.vm,
            data: data.to_vec(),
        };
        self.conn.send(&msg).map_err(|e| match e {
            avalon_proto::Error::Io(e) => e,
            other => io::Error::other(other.to_string()),
        })
    }
}

impl Drop for DebugTunnel {
    fn drop(&mut self) {
        // The reader thread ends with the session.
        let _ = self.conn.get_ref().shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    /// A one-connection server on an ephemeral port: reads the client's `Hello`, then
    /// answers with `answer` (nothing at all when it is `None`) and holds the
    /// connection open until the client goes away.
    fn server(answer: Option<Message>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut conn = Connection::new(stream);
            assert!(matches!(conn.recv(), Ok(Message::Hello { .. })));
            if let Some(answer) = answer {
                conn.send(&answer).unwrap();
            }
            // Until the client closes.
            while conn.recv().is_ok() {}
        });
        addr
    }

    #[test]
    fn a_refused_handshake_keeps_its_code() {
        let addr = server(Some(Message::Error {
            code: ErrorCode::Busy,
            message: "full".into(),
        }));
        let err = Client::connect(&addr, "test").err().unwrap();
        assert!(matches!(err, Error::Connect { .. }), "{err:?}");
        assert_eq!(err.code(), Some(ErrorCode::Busy));
        assert_eq!(err.to_string(), format!("cannot connect to {addr}: busy: full"));
    }

    #[test]
    fn sessions_time_out() {
        // The default session waits REPLY_TIMEOUT for a reply.
        let addr = server(Some(Message::Welcome {
            version: avalon_proto::PROTOCOL_VERSION,
            server: "fake".into(),
        }));
        let mut client = Client::connect(&addr, "test").unwrap();
        assert_eq!(client.server(), "fake");
        assert_eq!(
            client.connection().get_ref().read_timeout().unwrap(),
            Some(REPLY_TIMEOUT)
        );

        // A server that accepts but never says Welcome is given up on.
        let addr = server(None);
        let timeouts = Timeouts {
            reply: Duration::from_millis(200),
            ..Timeouts::default()
        };
        let start = Instant::now();
        let err = Client::connect_with(&addr, "test", timeouts).err().unwrap();
        assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
        match &err {
            Error::Connect {
                cause: avalon_proto::Error::Io(e),
                ..
            } => assert!(
                matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ),
                "{e:?}"
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(err.code(), None);
    }

    #[test]
    fn actions_by_state() {
        use VmState::*;
        let allowed = |state| -> Vec<&str> {
            VmAction::ALL
                .into_iter()
                .filter(|a| a.allowed_in(state))
                .map(VmAction::name)
                .collect()
        };
        assert_eq!(allowed(Created), ["start", "stop", "destroy"]);
        assert_eq!(allowed(Running), ["pause", "stop", "destroy"]);
        assert_eq!(allowed(Paused), ["resume", "stop", "destroy"]);
        // Stopped at a breakpoint, once its debugger has gone (QEMU's `cont`).
        assert_eq!(allowed(Debug), ["resume", "stop", "destroy"]);
        assert_eq!(allowed(Stopped), ["destroy"]);
        assert_eq!(allowed(Exited(3)), ["destroy"]);
        assert!(holds_disks(Created) && holds_disks(Running) && holds_disks(Paused) && holds_disks(Debug));
        assert!(!holds_disks(Stopped) && !holds_disks(Exited(0)));
    }

    #[test]
    fn lake_disks_of_a_profile() {
        let disk = |source: &str, read_only| {
            DeviceSpec::Disk(avalon_proto::DiskSpec {
                source: crate::parse_disk_source(source).unwrap(),
                read_only,
            })
        };
        let profile = HardwareProfile {
            devices: vec![
                disk("lake://10.0.0.9:7401/data", false),
                disk("boot.img", true),
                disk("lake://store/root", true),
            ],
            ..HardwareProfile::default()
        };
        assert_eq!(
            lake_disks(&profile).collect::<Vec<_>>(),
            [("10.0.0.9:7401", "data", false), ("store:7401", "root", true)]
        );
    }
}
