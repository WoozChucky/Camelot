//! User networking (`--net user`, spec section 9.8): the card's backend is a
//! [`gateway::Gateway`], a NAT gateway in this process that hands out an address by
//! DHCP, answers DNS, and carries the guest's TCP and UDP through host sockets, so the
//! guest reaches the internet and the host (10.0.2.2) with no driver on the host.
//!
//! The gateway lives in its own crate, with its own frame API, so that other hosts of
//! Ethernet frames (an Avalon switch) can use it without depending on Grail; this file
//! is the whole of the glue. Both calls are non-blocking, as a backend's must be: the
//! gateway services its sockets when the card polls it.

use super::NetBackend;
use gateway::Gateway;
use std::io;

impl NetBackend for Gateway {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        Gateway::send(self, frame);
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        Gateway::recv(self)
    }
}
