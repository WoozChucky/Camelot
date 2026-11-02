//! The client side of the Avalon platform: the operations a person orders, over the
//! Avalon protocol (`docs/avalon-protocol.md`).
//!
//! - [`Client`]: one session with a server. VM control (create, list, info, start,
//!   pause, resume, stop, destroy) talks to Avalon; disk management (create, list,
//!   delete) talks to Lake. Each operation is one request and its typed reply.
//! - [`parse`]: the text people type for addresses, RAM sizes, clocks, disk sources and
//!   disk sizes, so every front end accepts the same syntax (`--ram` and `--clock` are
//!   Grail's own parsers).
//! - [`show`]: the same values as text again (sizes, frequencies, device names).
//! - [`VmAction::allowed_in`], [`holds_disks`] and [`lake_disks`]: what a VM's state
//!   and profile mean for what a person may do with it and its disks.
//!
//! Round Table, the command-line client, is a thin layer over this crate, and so is
//! Pendragon, the web console: the two cannot drift apart.

use avalon_proto::{
    Connection, DeviceSpec, DiskInfo, DiskSource, ErrorCode, HardwareProfile, Message, VmDetails, VmRef,
    VmState, VmSummary,
};
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub mod parse;
pub mod show;

pub use parse::{parse_addr, parse_clock, parse_disk_size, parse_disk_source, parse_ram};
pub use show::{device_name, disk_source_text, format_hz, format_size};

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
    /// section 6.2): front ends offer only these.
    pub fn allowed_in(self, state: VmState) -> bool {
        use VmState::*;
        match self {
            VmAction::Start => state == Created,
            VmAction::Pause => state == Running,
            VmAction::Resume => state == Paused,
            VmAction::Stop => matches!(state, Created | Running | Paused),
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
/// stopped or its program exits (protocol doc, section 6.6).
pub fn holds_disks(state: VmState) -> bool {
    matches!(state, VmState::Created | VmState::Running | VmState::Paused)
}

/// The Lake images in a hardware profile, as (server address, image name, read-only),
/// in the profile's order.
pub fn lake_disks(profile: &HardwareProfile) -> impl Iterator<Item = (&str, &str, bool)> {
    profile
        .devices
        .iter()
        .filter_map(|DeviceSpec::Disk(disk)| match &disk.source {
            DiskSource::Lake { addr, name } => Some((addr.as_str(), name.as_str(), disk.read_only)),
            DiskSource::File(_) => None,
        })
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

    // ---- Disk management (Lake) ----

    /// Creates a zero-filled image of `sectors` 512-byte sectors.
    pub fn create_disk(&mut self, name: &str, sectors: u64) -> Result<()> {
        Ok(self.conn.request_ok(&Message::DiskCreate {
            name: name.to_string(),
            sectors,
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
        assert_eq!(allowed(Stopped), ["destroy"]);
        assert_eq!(allowed(Exited(3)), ["destroy"]);
        assert!(holds_disks(Created) && holds_disks(Running) && holds_disks(Paused));
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
