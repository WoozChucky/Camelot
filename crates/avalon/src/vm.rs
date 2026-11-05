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
//! has been sent. Between slices the thread compares what the framebuffer scans out
//! (VRAM or a buffer in RAM, at its mode's size) with it and sends the rectangles that
//! changed to every viewer; an unchanged screen sends nothing. A screen whose size
//! changed (the guest set another mode) starts again at the new size: `ScreenResized`
//! to every viewer, then the whole screen. The framebuffer's cursor plane is not in the
//! screen: the VM keeps a [`Cursor`] too, what the viewers have of it, and each look
//! sends after the rectangles what changed of it, its image (`CursorShape`) or its place
//! (`CursorPosition`), so a cursor that moves sends no rectangle. Two limits keep a busy
//! guest from flooding the link:
//!
//! - **Rate**: the thread looks at the screen at most once per [`SCREEN_INTERVAL`]
//!   (~30 times a second), however often the guest redraws. Changes between two looks
//!   go out together, as their final pixels.
//! - **Backlog**: a viewer with more than [`screen_backlog`] bytes still queued (a slow
//!   link), a little over one full frame of the screen, is sent no rectangles and nothing
//!   of the cursor (an image is up to 256 KiB), and is marked behind. Once its queue has
//!   drained it gets one full frame and the whole cursor, which bring it level with the
//!   others. So a slow viewer sees fewer frames rather than falling ever further behind
//!   and being cut off.
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
//! before its next slice, as Grail's own window does between slices. A viewer that
//! goes (it detaches, or its connection ends) lets go of what it held: the VM queues
//! a release for every key it has down and every pointer button it holds, as Linux's
//! input core releases the keys of a keyboard that is unplugged
//! (`input_dev_release_keys`), so a viewer closed with Alt+F4 leaves no Alt held.
//!
//! A VM can have a debugger (protocol doc, section 6.8): a client whose bytes are the
//! GDB remote serial protocol, for the stub `grail --gdb` serves ([`gdb::Session`]).
//! The thread is the stub's host, as `grail --gdb`'s loop is: between slices it hands
//! the debugger's bytes to the session, which answers them, runs the debugger's steps,
//! and while the machine has breakpoints or watchpoints it runs its slices with
//! [`Machine::debug_continue`], which stops at them, instead of [`Machine::run_for`].
//! The run states are QEMU's (`gdbstub/system.c`): the debugger's arrival, its Ctrl-C
//! and a `PauseVm` stop the VM as paused (SIGINT, `T02`, to a debugger waiting for a
//! stop reply); a breakpoint, a watchpoint or the end of a step stops it as debug
//! (SIGTRAP); a continue and a step run it (running). The console, the screen and
//! input go on as for any paused VM. A debugger that detaches (`D`) leaves the machine
//! without its breakpoints, running; one whose connection ends leaves it as it was.
//! While a VM has a debugger its state notices are rate-limited ([`NOTICE_RATE`]): a
//! `next` may step a million times, each step a running and a debug.
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
    AttachKind, Cursor, CursorView, DeviceEntry, DeviceSpec, ErrorCode, Frame, HardwareProfile, KeyEvent,
    Message, NetSpec, PointerChange, PointerMotion, Screen, VmDetails, VmState, VmSummary,
    PIXEL_FORMAT_XRGB8888,
};
use excalibur::csr::IRQ_M_TIMER;
use grail::framebuffer::{FB_HEIGHT, FB_WIDTH};
use grail::gdb::packet::Decoder;
use grail::gdb::{self, StopReason};
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

/// The least backlog a viewer has ([`screen_backlog`]), whatever the screen's size: what
/// it was when every screen was 640x480, 1.7 full frames of it.
pub const SCREEN_BACKLOG_MIN: usize = 2 * 1024 * 1024;
/// How much more than one full frame of the screen a viewer's backlog is.
pub const SCREEN_BACKLOG_MARGIN: usize = 512 * 1024;

/// A viewer with more than this many bytes queued and not yet written is sent no screen
/// or cursor updates until it has caught up (then one full frame and the cursor). It
/// follows the size of the screen, `width` × `height`: a little over one full frame of it
/// ([`SCREEN_BACKLOG_MARGIN`] more), and at least [`SCREEN_BACKLOG_MIN`]. So a slow viewer
/// of a small screen falls no further behind (in frames, and in time on its link) than it
/// would if no bigger screen could be shown; one of a screen in the framebuffer's largest
/// mode (1920x1080, 8.3 MB a frame) may have 8.4 MiB queued, below [`crate::MAX_QUEUED`]
/// by more than two more full frames (and a frame is far more than a cursor's image), so
/// a viewer that is held back is never cut off.
pub const fn screen_backlog(width: u32, height: u32) -> usize {
    let backlog = width as usize * height as usize * 4 + SCREEN_BACKLOG_MARGIN;
    if backlog > SCREEN_BACKLOG_MIN {
        backlog
    } else {
        SCREEN_BACKLOG_MIN
    }
}
const _: () = {
    let largest = grail::framebuffer::MODES[grail::framebuffer::MODES.len() - 1];
    let backlog = screen_backlog(largest.width, largest.height) as u64;
    assert!(backlog > grail::framebuffer::MAX_FRAME_SIZE);
    assert!(crate::outbox::MAX_QUEUED as u64 > backlog + 2 * grail::framebuffer::MAX_FRAME_SIZE);
};

