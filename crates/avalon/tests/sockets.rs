//! The Camelot kernel's sockets between two machines on an Avalon switch: `nc` in one
//! listens, `nc` in the other connects, and each prints what the other sent, over TCP
//! and over UDP (issue #85's second criterion, second half). The machines are Grail's,
//! run in this thread in turns of a few thousand steps, their cards plugged into an
//! in-process [`Switch`] as an Avalon VM's are; each kernel gets a static address
//! (10.0.9.1 and 10.0.9.2), patched into its configuration before it boots. It runs
//! twice ([`common::on_both_widths`]): with the RV32 kernel, `kernel/kernel.elf`, and
//! with the RV64 one, `kernel/kernel-rv64.elf`, whose variables are patched at their
//! load addresses (it is linked in the upper half and loaded at 0x8000_0000).

mod common;

use avalon::switch::{Card, Switch};
use avalon_proto::PortOwner;
use common::{grail_xlen, on_both_widths};
use grail::net::{MacAddr, NetBackend};
use grail::{Machine, MachineConfig};
use std::path::Path;
use std::sync::mpsc::{channel, Sender};

/// Steps a machine runs per turn, and at most in all.
const SLICE: u64 = 2_000;
const MAX_STEPS: u64 = 400_000_000;
const PROMPT: &str = "$ ";

/// A Camelot machine with one card on the switch, its console captured and fed
/// through `input`.
struct Vm {
    m: Machine,
    input: Sender<u8>,
}

impl Vm {
    /// Card 0 of VM `id`, with address 02:ca:00:00:00:0`id` and IPv4 address
    /// 10.0.9.`id`, plugged into `switch`.
    fn new(id: u8, switch: &Switch) -> Vm {
        let card = Card::new(0, MacAddr([0x02, 0xca, 0, 0, 0, id]));
        card.plug(
            PortOwner::Card {
                vm: u32::from(id),
                card: 0,
            },
            switch,
        )
        .unwrap();
        let backend: Box<dyn NetBackend> = Box::new(card.backend());
        let config = MachineConfig {
            ram_size: 16 << 20,
            xlen: grail_xlen(),
            // The RV64 kernel runs in S-mode, on Camelot's firmware.
            bios: match grail_xlen() {
                grail::Xlen::Rv32 => grail::Bios::None,
                grail::Xlen::Rv64 => grail::Bios::Default,
            },
            ..MachineConfig::default()
        };
        let mut m = Machine::with_devices(config, Vec::new(), vec![(card.mac, backend)]).unwrap();
        m.cpu.bus.uart.capture = true;
        // Time is steps, idle or not: a machine waiting in wfi does not skip ahead to its
        // next tick, so both machines' clocks keep together. (Otherwise nc -u, idle in a
        // one-second timeout, could see its second pass while the other machine is still
        // starting the program that answers.)
        m.cpu.wfi_skips_to_timer = false;
        let kernel = match grail_xlen() {
            grail::Xlen::Rv32 => "kernel.elf",
            grail::Xlen::Rv64 => "kernel-rv64.elf",
        };
        let file = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../kernel")
                .join(kernel),
        )
        .unwrap();
        m.load_elf(&file).unwrap();
        // The kernel's static configuration (kernel/net/net.c): no DHCP, this
        // address, a /24, no gateway. Each variable is written where it is loaded,
        // which on RV64 is not where it runs.
        let elf = grail::elf::parse(&file).unwrap();
        for (name, value) in [
            ("net_dhcp", 0),
            ("net_static_addr", u32::from_be_bytes([10, 0, 9, id])),
            ("net_static_mask", 0xffff_ff00),
            ("net_static_gateway", 0),
        ] {
            let va = elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"));
            let segment = elf
                .segments
                .iter()
                .find(|s| (s.vaddr..s.vaddr + s.mem_size).contains(&va))
                .unwrap_or_else(|| panic!("{name} is in no segment"));
            m.cpu
                .bus
                .load(va - segment.vaddr + segment.paddr, &u32::to_le_bytes(value))
                .unwrap();
        }
        let (input, rx) = channel();
        m.set_uart_input(rx);
        Vm { m, input }
    }

    fn out(&self) -> String {
        String::from_utf8_lossy(&self.m.cpu.bus.uart.captured).into_owned()
    }

    fn prompts(&self) -> usize {
        self.out().matches(&format!("\n{PROMPT}")).count()
    }

    fn type_line(&self, line: &str) {
        for b in line.bytes().chain(std::iter::once(b'\n')) {
            self.input.send(b).unwrap();
        }
    }
}

