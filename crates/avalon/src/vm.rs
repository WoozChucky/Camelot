//! One virtual machine: a Grail [`Machine`], its life cycle, its thread and its
//! console.
//!
//! A VM is created with its program loaded and its machine parked here, in state
//! created. Starting it moves the machine onto a thread of its own, which runs it in
//! slices with [`Machine::run_for`] and looks at the VM's state between slices: so
//! pause and stop take effect within one slice (a few milliseconds), and a real-time
//! machine sleeps off its pacing delay without holding anything. The thread sleeps
//! the same way when a full-speed machine is parked in `wfi` with nothing that could
//! wake it but input, instead of spinning through empty steps.
//!
//! Every VM keeps the recent output of its UART (the console) in a ring buffer, the
//! scrollback, and streams new output to the clients attached to its console. Their
//! input goes to the UART's receiver, in the order the server receives it.
//!
//! A VM's network cards ([`Card`]) plug into Avalon's switches. A frame queued for
//! one of them rings the VM's bell ([`Vm::kick`]), so a machine parked in `wfi`
//! takes it at once instead of after its nap.
//!
//! Locking: everything that changes is in [`Inner`], behind one mutex per VM, which
//! the VM thread takes only between slices. Events for attached clients are queued
//! under that lock, so every client sees them in the order they happened. The lock is
//! only ever held for a moment (a machine is dropped after it is let go, since that
//! can close disks over the network): other VMs' threads take it to ring the bell.

use crate::outbox::Outbox;
use crate::switch::Card;
use crate::{lock, Fail};
use avalon_proto::{
    AttachKind, DeviceEntry, DeviceSpec, ErrorCode, Frame, HardwareProfile, Message, NetSpec, VmDetails,
    VmState, VmSummary,
};
use excalibur::csr::IRQ_M_TIMER;
use grail::net::NetBackend;
use grail::{Machine, MachineConfig};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// Bytes of recent console output each VM keeps, sent to every client that attaches.
pub const SCROLLBACK: usize = 64 * 1024;

/// Steps per slice for a full-speed machine (a real-time one stops sooner, once it is
/// ~1 ms ahead of the wall clock): a few milliseconds of host time, so requests are
/// answered quickly without the thread spending its time on locks.
const SLICE: u64 = 100_000;

/// How long a full-speed machine parked in `wfi` sleeps before it looks again, if
/// nothing wakes it first (input and requests do).
const PARKED_NAP: Duration = Duration::from_millis(50);

pub struct Vm {
    /// Set by the server when the VM goes into its table.
    pub id: u32,
    pub name: String,
    pub profile: HardwareProfile,
    /// The guest's sysinfo device table.
    pub devices: Vec<DeviceEntry>,
    /// The network cards, in the profile's order.
    cards: Vec<Arc<Card>>,
    /// Steps run so far (instructions, and idle ticks of a machine waiting in `wfi`).
    steps: AtomicU64,
    /// Slices run so far (calls of `run_for`, empty ones included): how tests see that
    /// the thread sleeps when it has nothing to run.
    slices: AtomicU64,
    inner: Mutex<Inner>,
    /// Signalled whenever `inner` changes in a way the thread or a waiting request
    /// cares about.
    wake: Condvar,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// What the machine writes to its UART, collected by the thread between slices.
    output: OutputBuf,
    log: bool,
}

struct Inner {
    state: VmState,
    /// Destroyed, or the server is shutting down: the thread ends, nothing attaches.
    gone: bool,
    /// The machine, until the VM starts (then the thread owns it).
    machine: Option<Machine>,
    /// The thread is running a slice: a pause or stop waits for it to end, so that
    /// once the request is answered the machine does not run another step.
    busy: bool,
    /// Something the thread should look at before it sleeps again (input arrived).
    kick: bool,
    /// The UART's receiver. Bytes sent before the VM starts wait for it.
    input: Option<Sender<u8>>,
    scrollback: VecDeque<u8>,
    /// Clients attached to the console, at most once each.
    consoles: Vec<Arc<Outbox>>,
}

