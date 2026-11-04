//! Avalon: the hypervisor server of the Avalon platform. One long-running process that
//! creates and runs any number of Grail virtual machines, each with its own hardware
//! profile, and serves the Avalon protocol (`docs/avalon-protocol.md`) so that Round
//! Table can manage them and Herald can attach to their consoles and screens.
//!
//! - [`Server`] (`server.rs`): the listener, the VM table and two threads per client.
//! - `vm.rs`: one VM, its life cycle, its thread, its console and its screen (with the
//!   keyboard and pointer input of the clients that view it).
//! - `outbox.rs`: a client's outgoing queue (replies and events).
//! - [`machine_config`]: a hardware profile from the wire as a Grail `MachineConfig`.
//! - [`switch`]: the virtual Ethernet switches the VMs' network cards plug into.
//!
//! The `avalon` binary is a thin command line over [`Server`]; tests run a server in
//! process on an ephemeral port.

use avalon_proto::{ErrorCode, Message};
use std::sync::{Mutex, MutexGuard, PoisonError};

mod outbox;
mod profile;
mod server;
pub mod switch;
mod vm;

pub use outbox::MAX_QUEUED;
pub use profile::{card_configs, machine_config, CardConfig};
pub use server::{
    parse_listen, parse_max_ram, Config, Server, ServerThread, ShutdownHandle, VmDebug, DEFAULT_MAX_RAM,
    HANDSHAKE_TIMEOUT,
};
pub use vm::{
    screen_backlog, MAX_INPUT_QUEUE, SCREEN_BACKLOG_MARGIN, SCREEN_BACKLOG_MIN, SCREEN_INTERVAL, SCROLLBACK,
};

/// Locks `m`, carrying on if a thread panicked while holding it: the server's state
/// stays usable for every other VM and client.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A request that failed: the `Error` reply it gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail {
    pub code: ErrorCode,
    pub message: String,
}

impl Fail {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Fail {
            code,
            message: message.into(),
        }
    }
}

impl From<Fail> for Message {
    fn from(f: Fail) -> Message {
        Message::Error {
            code: f.code,
            message: f.message,
        }
    }
}
