//! One virtual machine: a Grail [`Machine`], its life cycle, its thread, its console
//! and its screen.
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
//! The screen (the framebuffer) streams to the clients attached to it, the viewers
//! (protocol doc, section 6.4). The VM keeps one [`Screen`]: exactly what every viewer
//! has been sent. Between slices the thread compares the framebuffer with it and sends
//! the rectangles that changed to every viewer; an unchanged screen sends nothing. Two
//! limits keep a busy guest from flooding the link:
//!
//! - **Rate**: the thread looks at the screen at most once per [`SCREEN_INTERVAL`]
//!   (~30 times a second), however often the guest redraws. Changes between two looks
//!   go out together, as their final pixels.
//! - **Backlog**: a viewer with more than [`SCREEN_BACKLOG`] bytes still queued (a slow
//!   link) is sent no rectangles, and is marked behind. Once its queue has drained it
//!   gets one full frame, which brings it level with the others. So a slow viewer sees
//!   fewer frames rather than falling ever further behind and being cut off.
//!
//! The comparison is skipped while nobody is attached; the VM only remembers that the
//! machine ran since its last look. A viewer that attaches is sent the screen as it
//! was last captured (with `ScreenAttached`, under the lock), and the thread's next
//! look brings it, like everyone else, up to date. When the machine ends (stopped,
//! exited) the thread captures the screen one last time, before the state change, so
//! viewers have the last frame by the time they hear of it, and later viewers are sent
//! it too.
//!
//! Keys and pointer changes from viewers queue here (at most [`MAX_INPUT_QUEUE`] of
//! each) and the thread hands them to the machine's keyboard and pointer devices
//! before its next slice, as Grail's own window does between slices.
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
//! Ending the cards as the machine ends takes their plug and switch locks under it,
//! which is the order [`crate::switch`] sets.

use crate::outbox::Outbox;
use crate::switch::Card;
use crate::{lock, Fail};
use avalon_proto::{
    AttachKind, DeviceEntry, DeviceSpec, ErrorCode, Frame, HardwareProfile, KeyEvent, Message, NetSpec,
    PointerChange, PointerMotion, Screen, VmDetails, VmState, VmSummary, PIXEL_FORMAT_XRGB8888,
};
use excalibur::csr::IRQ_M_TIMER;
use grail::framebuffer::{FB_HEIGHT, FB_WIDTH};
use grail::net::NetBackend;
use grail::pointer::{Motion, PointerInput};
use grail::{keyboard, Machine, MachineConfig};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Bytes of recent console output each VM keeps, sent to every client that attaches.
pub const SCROLLBACK: usize = 64 * 1024;

/// Steps per slice for a full-speed machine (a real-time one stops sooner, once it is
/// ~1 ms ahead of the wall clock): a few milliseconds of host time, so requests are
/// answered quickly without the thread spending its time on locks.
const SLICE: u64 = 100_000;

/// How long a full-speed machine parked in `wfi` sleeps before it looks again, if
/// nothing wakes it first (input and requests do).
const PARKED_NAP: Duration = Duration::from_millis(50);

/// Shortest time between two looks at the screen: at most ~30 updates a second go to
/// the viewers, however fast the guest draws.
pub const SCREEN_INTERVAL: Duration = Duration::from_millis(33);

/// A viewer with more than this many bytes queued and not yet written is sent no
/// screen updates until it has caught up (then one full frame). A little over one full
/// 640x480 frame (1.2 MB), and well below [`crate::MAX_QUEUED`], where a client is cut
/// off.
pub const SCREEN_BACKLOG: usize = 2 * 1024 * 1024;