/// Collects what a machine writes to its UART. Only the VM's own thread touches it,
/// so the lock is never contended.
#[derive(Clone, Default)]
struct OutputBuf(Arc<Mutex<Vec<u8>>>);

impl OutputBuf {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *lock(&self.0))
    }
}

impl Write for OutputBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn wrong_state(vm: &Vm, state: VmState, want: &str) -> Fail {
    Fail::new(
        ErrorCode::InvalidState,
        format!("VM {} ('{}') is {state}, not {want}", vm.id, vm.name),
    )
}

impl Vm {
    /// Builds the machine `config` describes, with `cards` as its network cards (still
    /// unplugged), and loads `program` into it, as VM 0 until the server gives it an
    /// id. Errors: `InvalidProfile` (a disk image that cannot be opened) and
    /// `InvalidProgram`. This can take a while (it opens files on the host and
    /// allocates the guest's RAM), so the server calls it without holding its table.
    pub fn new(
        name: String,
        profile: HardwareProfile,
        config: MachineConfig,
        cards: Vec<Arc<Card>>,
        program: &[u8],
        log: bool,
    ) -> Result<Vm, Fail> {
        let backends = cards
            .iter()
            .map(|card| (card.mac, Box::new(card.backend()) as Box<dyn NetBackend>))
            .collect();
        let mut machine = Machine::with_devices(config, Vec::new(), backends)
            .map_err(|e| Fail::new(ErrorCode::InvalidProfile, e))?;
        machine
            .load_elf(program)
            .map_err(|e| Fail::new(ErrorCode::InvalidProgram, format!("program: {e}")))?;
        let (input, rx) = mpsc::channel();
        machine.set_uart_input(rx);
        let output = OutputBuf::default();
        machine.set_uart_output(output.clone());
        let devices = machine
            .cpu
            .bus
            .sysinfo
            .devices()
            .iter()
            .map(|d| DeviceEntry {
                kind: d.kind as u32,
                base: d.base,
                size: d.size,
                irq: d.irq,
            })
            .collect();
        Ok(Vm {
            id: 0,
            name,
            profile,
            devices,
            cards,
            steps: AtomicU64::new(0),
            slices: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                state: VmState::Created,
                gone: false,
                machine: Some(machine),
                busy: false,
                kick: false,
                input: Some(input),
                scrollback: VecDeque::new(),
                consoles: Vec::new(),
            }),
            wake: Condvar::new(),
            thread: Mutex::new(None),
            output,
            log,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, Inner>) -> MutexGuard<'a, Inner> {
        self.wake.wait(guard).unwrap_or_else(PoisonError::into_inner)
    }

    pub fn state(&self) -> VmState {
        self.lock().state
    }

    pub fn steps(&self) -> u64 {
        self.steps.load(Ordering::Relaxed)
    }

    pub fn slices(&self) -> u64 {
        self.slices.load(Ordering::Relaxed)
    }

    /// How many clients are attached to the console.
    pub fn console_count(&self) -> usize {
        self.lock().consoles.len()
    }

    /// The VM as `VmList` shows it. Its cards are as they are now: their addresses
    /// (made up by the server if the profile set none), and the switches they are
    /// plugged into.
    pub fn summary(&self) -> VmSummary {
        let mut profile = self.profile.clone();
        let mut cards = self.cards.iter();
        for device in &mut profile.devices {
            if let (DeviceSpec::Net(net), Some(card)) = (device, cards.next()) {
                *net = NetSpec {
                    switch: card.switch().map(|s| s.name().to_string()),
                    mac: Some(card.mac.0),
                };
            }
        }
        VmSummary {
            id: self.id,
            name: self.name.clone(),
            state: self.state(),
            profile,
        }
    }

    /// Network card `index`, if the VM has it.
    pub fn card(&self, index: u8) -> Result<&Arc<Card>, Fail> {
        self.cards.get(index as usize).ok_or_else(|| {
            let has = match self.cards.len() {
                0 => "no network cards".to_string(),
                1 => "one network card, 0".to_string(),
                n => format!("{n} network cards, 0 to {}", n - 1),
            };
            Fail::new(
                ErrorCode::NoSuchCard,
                format!("VM {} ('{}') has {has}", self.id, self.name),
            )
        })
    }

    pub fn cards(&self) -> &[Arc<Card>] {
        &self.cards
    }

    /// Wakes the VM's thread if it is napping: a frame came in for one of its cards.
    /// A frame that comes while a slice runs makes the thread run another slice at
    /// once rather than nap.
    pub fn kick(&self) {
        let mut inner = self.lock();
        inner.kick = true;
        self.wake.notify_all();
    }

    pub fn details(&self) -> VmDetails {
        VmDetails {
            vm: self.summary(),
            devices: self.devices.clone(),
            steps: self.steps(),
        }
    }

    /// Sets the state and tells every attached client.
    fn set_state(&self, inner: &mut Inner, state: VmState) {
        inner.state = state;
        if self.log {
            eprintln!("avalon: VM {} ('{}') {state}", self.id, self.name);
        }
        broadcast(
            &mut inner.consoles,
            &Message::VmStateChanged { vm: self.id, state },
        );
        self.wake.notify_all();
    }

    /// Waits until the thread is between slices.
    fn settle<'a>(&self, mut inner: MutexGuard<'a, Inner>) -> MutexGuard<'a, Inner> {
        while inner.busy {
            inner = self.wait(inner);
        }
        inner
    }

    /// created → running: starts the VM's thread.
    pub fn start(self: &Arc<Self>) -> Result<(), Fail> {
        let mut inner = self.lock();
        if inner.state != VmState::Created || inner.gone {
            return Err(wrong_state(self, inner.state, "created"));
        }
        let machine = inner.machine.take().expect("a created VM has its machine");
        let vm = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name(format!("vm-{}", self.id))
            .spawn(move || vm.run(machine));
        match spawned {
            Ok(handle) => {
                *lock(&self.thread) = Some(handle);
                self.set_state(&mut inner, VmState::Running);
                Ok(())
            }
            Err(e) => {
                // The machine went with the closure that could not run.
                self.set_state(&mut inner, VmState::Stopped);
                Err(Fail::new(
                    ErrorCode::Internal,
                    format!("cannot start a VM thread: {e}"),
                ))
            }
        }
    }

    /// running → paused. Once this returns the machine runs no more steps.
    pub fn pause(&self) -> Result<(), Fail> {
        let mut inner = self.lock();
        if inner.state != VmState::Running {
            return Err(wrong_state(self, inner.state, "running"));
        }
        self.set_state(&mut inner, VmState::Paused);
        drop(self.settle(inner));
        Ok(())
    }

    /// paused → running.
    pub fn resume(&self) -> Result<(), Fail> {
        let mut inner = self.lock();
        if inner.state != VmState::Paused {
            return Err(wrong_state(self, inner.state, "paused"));
        }
        self.set_state(&mut inner, VmState::Running);
        Ok(())
    }

    /// created, running or paused → stopped. The machine is dropped (by the thread,
    /// which ends); the VM keeps its scrollback, step count and RAM budget until it is
    /// destroyed.
    pub fn stop(&self) -> Result<(), Fail> {
        let mut inner = self.lock();
        match inner.state {
            VmState::Created | VmState::Running | VmState::Paused => {
                let machine = inner.machine.take();
                self.set_state(&mut inner, VmState::Stopped);
                drop(self.settle(inner));
                drop(machine);
                Ok(())
            }
            state => Err(wrong_state(self, state, "created, running or paused")),
        }
    }

    /// Ends the VM for good: its thread stops and is joined, its machine is dropped,
    /// and attached clients get `Detached`. The caller has taken it out of the table.
    pub fn destroy(&self) {
        let (consoles, machine) = {
            let mut inner = self.lock();
            inner.gone = true;
            inner.input = None;
            self.wake.notify_all();
            (std::mem::take(&mut inner.consoles), inner.machine.take())
        };
        drop(machine);
        let detached = event_frame(&Message::Detached {
            vm: self.id,
            kind: AttachKind::Console,
        });
        for client in consoles {
            client.event(&detached);
        }
        self.join();
        if self.log {
            eprintln!("avalon: VM {} ('{}') destroyed", self.id, self.name);
        }
    }

    /// Server shutdown: a VM that has not finished is stopped (attached clients see the
    /// state change), and its thread is joined.
    pub fn shut_down(&self) {
        let machine = {
            let mut inner = self.lock();
            let machine = inner.machine.take();
            if matches!(inner.state, VmState::Created | VmState::Running | VmState::Paused) {
                self.set_state(&mut inner, VmState::Stopped);
            }
            inner.gone = true;
            self.wake.notify_all();
            machine
        };
        drop(machine);
        self.join();
    }

    fn join(&self) {
        let handle = lock(&self.thread).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// Attaches `client` to the console: queues `ConsoleAttached` with the scrollback,
    /// under the lock, so `ConsoleOutput` events follow it with nothing missed or
    /// repeated. Attaching again just sends the scrollback again.
    pub fn attach_console(&self, client: &Arc<Outbox>) -> Result<(), Fail> {
        let mut inner = self.lock();
        if inner.gone {
            return Err(Fail::new(
                ErrorCode::NoSuchVm,
                format!("VM {} is being destroyed", self.id),
            ));
        }
        let scrollback = inner.scrollback.iter().copied().collect();
        client.reply_now(&Message::ConsoleAttached {
            vm: self.id,
            scrollback,
        });
        if !inner.consoles.iter().any(|c| c.id == client.id) {
            inner.consoles.push(Arc::clone(client));
        }
        Ok(())
    }

    /// Detaches client `id` from the console. False if it was not attached.
    pub fn detach_console(&self, id: u64) -> bool {
        let mut inner = self.lock();
        let before = inner.consoles.len();
        inner.consoles.retain(|c| c.id != id);
        inner.consoles.len() != before
    }

    /// Input from client `id` for the UART. Dropped (false) unless that client is
    /// attached to the console; also dropped once the machine is gone.
    pub fn console_input(&self, id: u64, data: &[u8]) -> bool {
        let mut inner = self.lock();
        if !inner.consoles.iter().any(|c| c.id == id) {
            return false;
        }
        if let Some(input) = &inner.input {
            for &b in data {
                let _ = input.send(b);
            }
        }
        inner.kick = true;
        self.wake.notify_all();
        true
    }

    /// The VM thread: runs the machine in slices while the VM is running.
    fn run(self: Arc<Self>, mut machine: Machine) {
        let _guard = ThreadGuard(&self);
        let paced = machine.config().clock_hz.is_some();
        loop {
            {
                let mut inner = self.lock();
                loop {
                    if inner.gone || matches!(inner.state, VmState::Stopped | VmState::Exited(_)) {
                        return;
                    }
                    if inner.state == VmState::Running {
                        break;
                    }
                    inner = self.wait(inner);
                }
                inner.busy = true;
                inner.kick = false;
            }

            let steps = machine.run_for(SLICE, &mut |_| {});
            // `run_for` ends a slice early once the guest has written FB_PRESENT, until
            // the host takes that frame: take it, or every later slice is a single step.
            // (Nothing streams screens yet; this is where a frame would be sent.)
            machine.cpu.bus.fb.take_present();
            self.steps.fetch_add(steps, Ordering::Relaxed);
            self.slices.fetch_add(1, Ordering::Relaxed);
            let output = self.output.take();

            let mut inner = self.lock();
            inner.busy = false;
            self.wake.notify_all();
            if !output.is_empty() {
                self.console_output(&mut inner, output);
            }
            if let Some(code) = machine.exit_code() {
                if !inner.gone && matches!(inner.state, VmState::Running | VmState::Paused) {
                    self.set_state(&mut inner, VmState::Exited(code));
                }
                return;
            }
            // Nothing to run right now: a real-time machine ahead of the wall clock, or
            // a full-speed one parked in wfi. Requests and input wake the thread early.
            let nap = if paced {
                machine.pace_delay()
            } else {
                parked(&machine).then_some(PARKED_NAP)
            };
            if let Some(nap) = nap {
                if inner.state == VmState::Running && !inner.kick && !inner.gone {
                    drop(self.wake.wait_timeout(inner, nap));
                }
            }
        }
    }

    /// New console output: into the scrollback and to every attached client.
    fn console_output(&self, inner: &mut Inner, data: Vec<u8>) {
        let keep = data.len().min(SCROLLBACK);
        let excess = (inner.scrollback.len() + keep).saturating_sub(SCROLLBACK);
        inner.scrollback.drain(..excess);
        inner.scrollback.extend(&data[data.len() - keep..]);
        // A slice writes at most SLICE bytes, far below the frame limit.
        broadcast(&mut inner.consoles, &Message::ConsoleOutput { vm: self.id, data });
    }
}