/// The shortest time between two state notices (`VmStateChanged`) of a VM that has a
/// debugger, as QEMU's monitor rate-limits its noisy events (`monitor_qapi_event_queue`,
/// once a second): the first change goes out at once; the ones within the next second
/// are held, only the last kept, and that one goes out when the second ends, unless the
/// VM is back where the last notice left it. Without the limit a debugger's steps,
/// each of them two changes (running, then debug), would flood every client attached
/// to the VM. The end of the machine (stopped, exited) is never held. A state request
/// (`GetVmInfo`) always has the state as it is.
pub const NOTICE_RATE: Duration = Duration::from_secs(1);

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
    /// The cursor as the viewers have it, captured with the screen.
    cursor: Cursor,
    /// The machine has run since the last capture, so its framebuffer may differ.
    screen_owed: bool,
    /// When the screen was last looked at, for [`SCREEN_INTERVAL`].
    screen_looked: Option<Instant>,
    /// Viewers' keys and pointer changes, for the thread to hand to the machine.
    keys: VecDeque<keyboard::KeyEvent>,
    pointer: VecDeque<PointerInput>,
    /// The client attached as the VM's debugger (protocol doc, section 6.8), and what
    /// it has sent the stub that the thread has not taken yet.
    debugger: Option<Debugger>,
    /// The debugger the thread has a session for, once it has made one (and stopped the
    /// machine for it): what `attach_debugger` and `detach_debugger` wait for.
    linked: Option<u64>,
    /// A debugger that went without `D` (its connection ended, or it could no longer be
    /// written to): the thread hands the stub what it sent before it went, then lets go
    /// of its session and leaves the machine as it is, stopped or running, its
    /// breakpoints and watchpoints kept, as QEMU does when a GDB connection closes.
    dropped: Option<Dropped>,
    /// The state notices held back by [`NOTICE_RATE`].
    notices: Notices,
}

/// The rate limit of a VM's state notices ([`NOTICE_RATE`]).
struct Notices {
    /// The state the last notice gave.
    sent: VmState,
    /// When the last notice the limit applied to went out: until a [`NOTICE_RATE`]
    /// later, changes are held.
    at: Option<Instant>,
    /// The last change held.
    pending: Option<VmState>,
}

/// A client attached as a VM's debugger.
struct Debugger {
    outbox: Arc<Outbox>,
    /// Bytes of the GDB remote serial protocol for the stub.
    input: Vec<u8>,
}

/// A debugger gone without `D`, and the bytes it sent that the thread had not taken
/// yet. QEMU's stub reads a connection's bytes in order and sees its end after the
/// last of them (`gdb_chr_receive`, then `CHR_EVENT_CLOSED`), so a `c` sent just before
/// the connection closed still runs the VM: these still go to the stub, before the
/// thread lets go of the session.
struct Dropped {
    client: u64,
    input: Vec<u8>,
}

impl Dropped {
    /// `debugger` is gone without `D`.
    fn from(debugger: Debugger) -> Dropped {
        Dropped {
            client: debugger.outbox.id,
            input: debugger.input,
        }
    }
}

/// What the thread does once it has looked at the VM.
enum Work {
    /// Hand the debugger's bytes to the stub.
    Debug(Vec<u8>),
    /// Run a slice, after handing these keys and pointer changes to the machine.
    Slice(VecDeque<keyboard::KeyEvent>, VecDeque<PointerInput>),
}

/// The thread's side of a debugger: its session with Grail's stub ([`gdb::Session`],
/// the stub `grail --gdb` serves), the decoder of its bytes, and whether it waits for a
/// stop reply. The thread owns the machine, so it is the stub's host, as `grail --gdb`'s
/// main loop is.
struct DebugLink {
    client: u64,
    session: gdb::Session,
    decoder: Decoder,
    /// The debugger resumed the machine (`c`) and waits for its stop reply.
    waiting: bool,
}

/// How a batch of the debugger's bytes left the link.
enum LinkEnd {
    /// Still the debugger's: stopped, or resumed (`waiting`).
    Linked,
    /// `D`: `OK` sent; the debugger has detached.
    Detach,
    /// `k`: `X09` sent; the VM stops.
    Kill,
    /// A step reached the exit: `W` sent.
    Exited(u32),
}

impl DebugLink {
    /// Hands the debugger's bytes to the stub, as `grail --gdb` does with what its
    /// client sends: stopped, each packet is answered and a step runs here; resumed,
    /// only a Ctrl-C counts, which stops the machine (all-stop mode). `state` is told
    /// each run state the debugger moves the VM to, as QEMU's: running for a continue
    /// (`vm_start`) and for a step (`vm_prepare_start`), debug at the step's end, paused
    /// for a Ctrl-C. Returns what to send the debugger, and how the link stands.
    fn input(
        &mut self,
        machine: &mut Machine,
        bytes: &[u8],
        state: &mut impl FnMut(VmState),
    ) -> (Vec<u8>, LinkEnd) {
        let mut out = Vec::new();
        for &b in bytes {
            let Some(event) = self.decoder.push(b) else {
                continue;
            };
            if self.waiting {
                if gdb::Session::interrupts(&event) {
                    self.session.stopped(machine, StopReason::Interrupted, &mut out);
                    self.waiting = false;
                    state(VmState::Paused);
                }
                continue;
            }
            match self.session.event(machine, event, &mut out) {
                gdb::Next::Wait => {}
                gdb::Next::Continue => {
                    self.waiting = true;
                    state(VmState::Running);
                }
                gdb::Next::Step => {
                    state(VmState::Running);
                    if let Some(code) = self.session.step(machine, &mut |_| {}, &mut out) {
                        return (out, LinkEnd::Exited(code));
                    }
                    state(VmState::Debug);
                }
                gdb::Next::Detach => return (out, LinkEnd::Detach),
                gdb::Next::Kill => return (out, LinkEnd::Kill),
                gdb::Next::Exited(code) => return (out, LinkEnd::Exited(code)),
            }
        }
        (out, LinkEnd::Linked)
    }
}

/// A client attached to the screen.
struct Viewer {
    outbox: Arc<Outbox>,
    /// Updates were held back from it (its queue was over [`screen_backlog`]): it gets a
    /// full frame once it has caught up.
    behind: bool,
    /// The keys it has down, as their presses went to the machine, in the order they
    /// went down: released when it goes ([`Viewer::let_go`]).
    keys: Vec<keyboard::KeyEvent>,
    /// The pointer buttons its last pointer change held.
    buttons: u8,
}