/// Most keys, and most pointer changes, waiting for the machine. The devices' own
/// FIFOs hold 64; more than this waiting means the guest is not reading them (or is
/// paused), and further input is dropped, as a full FIFO drops it.
pub const MAX_INPUT_QUEUE: usize = 256;

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
    /// Times a frame for one of the cards rang the VM's bell ([`Vm::kick`]): how tests
    /// see that frames wake the VM.
    bells: AtomicU64,
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
    /// A `StopVm` waits for the thread: it captures the screen one last time, then
    /// moves the VM to stopped, so viewers get the last frame before the state change.
    stopping: bool,
    /// The UART's receiver. Bytes sent before the VM starts wait for it.
    input: Option<Sender<u8>>,
    scrollback: VecDeque<u8>,
    /// Clients attached to the console, at most once each.
    consoles: Vec<Arc<Outbox>>,
    /// Clients attached to the screen, at most once each.
    viewers: Vec<Viewer>,
    /// The screen as the viewers have it: what was captured last.
    screen: Screen,
    /// The machine has run since the last capture, so its framebuffer may differ.
    screen_owed: bool,
    /// When the screen was last looked at, for [`SCREEN_INTERVAL`].
    screen_looked: Option<Instant>,
    /// Viewers' keys and pointer changes, for the thread to hand to the machine.
    keys: VecDeque<keyboard::KeyEvent>,
    pointer: VecDeque<PointerInput>,
}

/// A client attached to the screen.
struct Viewer {
    outbox: Arc<Outbox>,
    /// Updates were held back from it (its queue was over [`SCREEN_BACKLOG`]): it gets a
    /// full frame once it has caught up.
    behind: bool,
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
            bells: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                state: VmState::Created,
                gone: false,
                machine: Some(machine),
                busy: false,
                kick: false,
                stopping: false,
                input: Some(input),
                scrollback: VecDeque::new(),
                consoles: Vec::new(),
                viewers: Vec::new(),
                screen: Screen::new(FB_WIDTH, FB_HEIGHT),
                screen_owed: false,
                screen_looked: None,
                keys: VecDeque::new(),
                pointer: VecDeque::new(),
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

    /// How many clients are attached to the screen.
    pub fn viewer_count(&self) -> usize {
        self.lock().viewers.len()
    }