/// Runs the machines in turns until `done` holds.
fn run_until(vms: &mut [&mut Vm], done: impl Fn(&[&mut Vm]) -> bool) {
    let mut steps = 0;
    while !done(vms) {
        for vm in vms.iter_mut() {
            vm.m.run_for(SLICE, &mut |_| {});
            assert_eq!(vm.m.exit_code(), None, "a machine stopped:\n{}", vm.out());
        }
        steps += SLICE;
        assert!(
            steps < MAX_STEPS,
            "no end in sight:\n{}\n----\n{}",
            vms[0].out(),
            vms[1].out()
        );
    }
}

/// Types `server` into the first machine and, once it printed `ready`, `client` into
/// the second; runs both until each shows its next prompt. Returns what each printed
/// after its command line, up to the prompt.
fn exchange(vms: &mut [&mut Vm], server: &str, ready: &str, client: &str) -> (String, String) {
    let before = [vms[0].out().len(), vms[1].out().len()];
    let prompts = [vms[0].prompts(), vms[1].prompts()];
    vms[0].type_line(server);
    run_until(vms, |vms| vms[0].out()[before[0]..].contains(ready));
    vms[1].type_line(client);
    run_until(vms, |vms| {
        vms[0].prompts() > prompts[0] && vms[1].prompts() > prompts[1]
    });
    let text = |vm: &Vm, before: usize, line: &str| {
        let out = vm.out();
        out[before..]
            .strip_prefix(&format!("{line}\n"))
            .and_then(|t| t.strip_suffix(PROMPT))
            .unwrap_or_else(|| panic!("{line:?}:\n{out}"))
            .to_string()
    };
    (text(vms[0], before[0], server), text(vms[1], before[1], client))
}

#[test]
fn nc_talks_between_two_machines_on_a_switch() {
    on_both_widths(|| {
        let switch = Switch::new("lan");
        let mut a = Vm::new(1, &switch);
        let mut b = Vm::new(2, &switch);
        let vms = &mut [&mut a, &mut b];
        run_until(vms, |vms| vms.iter().all(|vm| vm.prompts() >= 1));
        for (vm, addr) in vms.iter().zip(["10.0.9.1", "10.0.9.2"]) {
            assert!(
                vm.out().contains(&format!("net: {addr}/24 (static)\n")),
                "{}",
                vm.out()
            );
        }

        // TCP: each side sends its input and ends its side; each prints the other's.
        let (server, client) = exchange(
            vms,
            "echo reply | nc -lv 5000",
            "listening on port 5000\n",
            "echo hello | nc 10.0.9.1 5000",
        );
        let lines: Vec<&str> = server.lines().collect();
        assert_eq!(lines[0], "listening on port 5000", "{server}");
        assert!(lines[1].starts_with("connection from 10.0.9.2:"), "{server}");
        assert_eq!(lines[2..], ["hello"], "{server}");
        assert_eq!(client, "reply\n");

        // UDP: the first datagram says who the client is; each side stops once its input
        // is over and nothing more came for a second.
        let (server, client) = exchange(
            vms,
            "echo pong | nc -luv 5001",
            "listening on port 5001\n",
            "echo ping | nc -u 10.0.9.1 5001",
        );
        let lines: Vec<&str> = server.lines().collect();
        assert_eq!(lines[0], "listening on port 5001", "{server}");
        assert!(lines[1].starts_with("datagram from 10.0.9.2:"), "{server}");
        assert_eq!(lines[2..], ["ping"], "{server}");
        assert_eq!(client, "pong\n");

        // Nobody listens: the connection is refused, and nc says so.
        let (_, client) = exchange(vms, "echo", "", "echo x | nc 10.0.9.1 5002");
        assert_eq!(client, "nc: 10.0.9.1: connection refused\nnc: exit 1\n");

        for vm in vms.iter_mut() {
            vm.type_line("exit 0");
            assert_eq!(
                vm.m.run(Some(MAX_STEPS), |_| {}),
                grail::RunResult::Exited(0),
                "{}",
                vm.out()
            );
        }
    });
}