/// Whether a full-speed machine is stalled in `wfi` with only an external interrupt
/// (input) left to wake it: no interrupt is pending, and the timer cannot fire (it is
/// masked or not set; a set timer would end the wait at the next step).
fn parked(machine: &Machine) -> bool {
    let cpu = &machine.cpu;
    let timer_armed = cpu.csr.mie & (1 << IRQ_M_TIMER) != 0 && cpu.bus.clint.mtimecmp != u64::MAX;
    cpu.waiting && !timer_armed
}

fn event_frame(msg: &Message) -> Frame {
    Frame::from_message(msg).expect("events the server makes are valid and fit in a frame")
}

/// Queues an event for every client in `clients`, dropping those that are gone.
fn broadcast(clients: &mut Vec<Arc<Outbox>>, msg: &Message) {
    if clients.is_empty() {
        return;
    }
    let frame = event_frame(msg);
    clients.retain(|c| c.event(&frame));
}

/// Keeps a VM consistent if its thread ends by panicking (a Grail bug): the VM is
/// stopped, and requests waiting for the slice to end are released.
struct ThreadGuard<'a>(&'a Vm);

impl Drop for ThreadGuard<'_> {
    fn drop(&mut self) {
        let vm = self.0;
        let mut inner = vm.lock();
        inner.busy = false;
        if std::thread::panicking() {
            eprintln!("avalon: VM {} ('{}') crashed; it is stopped", vm.id, vm.name);
            if !inner.gone && matches!(inner.state, VmState::Running | VmState::Paused) {
                vm.set_state(&mut inner, VmState::Stopped);
            }
        }
        inner.input = None;
        vm.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_keeps_the_latest_bytes() {
        let vm = Vm::new(
            "t".into(),
            HardwareProfile::with_ram(4096),
            MachineConfig::with_ram(4096),
            Vec::new(),
            b"not an ELF file",
            false,
        );
        assert!(matches!(
            vm,
            Err(Fail {
                code: ErrorCode::InvalidProgram,
                ..
            })
        ));

        let vm = Vm::new(
            "t".into(),
            HardwareProfile::with_ram(1 << 20),
            MachineConfig::with_ram(1 << 20),
            Vec::new(),
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../examples/hello/hello.elf"
            ))
            .unwrap(),
            false,
        )
        .unwrap();
        let mut inner = vm.lock();
        vm.console_output(&mut inner, vec![1; SCROLLBACK - 10]);
        vm.console_output(&mut inner, vec![2; 20]);
        assert_eq!(inner.scrollback.len(), SCROLLBACK);
        assert_eq!(inner.scrollback.iter().filter(|&&b| b == 2).count(), 20);
        assert_eq!(inner.scrollback.back(), Some(&2));
        vm.console_output(&mut inner, (0..=255).cycle().take(SCROLLBACK + 7).collect());
        assert_eq!(inner.scrollback.len(), SCROLLBACK);
        assert_eq!(inner.scrollback[0], 7);
    }
}