    /// How many keys, and how many pointer changes, wait for the machine.
    pub fn queued_input(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.keys.len(), inner.pointer.len())
    }

    /// The VM as `VmList` shows it. Its cards are as they are now: their addresses
    /// (made up by the server if the profile set none), and the switches they are
    /// plugged into.
    pub fn summary(&self) -> VmSummary {
        // The state first, then the cards: a machine's cards are unplugged before its
        // state says it has ended, so a summary that says so never shows them plugged.
        let state = self.state();
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
            state,
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

    /// Makes a frame queued for any of the VM's cards ring its bell, which wakes the
    /// VM ([`Vm::kick`]). The server does this once the VM is in its `Arc`. The bells
    /// hold the VM weakly: the cards are the VM's, and must not keep it alive.
    pub fn ring_on_frames(self: &Arc<Self>) {
        for card in &self.cards {
            let weak = Arc::downgrade(self);
            card.set_bell(Arc::new(move || {
                if let Some(vm) = weak.upgrade() {
                    vm.kick();
                }
            }));
        }
    }

    /// Times a frame for one of the cards rang the bell.
    pub fn bells(&self) -> u64 {
        self.bells.load(Ordering::Relaxed)
    }

    /// Wakes the VM's thread if it is napping: a frame came in for one of its cards.
    /// A frame that comes while a slice runs makes the thread run another slice at
    /// once rather than nap.
    pub fn kick(&self) {
        self.bells.fetch_add(1, Ordering::Relaxed);
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
    ///
    /// A machine that has ended (stopped or exited) ends its network cards first: they
    /// are unplugged for good before anyone can hear of the new state, so a client told
    /// the VM has stopped (by this event, a `StopVm` reply or a `VmInfo`) never sees
    /// its cards on a switch, nor plugs one in (protocol doc, section 6.7). The machine
    /// itself may be dropped a moment later, by whichever thread holds it.
    fn set_state(&self, inner: &mut Inner, state: VmState) {
        if matches!(state, VmState::Stopped | VmState::Exited(_)) {
            for card in &self.cards {
                card.end();
            }
        }
        inner.state = state;
        if self.log {
            eprintln!("avalon: VM {} ('{}') {state}", self.id, self.name);
        }
        let event = event_frame(&Message::VmStateChanged { vm: self.id, state });
        inner.consoles.retain(|c| c.event(&event));
        // Once to each client, also to one attached to both the console and the screen.
        let consoles = &inner.consoles;
        inner
            .viewers
            .retain(|v| consoles.iter().any(|c| c.id == v.outbox.id) || v.outbox.event(&event));
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
    /// destroyed. A machine that has run is stopped by its thread, between two slices:
    /// it sends the viewers the last frame, then the state change, as an exit does.
    pub fn stop(&self) -> Result<(), Fail> {
        let mut inner = self.lock();
        match inner.state {
            VmState::Created => {
                let machine = inner.machine.take();
                self.set_state(&mut inner, VmState::Stopped);
                drop(inner);
                drop(machine);
                Ok(())
            }
            VmState::Running | VmState::Paused => {
                inner.stopping = true;
                self.wake.notify_all();
                // Until the thread has stopped the VM (or the VM is being destroyed, or
                // its thread crashed, which stops it too).
                while matches!(inner.state, VmState::Running | VmState::Paused) && !inner.gone {
                    inner = self.wait(inner);
                }
                drop(self.settle(inner));
                Ok(())
            }
            state => Err(wrong_state(self, state, "created, running or paused")),
        }
    }

    /// Ends the VM for good: its thread stops and is joined, its machine is dropped,
    /// and attached clients get `Detached`. The caller has taken it out of the table.
    pub fn destroy(&self) {
        let (consoles, viewers, machine) = {
            let mut inner = self.lock();
            inner.gone = true;
            // Its machine is ending: its cards go now, not when the thread lets go.
            for card in &self.cards {
                card.end();
            }
            inner.input = None;
            self.wake.notify_all();
            (
                std::mem::take(&mut inner.consoles),
                std::mem::take(&mut inner.viewers),
                inner.machine.take(),
            )
        };
        drop(machine);
        let viewers = viewers.into_iter().map(|v| v.outbox).collect();
        for (clients, kind) in [(consoles, AttachKind::Console), (viewers, AttachKind::Screen)] {
            let detached = event_frame(&Message::Detached { vm: self.id, kind });
            for client in clients {
                client.event(&detached);
            }
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

    /// Attaches `client` to the screen: queues `ScreenAttached` and a full frame of the
    /// screen as the other viewers have it, under the lock, so the updates that follow
    /// apply to it. Attaching again just sends the full frame again.
    pub fn attach_screen(&self, client: &Arc<Outbox>) -> Result<(), Fail> {
        let mut inner = self.lock();
        if inner.gone {
            return Err(Fail::new(
                ErrorCode::NoSuchVm,
                format!("VM {} is being destroyed", self.id),
            ));
        }
        client.reply_now(&Message::ScreenAttached {
            vm: self.id,
            width: inner.screen.width(),
            height: inner.screen.height(),
            format: PIXEL_FORMAT_XRGB8888,
        });
        let full = event_frame(&inner.screen.frame(self.id, inner.screen.bounds()));
        if client.event(&full) {
            match inner.viewers.iter_mut().find(|v| v.outbox.id == client.id) {
                Some(v) => v.behind = false,
                None => inner.viewers.push(Viewer {
                    outbox: Arc::clone(client),
                    behind: false,
                }),
            }
        }
        // A paused thread that owes the screen a look takes it now.
        self.wake.notify_all();
        Ok(())
    }

    /// Detaches client `id` from the screen. False if it was not attached.
    pub fn detach_screen(&self, id: u64) -> bool {
        let mut inner = self.lock();
        let before = inner.viewers.len();
        inner.viewers.retain(|v| v.outbox.id != id);
        inner.viewers.len() != before
    }

    /// A key from client `id` for the keyboard device. Dropped (false) unless that
    /// client is attached to the screen and the machine can still run, and also when
    /// [`MAX_INPUT_QUEUE`] keys are waiting already.
    pub fn key_input(&self, id: u64, key: KeyEvent) -> bool {
        let key = keyboard::KeyEvent {
            code: key.code,
            release: key.release,
            shift: key.shift,
            ctrl: key.ctrl,
        };
        self.input_event(id, |inner| queue(&mut inner.keys, key))
    }

    /// A pointer change from client `id` for the pointer device; dropped as
    /// [`Vm::key_input`] drops keys.
    pub fn pointer_input(&self, id: u64, change: PointerChange) -> bool {
        self.input_event(id, |inner| queue(&mut inner.pointer, pointer_input(change)))
    }

    fn input_event(&self, id: u64, push: impl FnOnce(&mut Inner) -> bool) -> bool {
        let mut inner = self.lock();
        let live = matches!(inner.state, VmState::Created | VmState::Running | VmState::Paused);
        if inner.gone || !live || !inner.viewers.iter().any(|v| v.outbox.id == id) || !push(&mut inner) {
            return false;
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
            let (keys, pointer) = {
                let mut inner = self.lock();
                loop {
                    if inner.gone || matches!(inner.state, VmState::Stopped | VmState::Exited(_)) {
                        self.last_look(&mut inner, &machine);
                        return;
                    }
                    if inner.stopping {
                        // `stop` waits for this: the last frame, then the state change.
                        self.last_look(&mut inner, &machine);
                        self.set_state(&mut inner, VmState::Stopped);
                        return;
                    }
                    if inner.state == VmState::Running {
                        break;
                    }
                    // Paused: the viewers still get what the last slices drew, and a
                    // viewer that falls behind still catches up.
                    self.look(&mut inner, &machine);
                    inner = match self.screen_due(&inner) {
                        Some(due) => {
                            let left = due.saturating_duration_since(Instant::now());
                            self.wake
                                .wait_timeout(inner, left)
                                .unwrap_or_else(PoisonError::into_inner)
                                .0
                        }
                        None => self.wait(inner),
                    };
                }
                inner.busy = true;
                inner.kick = false;
                (
                    std::mem::take(&mut inner.keys),
                    std::mem::take(&mut inner.pointer),
                )
            };

            // Viewers' input first, as Grail's window hands it over between slices.
            for key in keys {
                machine.key_event(key);
            }
            for change in pointer {
                machine.pointer_event(change);
            }
            let steps = machine.run_for(SLICE, &mut |_| {});
            // `run_for` ends a slice early once the guest has written FB_PRESENT, until
            // the host takes that frame: take it, or every later slice is a single step.
            // The screen goes out at the pace of `look`, not on presents: viewers see
            // the framebuffer, presented or not, as Grail's window does.
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
            if steps > 0 {
                inner.screen_owed = true;
            }
            if let Some(code) = machine.exit_code() {
                // The last frame before the state change, so that viewers have it by
                // the time they hear the program has ended.
                self.last_look(&mut inner, &machine);
                if !inner.gone && matches!(inner.state, VmState::Running | VmState::Paused) {
                    // A stop asked for during the slice wins, as it would have had it
                    // come a step earlier.
                    let end = if inner.stopping {
                        VmState::Stopped
                    } else {
                        VmState::Exited(code)
                    };
                    self.set_state(&mut inner, end);
                }
                return;
            }
            self.look(&mut inner, &machine);
            // Nothing to run right now: a real-time machine ahead of the wall clock, or
            // a full-speed one parked in wfi. Requests and input wake the thread early,
            // and so does the next look at the screen, when one is owed.
            let nap = if paced {
                machine.pace_delay()
            } else {
                parked(&machine).then_some(PARKED_NAP)
            };
            if let Some(mut nap) = nap {
                if let Some(due) = self.screen_due(&inner) {
                    nap = nap.min(due.saturating_duration_since(Instant::now()));
                }
                if inner.state == VmState::Running && !inner.kick && !inner.gone && !inner.stopping {
                    drop(self.wake.wait_timeout(inner, nap));
                }
            }
        }
    }

    /// When the thread should next look at the screen: `None` when nobody is attached
    /// or there is nothing to look for (the machine has not run since the last look,
    /// and no viewer is behind).
    fn screen_due(&self, inner: &Inner) -> Option<Instant> {
        let behind = inner.viewers.iter().any(|v| v.behind);
        if inner.viewers.is_empty() || !(inner.screen_owed || behind) {
            return None;
        }
        Some(
            inner
                .screen_looked
                .map_or_else(Instant::now, |t| t + SCREEN_INTERVAL),
        )
    }

    /// Looks at the screen if that is due: sends the viewers what changed since the
    /// last look, and a full frame to a viewer that has caught up.
    fn look(&self, inner: &mut Inner, machine: &Machine) {
        if self.screen_due(inner).is_some_and(|due| due <= Instant::now()) {
            self.update_viewers(inner, machine, false);
        }
    }

    /// The machine is ending: the last capture, made whether or not anyone is attached
    /// (it is what later viewers will be sent), and sent to every viewer, behind or not.
    fn last_look(&self, inner: &mut Inner, machine: &Machine) {
        inner.screen_owed = true;
        self.update_viewers(inner, machine, true);
    }

    /// Captures the screen if the machine has run since the last capture, and sends
    /// the rectangles that changed to every viewer that keeps up. A viewer over
    /// [`SCREEN_BACKLOG`] gets nothing and is marked behind; a viewer that is behind
    /// gets a full frame once it is under it again (or at once when `last`).
    fn update_viewers(&self, inner: &mut Inner, machine: &Machine, last: bool) {
        inner.screen_looked = Some(Instant::now());
        let rects = if std::mem::take(&mut inner.screen_owed) {
            let fb = &machine.cpu.bus.fb;
            // A display that is off shows black, as Grail's window does.
            inner.screen.update(fb.enabled().then_some(fb.vram.as_slice()))
        } else {
            Vec::new()
        };
        let id = self.id;
        let Inner { viewers, screen, .. } = inner;
        let frames: Vec<Frame> = rects.iter().map(|&r| event_frame(&screen.frame(id, r))).collect();
        let mut full = None;
        viewers.retain_mut(|v| {
            let keeping_up = v.outbox.queued() <= SCREEN_BACKLOG;
            if v.behind {
                if !(keeping_up || last) {
                    return true;
                }
                v.behind = false;
                let full = full.get_or_insert_with(|| event_frame(&screen.frame(id, screen.bounds())));
                return v.outbox.event(full);
            }
            if frames.is_empty() {
                return true;
            }
            if !(keeping_up || last) {
                v.behind = true;
                return true;
            }
            // `event` is false once the client is gone (or cut off): it is dropped.
            frames.iter().all(|f| v.outbox.event(f))
        });
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

/// Queues `item` unless [`MAX_INPUT_QUEUE`] are waiting. Returns whether it did.
fn queue<T>(q: &mut VecDeque<T>, item: T) -> bool {
    if q.len() >= MAX_INPUT_QUEUE {
        return false;
    }
    q.push_back(item);
    true
}

/// A viewer's pointer change as the input of Grail's pointer device. Positions past
/// `i32::MAX` are past the screen's edge either way: the device clamps them to it.
fn pointer_input(change: PointerChange) -> PointerInput {
    let motion = match change.motion {
        PointerMotion::None => Motion::None,
        PointerMotion::To { x, y } => Motion::To {
            x: x.min(i32::MAX as u32) as i32,
            y: y.min(i32::MAX as u32) as i32,
        },
        PointerMotion::By { dx, dy } => Motion::By { dx, dy },
    };
    PointerInput {
        motion,
        buttons: change.buttons,
        wheel: change.wheel as i32,
    }
}

/// Whether a full-speed machine is stalled in `wfi` with only an external interrupt
/// (input) left to wake it: every hart waits with no interrupt pending, and no hart's
/// timer can fire (it is masked or not set; a set timer would end the wait at the next
/// step).
fn parked(machine: &Machine) -> bool {
    let cpu = &machine.cpu;
    let timer_armed = |hart: &grail::cpu::Hart| {
        hart.csr.mie & (1 << IRQ_M_TIMER) != 0 && cpu.bus.clint.mtimecmp[hart.id as usize] != u64::MAX
    };
    cpu.all_idle() && !cpu.harts().iter().any(timer_armed)
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
    use crate::switch::Switch;
    use grail::net::MacAddr;

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

    /// A VM with one network card, in state created.
    fn vm_with_a_card(mac: MacAddr) -> Arc<Vm> {
        let vm = Vm::new(
            "t".into(),
            HardwareProfile::with_ram(1 << 20),
            MachineConfig::with_ram(1 << 20),
            vec![Card::new(0, mac)],
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../examples/hello/hello.elf"
            ))
            .unwrap(),
            false,
        )
        .unwrap();
        Arc::new(vm)
    }

    #[test]
    fn a_frame_for_a_card_wakes_its_vm() {
        let vm = vm_with_a_card(MacAddr([2, 0, 0, 0, 0, 1]));
        vm.ring_on_frames();
        let lan = Switch::new("lan");
        let owner = |vm| avalon_proto::PortOwner::Card { vm, card: 0 };
        vm.card(0).unwrap().plug(owner(1), &lan).unwrap();
        let other = Card::new(0, MacAddr([2, 0, 0, 0, 0, 2]));
        other.plug(owner(2), &lan).unwrap();

        // Something waits on the VM's condition variable for a kick, as its thread
        // naps. `busy` (otherwise unused before the VM starts) says it is waiting: it
        // is set under the lock, which `wait` lets go of only once it waits.
        let waiter = {
            let vm = Arc::clone(&vm);
            std::thread::spawn(move || {
                let mut inner = vm.lock();
                inner.busy = true;
                let (inner, timeout) = vm
                    .wake
                    .wait_timeout_while(inner, Duration::from_secs(10), |i| !i.kick)
                    .unwrap();
                (inner.kick, timeout.timed_out())
            })
        };
        while !vm.lock().busy {
            std::thread::yield_now();
        }
        assert_eq!(vm.bells(), 0);
        let mut frame = vec![0xff; 6];
        frame.extend([2, 0, 0, 0, 0, 2, 0x88, 0xb5]);
        frame.resize(60, 0);
        other.backend().send(&frame).unwrap();
        // Woken by the frame, not by the 10 s timeout.
        assert_eq!(waiter.join().unwrap(), (true, false));
        assert_eq!(vm.bells(), 1);
    }

    #[test]
    fn a_stopped_vm_has_unplugged_its_cards_by_the_time_anyone_hears_of_it() {
        let vm = vm_with_a_card(MacAddr([2, 0, 0, 0, 0, 1]));
        let lan = Switch::new("lan");
        let card = Arc::clone(vm.card(0).unwrap());
        card.plug(avalon_proto::PortOwner::Card { vm: 1, card: 0 }, &lan)
            .unwrap();
        // Holding the machine as the VM thread does past the state change.
        let machine = vm.lock().machine.take();
        {
            let mut inner = vm.lock();
            vm.set_state(&mut inner, VmState::Stopped);
        }
        assert!(lan.info().ports.is_empty());
        assert!(card.switch().is_none());
        assert_eq!(
            card.plug(avalon_proto::PortOwner::Card { vm: 1, card: 0 }, &lan)
                .unwrap_err()
                .code,
            ErrorCode::InvalidState
        );
        drop(machine);
    }
}