impl Viewer {
    fn new(outbox: &Arc<Outbox>) -> Viewer {
        Viewer {
            outbox: Arc::clone(outbox),
            behind: false,
            keys: Vec::new(),
            buttons: 0,
        }
    }

    /// Key `key` went to the machine: a press is held until its release. A key is the
    /// physical key its usage names, or, with no usage the HID page defines, its ASCII
    /// code; one that is neither reaches no mode and is not kept. So there are at most
    /// as many held keys as the two tables have entries.
    fn key(&mut self, key: keyboard::KeyEvent) {
        let Some(id) = key_id(&key) else {
            return;
        };
        let at = self.keys.iter().position(|k| key_id(k) == Some(id));
        match (key.release, at) {
            (false, None) => self.keys.push(key),
            (true, Some(at)) => {
                self.keys.remove(at);
            }
            // A repeat of a key held, or the release of one that was not.
            _ => {}
        }
    }

    /// The viewer is going: queues a release for each key it has down, in the order
    /// they went down, with the code and usage of its press, and a pointer change that
    /// lets go of its buttons. These go past [`MAX_INPUT_QUEUE`]: there are few of
    /// them, and a release lost would leave the key stuck.
    fn let_go(self, keys: &mut VecDeque<keyboard::KeyEvent>, pointer: &mut VecDeque<PointerInput>) {
        for key in self.keys {
            keys.push_back(keyboard::KeyEvent::release(key.code).with_usage(key.usage));
        }
        if self.buttons != 0 {
            pointer.push_back(PointerInput {
                motion: Motion::None,
                buttons: 0,
                wheel: 0,
            });
        }
    }
}

