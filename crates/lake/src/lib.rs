//! Lake: the storage server of the Avalon platform. It keeps a directory of disk images
//! and serves them over the Avalon protocol (`docs/avalon-protocol.md`): disk
//! management (`DiskCreate`, a new image or a copy of one, `ListDisks`, `DiskDelete`,
//! what Round Table's `disk` commands send) and block I/O (`BlockOpen`, `BlockRead`, `BlockWrite`, `BlockFlush`,
//! `BlockSize`, `BlockClose`, what Grail's network disk sends for a VM whose profile
//! names `lake://addr/name`). So a VM's disk can live in another process, or on
//! another machine, and outlive the VM.
//!
//! - [`Store`] (`store.rs`): the images directory and who has which image attached.
//!   One writer per image at a time, or any number of read-only attachments.
//! - [`Server`] (`server.rs`): the listener and one thread per client.
//!
//! The `lake` binary is a thin command line over [`Server`]; tests run a server in
//! process on an ephemeral port.

use avalon_proto::{ErrorCode, Message};
use std::sync::{Mutex, MutexGuard, PoisonError};

mod server;
mod store;

pub use server::{
    parse_listen, Config, Server, ServerThread, ShutdownHandle, HANDSHAKE_TIMEOUT, MAX_HANDLES,
};
pub use store::{Image, Store, MAX_IMAGE_SECTORS};

/// Locks `m`, carrying on if a thread panicked while holding it: the attachment table
/// stays usable for every other client.
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
