//! An in-process Ethernet hub: every frame a port sends is delivered to every other
//! port, like a repeater hub (no address learning, so each card sees everything). Cards
//! of different machines, on different threads, connect by attaching to the same hub.

use super::{MacAddr, NetBackend};
use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

/// Frames waiting for one port, at most. A port whose card does not keep up (a paused
/// or slow machine) loses the frames past this, not the hub's memory.
pub const HUB_QUEUE: usize = 256;

/// A handle to a hub. Clones are the same hub; compared by identity.
#[derive(Clone, Default)]
pub struct Hub {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    ports: Vec<Port>,
    /// Ports attached so far, including detached ones: numbers are never reused.
    attached: u32,
}

struct Port {
    id: u32,
    queue: VecDeque<Vec<u8>>,
    dropped: u64,
}

impl Hub {
    pub fn new() -> Self {
        Hub::default()
    }

    /// A new port on this hub: a backend for one card. Ports are numbered from 1 in
    /// the order they attach.
    pub fn attach(&self) -> HubPort {
        let mut state = lock(&self.state);
        state.attached += 1;
        let id = state.attached;
        state.ports.push(Port {
            id,
            queue: VecDeque::new(),
            dropped: 0,
        });
        HubPort {
            state: self.state.clone(),
            id,
        }
    }

    /// Ports attached now.
    pub fn ports(&self) -> usize {
        lock(&self.state).ports.len()
    }
}

impl PartialEq for Hub {
    fn eq(&self, other: &Hub) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for Hub {}

impl fmt::Debug for Hub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hub({:p}, {} ports)", Arc::as_ptr(&self.state), self.ports())
    }
}

/// One port of a [`Hub`]; dropping it detaches it (its queued frames go with it).
pub struct HubPort {
    state: Arc<Mutex<State>>,
    id: u32,
}

impl HubPort {
    /// The port's number on its hub, from 1.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Grail's default MAC address for the card on this port: [`MacAddr::local`] of
    /// its number, so the cards on one hub all differ.
    pub fn default_mac(&self) -> MacAddr {
        MacAddr::local(self.id)
    }

    /// Frames lost because this port's queue was full.
    pub fn dropped(&self) -> u64 {
        lock(&self.state)
            .ports
            .iter()
            .find(|p| p.id == self.id)
            .map_or(0, |p| p.dropped)
    }
}

impl NetBackend for HubPort {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let mut state = lock(&self.state);
        for port in state.ports.iter_mut().filter(|p| p.id != self.id) {
            if port.queue.len() < HUB_QUEUE {
                port.queue.push_back(frame.to_vec());
            } else {
                port.dropped += 1;
            }
        }
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let mut state = lock(&self.state);
        state
            .ports
            .iter_mut()
            .find(|p| p.id == self.id)?
            .queue
            .pop_front()
    }
}

impl Drop for HubPort {
    fn drop(&mut self) {
        lock(&self.state).ports.retain(|p| p.id != self.id);
    }
}

/// The hub's state. A thread that panicked while holding the lock left it consistent
/// (every update is a single push or pop), so a poisoned lock is used as it is: one
/// machine's crash must not take the others down.
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_go_to_every_other_port() {
        let hub = Hub::new();
        let mut a = hub.attach();
        let mut b = hub.attach();
        let mut c = hub.attach();
        assert_eq!([a.id(), b.id(), c.id()], [1, 2, 3]);
        assert_eq!(hub.ports(), 3);
        a.send(&[1; 14]).unwrap();
        b.send(&[2; 14]).unwrap();
        assert_eq!(a.recv(), Some(vec![2; 14]), "not its own");
        assert_eq!(a.recv(), None);
        assert_eq!(b.recv(), Some(vec![1; 14]));
        assert_eq!(b.recv(), None);
        assert_eq!(c.recv(), Some(vec![1; 14]), "in the order they were sent");
        assert_eq!(c.recv(), Some(vec![2; 14]));
        assert_eq!(c.recv(), None);
        assert!(a.link_up());
    }

    #[test]
    fn ports_have_distinct_default_macs() {
        let hub = Hub::new();
        let (a, b) = (hub.attach(), hub.attach());
        assert_eq!(a.default_mac(), MacAddr::local(1));
        assert_eq!(b.default_mac(), MacAddr::local(2));
        drop(a);
        assert_eq!(hub.attach().id(), 3, "numbers are not reused");
    }

    #[test]
    fn queues_are_bounded() {
        let hub = Hub::new();
        let mut a = hub.attach();
        let mut b = hub.attach();
        for i in 0..HUB_QUEUE + 5 {
            a.send(&[i as u8; 14]).unwrap();
        }
        assert_eq!(b.dropped(), 5);
        assert_eq!(a.dropped(), 0);
        let got: Vec<_> = std::iter::from_fn(|| b.recv()).collect();
        assert_eq!(got.len(), HUB_QUEUE);
        assert_eq!(got[0], [0; 14], "the oldest are kept");
        a.send(&[9; 14]).unwrap();
        assert_eq!(b.recv(), Some(vec![9; 14]), "room again");
    }

    #[test]
    fn dropped_ports_leave_the_hub() {
        let hub = Hub::new();
        let mut a = hub.attach();
        let b = hub.attach();
        a.send(&[1; 14]).unwrap();
        drop(b);
        assert_eq!(hub.ports(), 1);
        a.send(&[2; 14]).unwrap(); // to nobody
        assert_eq!(a.recv(), None);
        let mut c = hub.attach();
        assert_eq!(c.recv(), None, "a new port starts empty");
    }

    #[test]
    fn hubs_compare_by_identity() {
        let hub = Hub::new();
        assert_eq!(hub, hub.clone());
        assert_ne!(hub, Hub::new());
        let _port = hub.attach();
        assert!(format!("{hub:?}").ends_with(", 1 ports)"));
    }

    #[test]
    fn ports_work_across_threads() {
        let hub = Hub::new();
        let mut here = hub.attach();
        let mut there = hub.attach();
        let echo = std::thread::spawn(move || {
            // Echo 100 frames back, each with its first byte incremented.
            let mut n = 0;
            while n < 100 {
                match there.recv() {
                    Some(mut f) => {
                        f[0] += 1;
                        there.send(&f).unwrap();
                        n += 1;
                    }
                    None => std::thread::yield_now(),
                }
            }
        });
        for i in 0..100u8 {
            here.send(&[i; 14]).unwrap();
        }
        let mut got = Vec::new();
        while got.len() < 100 {
            match here.recv() {
                Some(f) => got.push(f[0]),
                None => std::thread::yield_now(),
            }
        }
        echo.join().unwrap();
        assert_eq!(got, (1..=100).collect::<Vec<u8>>());
    }
}