/// Which key `key` is, for [`Viewer::key`]: its usage if the HID page defines it,
/// otherwise 0x1_0000 plus its ASCII code if the spec's table has it.
fn key_id(key: &keyboard::KeyEvent) -> Option<u32> {
    if keyboard::usage::is_valid(key.usage) {
        Some(key.usage as u32)
    } else if keyboard::code::is_valid(key.code) {
        Some(0x1_0000 | key.code as u32)
    } else {
        None
    }
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
    /// unplugged), and loads `program` into it (with an empty one, the machine boots
    /// from its disk 0), as VM 0 until the server gives it an id. Errors:
    /// `InvalidProfile` (a disk image that cannot be opened) and `InvalidProgram`
    /// (also no disk to boot from). This can take a while (it opens files on the host and
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
        // No program: the machine boots from its disk 0, as `grail --disk IMG` does.
        let loaded = if program.is_empty() {
            machine.boot_from_disk()
        } else {
            machine.load_elf(program)
        };
        loaded.map_err(|e| Fail::new(ErrorCode::InvalidProgram, format!("program: {e}")))?;
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
                // Every device is below 4 GiB; the protocol's 32-bit fields widen with #158.
                base: u32::try_from(d.base).expect("devices are below 4 GiB"),
                size: u32::try_from(d.size).expect("devices are below 4 GiB"),
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
                cursor: Cursor::default(),
                screen_owed: false,
                screen_looked: None,
                keys: VecDeque::new(),
                pointer: VecDeque::new(),
                debugger: None,
                linked: None,
                dropped: None,
                notices: Notices {
                    sent: VmState::Created,
                    at: None,
                    pending: None,
                },
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

    /// How many viewers are behind: held back over [`screen_backlog`], owed a full frame.
    pub fn behind_count(&self) -> usize {
        self.lock().viewers.iter().filter(|v| v.behind).count()
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

    /// Sets the state and tells every attached client, now or, while the VM has a
    /// debugger, within [`NOTICE_RATE`].
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
        let notices = &mut inner.notices;
        let ended = matches!(state, VmState::Stopped | VmState::Exited(_));
        if inner.debugger.is_some() && !ended {
            let now = Instant::now();
            if notices.at.is_some_and(|at| now < at + NOTICE_RATE) {
                // The thread sends it when the second is over.
                notices.pending = Some(state);
                self.wake.notify_all();
                return;
            }
            notices.at = Some(now);
        } else {
            notices.at = None;
        }
        notices.pending = None;
        self.notify(inner, state);
    }

    /// When the state notice held back must go out, if one is.
    fn notice_due(inner: &Inner) -> Option<Instant> {
        let n = &inner.notices;
        n.pending.and(n.at).map(|at| at + NOTICE_RATE)
    }

    /// Sends the held state notice if its time has come (the thread looks between
    /// slices and while it waits): unless the VM is in the state the last notice gave,
    /// which starts another second of the limit.
    fn send_due_notice(&self, inner: &mut Inner) {
        if Vm::notice_due(inner).is_none_or(|due| Instant::now() < due) {
            return;
        }
        let state = inner.notices.pending.take().expect("a notice is due");
        if state == inner.notices.sent {
            return;
        }
        inner.notices.at = Some(Instant::now());
        self.notify(inner, state);
    }

    /// Tells every attached client the VM is in `state` (`VmStateChanged`).
    fn notify(&self, inner: &mut Inner, state: VmState) {
        inner.notices.sent = state;
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
        let Inner {
            consoles,
            viewers,
            debugger,
            dropped,
            ..
        } = inner;
        if let Some(d) = debugger {
            let told = consoles.iter().any(|c| c.id == d.outbox.id)
                || viewers.iter().any(|v| v.outbox.id == d.outbox.id);
            if !told && !d.outbox.event(&event) {
                // Gone without `D`.
                *dropped = debugger.take().map(Dropped::from);
            }
        }
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
        self.spawn(&mut inner, VmState::Running)
    }

    /// Moves the machine of a created VM onto a thread of its own, the VM in `state`.
    fn spawn(self: &Arc<Self>, inner: &mut Inner, state: VmState) -> Result<(), Fail> {
        let machine = inner.machine.take().expect("a created VM has its machine");
        let vm = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name(format!("vm-{}", self.id))
            .spawn(move || vm.run(machine));
        match spawned {
            Ok(handle) => {
                *lock(&self.thread) = Some(handle);
                self.set_state(inner, state);
                Ok(())
            }
            Err(e) => {
                // The machine went with the closure that could not run.
                self.set_state(inner, VmState::Stopped);
                Err(Fail::new(
                    ErrorCode::Internal,
                    format!("cannot start a VM thread: {e}"),
                ))
            }
        }
    }

    /// running → paused. Once this returns the machine runs no more steps. A debugger
    /// waiting for a stop reply gets one (`T02`), as from QEMU's `stop`.
    pub fn pause(&self) -> Result<(), Fail> {
        let mut inner = self.lock();
        if inner.state != VmState::Running {
            return Err(wrong_state(self, inner.state, "running"));
        }
        self.set_state(&mut inner, VmState::Paused);
        drop(self.settle(inner));
        Ok(())
    }

    /// paused → running, and debug → running for a VM without a debugger (one whose
    /// debugger's connection ended at a stop), as QEMU's `cont`. A VM with a debugger,
    /// stopped, is the debugger's to resume: it is waiting for no stop reply, and would
    /// not be told when the VM stops again.
    pub fn resume(&self) -> Result<(), Fail> {
        let mut inner = self.lock();
        if !matches!(inner.state, VmState::Paused | VmState::Debug) {
            return Err(wrong_state(self, inner.state, "paused"));
        }
        if inner.debugger.is_some() {
            return Err(Fail::new(
                ErrorCode::InvalidState,
                format!(
                    "VM {} ('{}') is {}, held by its debugger: the debugger resumes it",
                    self.id, self.name, inner.state
                ),
            ));
        }
        self.set_state(&mut inner, VmState::Running);
        Ok(())
    }

    /// created, running, paused or debug → stopped. The machine is dropped (by the thread,
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
            VmState::Running | VmState::Paused | VmState::Debug => {
                inner.stopping = true;
                self.wake.notify_all();
                // Until the thread has stopped the VM (or the VM is being destroyed, or
                // its thread crashed, which stops it too).
                while matches!(inner.state, VmState::Running | VmState::Paused | VmState::Debug)
                    && !inner.gone
                {
                    inner = self.wait(inner);
                }
                drop(self.settle(inner));
                Ok(())
            }
            state => Err(wrong_state(self, state, "created, running, paused or debug")),
        }
    }

    /// Ends the VM for good: its thread stops and is joined, its machine is dropped,
    /// and attached clients get `Detached`. The caller has taken it out of the table.
    pub fn destroy(&self) {
        let (consoles, viewers, debugger, machine) = {
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
                inner.debugger.take(),
                inner.machine.take(),
            )
        };
        drop(machine);
        let viewers = viewers.into_iter().map(|v| v.outbox).collect();
        let debugger = debugger.into_iter().map(|d| d.outbox).collect();
        for (clients, kind) in [
            (consoles, AttachKind::Console),
            (viewers, AttachKind::Screen),
            (debugger, AttachKind::Debug),
        ] {
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
            if matches!(
                inner.state,
                VmState::Created | VmState::Running | VmState::Paused | VmState::Debug
            ) {
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

    /// Makes `client` the VM's debugger (protocol doc, section 6.8): the VM stops for it
    /// as QEMU's does for a GDB connection, paused (`vm_stop(RUN_STATE_PAUSED)`). A
    /// created VM starts paused, before its first instruction (as `grail --gdb-wait`);
    /// a running one stops between two slices; a paused one, or one in state debug (its
    /// last debugger's connection ended at a stop), stays as it is. Returns once the
    /// thread has stopped the machine. Errors: `Busy` (it has a debugger),
    /// `InvalidState` (its machine has ended).
    pub fn attach_debugger(self: &Arc<Self>, client: &Arc<Outbox>) -> Result<(), Fail> {
        let mut inner = self.lock();
        if inner.gone {
            return Err(Fail::new(
                ErrorCode::NoSuchVm,
                format!("VM {} is being destroyed", self.id),
            ));
        }
        if let Some(d) = &inner.debugger {
            let who = if d.outbox.id == client.id {
                "this connection"
            } else {
                "another client"
            };
            return Err(Fail::new(
                ErrorCode::Busy,
                format!("VM {} ('{}') has a debugger already: {who}", self.id, self.name),
            ));
        }
        const LIVE: &str = "created, running, paused or debug";
        if !matches!(
            inner.state,
            VmState::Created | VmState::Running | VmState::Paused | VmState::Debug
        ) {
            return Err(wrong_state(self, inner.state, LIVE));
        }
        inner.debugger = Some(Debugger {
            outbox: Arc::clone(client),
            input: Vec::new(),
        });
        if inner.state == VmState::Created {
            self.spawn(&mut inner, VmState::Paused)?;
        }
        self.wake.notify_all();
        // Until the thread has its session (the machine stopped for the debugger), or the
        // machine has ended meanwhile.
        while inner.linked != Some(client.id)
            && inner.debugger.as_ref().is_some_and(|d| d.outbox.id == client.id)
            && !inner.gone
        {
            inner = self.wait(inner);
        }
        if inner.linked == Some(client.id) {
            Ok(())
        } else {
            Err(wrong_state(self, inner.state, LIVE))
        }
    }

    /// Client `id` stops being the VM's debugger, as after GDB's `D`: the thread clears
    /// the breakpoints and watchpoints and runs a VM that is stopped (paused or debug)
    /// on, as QEMU's `D` does (`gdb_continue`). Returns once it has. False if `id` was
    /// not its debugger.
    pub fn detach_debugger(&self, id: u64) -> bool {
        self.lose_debugger(id, false)
    }

    /// Client `id`'s connection has ended: if it was the VM's debugger, the VM is left
    /// as it is, stopped or running, with its breakpoints and watchpoints, as QEMU
    /// leaves a VM whose GDB connection closes. The next debugger's `?` removes them.
    pub fn debugger_gone(&self, id: u64) {
        self.lose_debugger(id, true);
    }

    fn lose_debugger(&self, id: u64, dropped: bool) -> bool {
        let mut inner = self.lock();
        if inner.debugger.as_ref().map(|d| d.outbox.id) != Some(id) {
            return false;
        }
        let debugger = inner.debugger.take();
        if dropped {
            inner.dropped = debugger.map(Dropped::from);
        }
        self.wake.notify_all();
        while inner.linked == Some(id) && !inner.gone {
            inner = self.wait(inner);
        }
        true
    }

    /// Bytes from client `id` for the stub. Dropped (false) unless it is the debugger.
    pub fn debug_input(&self, id: u64, data: &[u8]) -> bool {
        let mut inner = self.lock();
        match &mut inner.debugger {
            Some(d) if d.outbox.id == id => d.input.extend_from_slice(data),
            _ => return false,
        }
        inner.kick = true;
        self.wake.notify_all();
        true
    }

    /// Whether the VM has a debugger.
    pub fn debugged(&self) -> bool {
        self.lock().debugger.is_some()
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
        // Then the cursor, its image and, if it shows, its place.
        let cursor = inner.cursor.messages(self.id);
        if client.event(&full) && cursor.iter().all(|m| client.event(&event_frame(m))) {
            match inner.viewers.iter_mut().find(|v| v.outbox.id == client.id) {
                Some(v) => v.behind = false,
                None => inner.viewers.push(Viewer::new(client)),
            }
        }
        // A paused thread that owes the screen a look takes it now.
        self.wake.notify_all();
        Ok(())
    }

    /// Detaches client `id` from the screen, releasing the keys and buttons it held.
    /// False if it was not attached.
    pub fn detach_screen(&self, id: u64) -> bool {
        let mut inner = self.lock();
        let Some(at) = inner.viewers.iter().position(|v| v.outbox.id == id) else {
            return false;
        };
        let viewer = inner.viewers.remove(at);
        let Inner { keys, pointer, .. } = &mut *inner;
        viewer.let_go(keys, pointer);
        inner.kick = true;
        self.wake.notify_all();
        true
    }

    /// A key from client `id` for the keyboard device. Dropped (false) unless that
    /// client is attached to the screen and the machine can still run, and also when
    /// [`MAX_INPUT_QUEUE`] keys are waiting already.
    pub fn key_input(&self, id: u64, key: KeyEvent) -> bool {
        let key = keyboard::KeyEvent {
            code: key.code,
            usage: key.usage,
            release: key.release,
            repeat: key.repeat,
            shift: key.shift,
            ctrl: key.ctrl,
        };
        self.input_event(id, |inner, at| {
            let queued = queue(&mut inner.keys, key);
            if queued {
                inner.viewers[at].key(key);
            }
            queued
        })
    }

    /// A pointer change from client `id` for the pointer device; dropped as
    /// [`Vm::key_input`] drops keys.
    pub fn pointer_input(&self, id: u64, change: PointerChange) -> bool {
        self.input_event(id, |inner, at| {
            let queued = queue(&mut inner.pointer, pointer_input(change));
            if queued {
                inner.viewers[at].buttons = change.buttons;
            }
            queued
        })
    }

    /// Queues input from client `id` with `push`, which is given the client's place in
    /// the viewers, if it is one and the machine can still run.
    fn input_event(&self, id: u64, push: impl FnOnce(&mut Inner, usize) -> bool) -> bool {
        let mut inner = self.lock();
        let live = matches!(
            inner.state,
            VmState::Created | VmState::Running | VmState::Paused | VmState::Debug
        );
        let Some(at) = inner.viewers.iter().position(|v| v.outbox.id == id) else {
            return false;
        };
        if inner.gone || !live || !push(&mut inner, at) {
            return false;
        }
        inner.kick = true;
        self.wake.notify_all();
        true
    }

    /// The VM thread: runs the machine in slices while the VM is running, and serves its
    /// debugger, if it has one, between slices.
    fn run(self: Arc<Self>, mut machine: Machine) {
        let _guard = ThreadGuard(&self);
        let paced = machine.config().clock_hz.is_some();
        // The debugger's session with the machine's stub, while there is a debugger.
        let mut link: Option<DebugLink> = None;
        loop {
            let work = {
                let mut inner = self.lock();
                let work = loop {
                    if inner.gone || matches!(inner.state, VmState::Stopped | VmState::Exited(_)) {
                        self.last_look(&mut inner, &machine);
                        self.end_debugger(&mut inner, link.take());
                        return;
                    }
                    if inner.stopping {
                        // `stop` waits for this: the last frame, then the state change.
                        self.last_look(&mut inner, &machine);
                        self.set_state(&mut inner, VmState::Stopped);
                        self.end_debugger(&mut inner, link.take());
                        return;
                    }
                    self.send_due_notice(&mut inner);
                    // What a debugger gone without `D` sent before it went reaches the
                    // stub before its session ends.
                    if let Some(input) = Vm::dropped_input(&mut inner, &link) {
                        break Work::Debug(input);
                    }
                    self.link_debugger(&mut inner, &mut link, &mut machine);
                    if let Some(l) = link.as_mut().filter(|l| l.waiting) {
                        if inner.state == VmState::Paused {
                            // PauseVm while the debugger waited: its stop reply, as QEMU's
                            // `gdb_vm_state_change` sends one for any stop (SIGINT).
                            let mut out = Vec::new();
                            l.session.stopped(&mut machine, StopReason::Interrupted, &mut out);
                            l.waiting = false;
                            self.debug_output(&mut inner, out);
                        }
                    }
                    let input = inner.debugger.as_mut().map(|d| std::mem::take(&mut d.input));
                    if let Some(input) = input.filter(|i| !i.is_empty()) {
                        break Work::Debug(input);
                    }
                    if inner.state == VmState::Running {
                        break Work::Slice(
                            std::mem::take(&mut inner.keys),
                            std::mem::take(&mut inner.pointer),
                        );
                    }
                    // Paused, or stopped for the debugger: the viewers still get what
                    // the last slices drew, and a viewer that falls behind still
                    // catches up.
                    self.look(&mut inner, &machine);
                    let due = [self.screen_due(&inner), Vm::notice_due(&inner)];
                    inner = match due.into_iter().flatten().min() {
                        Some(due) => {
                            let left = due.saturating_duration_since(Instant::now());
                            self.wake
                                .wait_timeout(inner, left)
                                .unwrap_or_else(PoisonError::into_inner)
                                .0
                        }
                        None => self.wait(inner),
                    };
                };
                inner.busy = true;
                inner.kick = false;
                work
            };
            let (keys, pointer) = match work {
                Work::Debug(input) => {
                    let link = link.as_mut().expect("a debugger's input comes with its link");
                    let before = machine.debug_steps();
                    // The run states the debugger moves the VM through. A request that
                    // came meanwhile wins: a stop, or a pause during a step.
                    let mut moved = |to: VmState| {
                        let mut inner = self.lock();
                        let from = inner.state;
                        let follows = match to {
                            VmState::Running => matches!(from, VmState::Paused | VmState::Debug),
                            _ => from == VmState::Running,
                        };
                        if follows && !inner.gone && !inner.stopping {
                            self.set_state(&mut inner, to);
                        }
                    };
                    let (out, end) = link.input(&mut machine, &input, &mut moved);
                    self.steps
                        .fetch_add(machine.debug_steps() - before, Ordering::Relaxed);
                    let output = self.output.take();
                    let mut inner = self.lock();
                    inner.busy = false;
                    self.wake.notify_all();
                    if !output.is_empty() {
                        self.console_output(&mut inner, output);
                    }
                    self.debug_output(&mut inner, out);
                    match end {
                        LinkEnd::Linked => {}
                        LinkEnd::Detach => {
                            if inner.debugger.as_ref().map(|d| d.outbox.id) == Some(link.client) {
                                inner.debugger = None;
                            }
                            // The next look lets go of the link and runs the machine on.
                        }
                        LinkEnd::Kill => {
                            // As StopVm: the last frame, the state change, the debugger
                            // detached (`X09` has been sent).
                            link.waiting = false;
                            self.last_look(&mut inner, &machine);
                            self.set_state(&mut inner, VmState::Stopped);
                            self.end_debugger(&mut inner, None);
                            return;
                        }
                        LinkEnd::Exited(code) => {
                            link.waiting = false;
                            self.last_look(&mut inner, &machine);
                            if !inner.gone {
                                self.set_state(&mut inner, VmState::Exited(code));
                            }
                            self.end_debugger(&mut inner, None);
                            return;
                        }
                    }
                    continue;
                }
                Work::Slice(keys, pointer) => (keys, pointer),
            };

            // Viewers' input first, as Grail's window hands it over between slices.
            for key in keys {
                machine.key_event(key);
            }
            for change in pointer {
                machine.pointer_event(change);
            }
            // With breakpoints or watchpoints (its debugger's, or those of one whose
            // connection ended), the machine runs as Grail's stub runs it, stopping at
            // them.
            let debugged = machine.has_debug_points() || link.as_ref().is_some_and(|l| l.waiting);
            let (steps, stop) = if debugged {
                let before = machine.debug_steps();
                let stop = machine.debug_continue(SLICE, &mut |_| {});
                (machine.debug_steps() - before, stop)
            } else {
                (machine.run_for(SLICE, &mut |_| {}), None)
            };
            // `run_for` ends a slice early once the guest has shown a new frame (written
            // FB_PRESENT, made a modeset that took effect, or a page flip took effect at a
            // vertical blank), and once a vertical blank has changed the cursor (moved,
            // shown or hidden it, or given it an image). A present also waits
            // for the host to take it: take it, so that `present_pending` does not say a
            // frame is waiting when none is.
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
            if let Some(stop) = stop {
                // A breakpoint, a watchpoint or the exit: the stop reply of a debugger
                // waiting for one. With none (or none waiting), the VM stops all the
                // same, as QEMU's does in state debug.
                if let Some(link) = link.as_mut().filter(|l| l.waiting) {
                    let mut out = Vec::new();
                    link.session
                        .stopped(&mut machine, StopReason::Machine(stop), &mut out);
                    link.waiting = false;
                    self.debug_output(&mut inner, out);
                }
                if machine.exit_code().is_none() && inner.state == VmState::Running {
                    self.set_state(&mut inner, VmState::Debug);
                }
            }
            if let Some(code) = machine.exit_code() {
                // The last frame before the state change, so that viewers have it by
                // the time they hear the program has ended.
                self.last_look(&mut inner, &machine);
                if !inner.gone && matches!(inner.state, VmState::Running | VmState::Paused | VmState::Debug) {
                    // A stop asked for during the slice wins, as it would have had it
                    // come a step earlier.
                    let end = if inner.stopping {
                        VmState::Stopped
                    } else {
                        VmState::Exited(code)
                    };
                    self.set_state(&mut inner, end);
                }
                self.end_debugger(&mut inner, link.take());
                return;
            }
            if inner.state != VmState::Running {
                continue;
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
                for due in [self.screen_due(&inner), Vm::notice_due(&inner)]
                    .into_iter()
                    .flatten()
                {
                    nap = nap.min(due.saturating_duration_since(Instant::now()));
                }
                let debug_input = inner.debugger.as_ref().is_some_and(|d| !d.input.is_empty())
                    || inner.dropped.as_ref().is_some_and(|d| !d.input.is_empty());
                if !inner.kick && !inner.gone && !inner.stopping && !debug_input {
                    drop(self.wake.wait_timeout(inner, nap));
                }
            }
        }
    }

    /// Makes the thread's link with the debugger follow `inner.debugger`, as QEMU's stub
    /// follows its GDB connection. A new debugger gets a session with the machine's
    /// stub, which stops the machine for it: a running VM is paused
    /// (`vm_stop(RUN_STATE_PAUSED)`), a stopped one stays as it is. A debugger that
    /// detached (`D`, `DebugDetach`) leaves the machine without its breakpoints and
    /// watchpoints, and a stopped VM running again (`gdb_continue`). One whose
    /// connection ended leaves it as it is, breakpoints and all: QEMU does nothing then.
    fn link_debugger(&self, inner: &mut Inner, link: &mut Option<DebugLink>, machine: &mut Machine) {
        let client = inner.debugger.as_ref().map(|d| d.outbox.id);
        if link.as_ref().map(|l| l.client) == client {
            return;
        }
        if let Some(old) = link.take() {
            if inner.dropped.take().map(|d| d.client) != Some(old.client) {
                machine.clear_debug_points();
                if matches!(inner.state, VmState::Paused | VmState::Debug) {
                    self.set_state(inner, VmState::Running);
                }
            }
        }
        if let Some(client) = client {
            *link = Some(DebugLink {
                client,
                session: gdb::Session::new(machine),
                decoder: Decoder::default(),
                waiting: false,
            });
            if inner.state == VmState::Running {
                self.set_state(inner, VmState::Paused);
            }
        }
        inner.linked = client;
        self.wake.notify_all();
    }

    /// The stub's bytes for the debugger.
    fn debug_output(&self, inner: &mut Inner, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        let Some(debugger) = &inner.debugger else {
            return;
        };
        let event = event_frame(&Message::DebugOutput { vm: self.id, data });
        if !debugger.outbox.event(&event) {
            // Gone, as if its connection had ended.
            inner.dropped = inner.debugger.take().map(Dropped::from);
        }
    }

    /// The bytes a debugger gone without `D` sent that the stub has not had, while the
    /// thread still has its session (`link`).
    fn dropped_input(inner: &mut Inner, link: &Option<DebugLink>) -> Option<Vec<u8>> {
        let dropped = inner.dropped.as_mut()?;
        if link.as_ref().map(|l| l.client) != Some(dropped.client) || dropped.input.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut dropped.input))
    }

    /// The machine has ended (stopped, exited, destroyed): a debugger waiting for a stop
    /// reply gets `X09` (an exit has sent its `W` already), and the attachment ends
    /// (`Detached`), since there is nothing left to debug.
    fn end_debugger(&self, inner: &mut Inner, link: Option<DebugLink>) {
        if let Some(mut link) = link.filter(|l| l.waiting) {
            let mut out = Vec::new();
            link.session.killed(&mut out);
            self.debug_output(inner, out);
        }
        if let Some(debugger) = inner.debugger.take() {
            debugger.outbox.event(&event_frame(&Message::Detached {
                vm: self.id,
                kind: AttachKind::Debug,
            }));
        }
        inner.linked = None;
        self.wake.notify_all();
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

    /// Captures the screen and the cursor if the machine has run since the last
    /// capture, and sends every viewer that keeps up the rectangles that changed, then
    /// what changed of the cursor (protocol doc, section 6.4, **Cursor**). A viewer over
    /// [`screen_backlog`] of the screen when something changed gets nothing and is marked
    /// behind: a cursor's image is up to 256 KiB, so an animated cursor would otherwise run
    /// a stalled viewer up to [`crate::MAX_QUEUED`] as frames would. A viewer that is behind
    /// gets a full frame, then the whole cursor (as one attaching does), once it is under
    /// it again (or at once when `last`).
    ///
    /// The screen is what the framebuffer scans out, at its mode's size. When the guest
    /// has changed modes, the screen starts again at the new size: every viewer, behind
    /// or not, is sent `ScreenResized`, and the new screen goes out whole, as a full
    /// frame would.
    fn update_viewers(&self, inner: &mut Inner, machine: &Machine, last: bool) {
        inner.screen_looked = Some(Instant::now());
        let id = self.id;
        let mut resized = None;
        let mut cursor_changes = Vec::new();
        let rects = if std::mem::take(&mut inner.screen_owed) {
            // A display that is off shows black, as Grail's window does, and no cursor.
            let frame = machine.cpu.bus.frame();
            let shown = frame.cursor.map(|c| CursorView {
                width: c.width,
                height: c.height,
                hot_x: c.hot_x,
                hot_y: c.hot_y,
                pixels: c.pixels,
                x: c.x,
                y: c.y,
            });
            cursor_changes = inner.cursor.update(id, shown).iter().map(event_frame).collect();
            if (frame.width, frame.height) == (inner.screen.width(), inner.screen.height()) {
                inner.screen.update(frame.pixels)
            } else {
                inner.screen = Screen::new(frame.width, frame.height);
                inner.screen.update(frame.pixels);
                resized = Some(event_frame(&Message::ScreenResized {
                    vm: id,
                    width: frame.width,
                    height: frame.height,
                    format: PIXEL_FORMAT_XRGB8888,
                }));
                vec![inner.screen.bounds()]
            }
        } else {
            Vec::new()
        };
        let Inner {
            viewers,
            screen,
            cursor,
            keys,
            pointer,
            kick,
            ..
        } = inner;
        let frames: Vec<Frame> = rects.iter().map(|&r| event_frame(&screen.frame(id, r))).collect();
        let backlog = screen_backlog(screen.width(), screen.height());
        let mut full = None;
        let mut whole_cursor = None;
        // Whether viewer `v` stays: false once its client is gone (or cut off).
        let mut send = |v: &mut Viewer| {
            // Every viewer learns the new size, so that what it is sent next fits.
            if let Some(resized) = &resized {
                if !v.outbox.event(resized) {
                    return false;
                }
            }
            let keeping_up = v.outbox.queued() <= backlog;
            if v.behind {
                if !(keeping_up || last) {
                    return true;
                }
                v.behind = false;
                // Level with the others: the screen and the cursor as they have them.
                let full = full.get_or_insert_with(|| event_frame(&screen.frame(id, screen.bounds())));
                let whole_cursor = whole_cursor
                    .get_or_insert_with(|| cursor.messages(id).iter().map(event_frame).collect::<Vec<_>>());
                return v.outbox.event(full) && whole_cursor.iter().all(|f| v.outbox.event(f));
            }
            if frames.is_empty() && cursor_changes.is_empty() {
                return true;
            }
            if !(keeping_up || last) {
                v.behind = true;
                return true;
            }
            // `event` is false once the client is gone (or cut off): it is dropped.
            frames.iter().chain(&cursor_changes).all(|f| v.outbox.event(f))
        };
        for mut v in std::mem::take(viewers) {
            if send(&mut v) {
                viewers.push(v);
            } else {
                // Gone, as if it had detached: what it held is let go.
                v.let_go(keys, pointer);
                *kick = true;
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
/// step), nor the framebuffer's vertical blank (its interrupt is off).
fn parked(machine: &Machine) -> bool {
    let cpu = &machine.cpu;
    let timer_armed = |hart: &grail::cpu::Hart| {
        hart.csr.mie & (1 << IRQ_M_TIMER) != 0 && cpu.bus.clint.mtimecmp[hart.id as usize] != u64::MAX
    };
    // A vertical blank whose interrupt is enabled wakes a hart as a timer would.
    cpu.all_idle() && !cpu.harts().iter().any(timer_armed) && cpu.bus.device_deadline().is_none()
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
            if !inner.gone && matches!(inner.state, VmState::Running | VmState::Paused | VmState::Debug) {
                vm.set_state(&mut inner, VmState::Stopped);
            }
            if let Some(debugger) = inner.debugger.take() {
                debugger.outbox.event(&event_frame(&Message::Detached {
                    vm: vm.id,
                    kind: AttachKind::Debug,
                }));
            }
        }
        inner.linked = None;
        inner.input = None;
        vm.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::switch::Switch;
    use grail::net::MacAddr;

    /// The backlog is a little over one full frame of the screen, at least 2 MiB, and
    /// leaves room under the queue's limit in every mode.
    #[test]
    fn the_backlog_follows_the_screen() {
        use grail::framebuffer::{MAX_FRAME_SIZE, MODES};
        assert_eq!(
            screen_backlog(640, 480),
            SCREEN_BACKLOG_MIN,
            "as for 640x480 before"
        );
        assert_eq!(screen_backlog(1, 1), SCREEN_BACKLOG_MIN);
        assert_eq!(
            screen_backlog(1920, 1080),
            1920 * 1080 * 4 + SCREEN_BACKLOG_MARGIN
        );
        assert_eq!(screen_backlog(1280, 1024), 5 * 1024 * 1024 + 512 * 1024);
        for mode in MODES {
            let backlog = screen_backlog(mode.width, mode.height) as u64;
            assert!(backlog > mode.frame_size(), "{mode:?}");
            assert!(
                backlog < mode.frame_size() + 2 * 1024 * 1024,
                "under two frames of 640x480 more than one of {mode:?}"
            );
            assert!(crate::MAX_QUEUED as u64 > backlog + 2 * MAX_FRAME_SIZE);
        }
    }

    #[test]
    fn a_viewer_lets_go_of_what_it_held() {
        use keyboard::{code, usage, KeyEvent};
        let mut v = Viewer::new(&Arc::new(Outbox::new(1, || {})));
        let shift = KeyEvent::key_press(usage::LEFT_SHIFT);
        let a = KeyEvent::press(b'A').with_usage(usage::A).with_shift(true);
        let enter = KeyEvent::press(code::ENTER).with_usage(usage::ENTER);
        let kp_enter = KeyEvent::press(code::ENTER).with_usage(usage::KP_ENTER);
        let f1 = KeyEvent::press(code::F1); // a viewer that knows no usages
        for key in [
            shift,
            a,
            a.with_repeat(true),
            enter,
            kp_enter,
            f1,
            KeyEvent::release(code::ENTER).with_usage(usage::ENTER),
            KeyEvent::key_release(usage::ESCAPE),  // never went down
            KeyEvent::press(0).with_usage(0xffff), // no key of either table
        ] {
            v.key(key);
        }
        v.buttons = grail::pointer::BUTTON_LEFT;
        let (mut keys, mut pointer) = (VecDeque::new(), VecDeque::new());
        v.let_go(&mut keys, &mut pointer);
        // Each held key once, in the order it went down, with no Shift or Ctrl; the
        // keypad's Enter, which shares Enter's code, is still held.
        assert_eq!(
            Vec::from(keys),
            [
                KeyEvent::key_release(usage::LEFT_SHIFT),
                KeyEvent::release(b'A').with_usage(usage::A),
                KeyEvent::release(code::ENTER).with_usage(usage::KP_ENTER),
                KeyEvent::release(code::F1),
            ]
        );
        assert_eq!(
            Vec::from(pointer),
            [PointerInput {
                motion: Motion::None,
                buttons: 0,
                wheel: 0
            }]
        );
        // Nothing held, nothing to let go.
        let (mut keys, mut pointer) = (VecDeque::new(), VecDeque::new());
        Viewer::new(&Arc::new(Outbox::new(2, || {}))).let_go(&mut keys, &mut pointer);
        assert!(keys.is_empty() && pointer.is_empty());
    }

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

    /// A full-speed VM naps between slices only when every hart is parked: one hart in
    /// `wfi` while another runs must not slow the machine down, nor may another hart's
    /// armed timer be missed.
    #[test]
    fn parked_looks_at_every_hart() {
        let mut m = Machine::new(MachineConfig {
            harts: 2,
            ..MachineConfig::with_ram(1 << 20)
        })
        .unwrap();
        m.cpu.hart_mut(0).waiting = true;
        m.cpu.hart_mut(1).waiting = false;
        assert!(!parked(&m), "hart 1 runs");
        m.cpu.hart_mut(1).waiting = true;
        assert!(parked(&m), "both wait, no timer");
        // A timer set but masked does not wake hart 1; armed (MTIE set too), it does.
        m.cpu.bus.clint.mtimecmp[1] = 1_000_000;
        assert!(parked(&m));
        m.cpu.hart_mut(1).csr.mie = 1 << IRQ_M_TIMER;
        assert!(!parked(&m), "hart 1's timer is armed");
        m.cpu.bus.clint.mtimecmp[1] = u64::MAX;
        assert!(parked(&m), "MTIE without a deadline");
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
