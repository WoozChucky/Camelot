//! The device tree of a machine (spec section 7, "Firmware"): what Grail hands the
//! firmware in a1, as QEMU hands OpenSBI the tree of its `virt` machine.
//!
//! It is made from the machine's configuration and its device table the way QEMU's
//! `hw/riscv/virt.c` makes `virt`'s, node for node where Grail's devices are `virt`'s,
//! so that OpenSBI's generic platform and Linux find them with the drivers they have:
//!
//! - `/cpus`: a `cpu@N` per hart with its ISA (`riscv,isa` and the
//!   `riscv,isa-base`/`riscv,isa-extensions` pair of the current binding), its paging
//!   (`mmu-type`: `riscv,sv48`, or `riscv,sv39` with `--paging sv39`) and its local
//!   interrupt controller; `timebase-frequency`, sysinfo's `SYS_TIMEBASE_HZ`; the
//!   `cpu-map`;
//! - `/memory@80000000` and `/reserved-memory`, whose `firmware@80000000` (`no-map`)
//!   is the firmware's 2 MiB, as OpenSBI's own fixup reserves its region;
//! - `/soc`: the CLINT (`sifive,clint0`), the PLIC (`sifive,plic-1.0.0`, its 31
//!   sources, two contexts a hart: M then S), the UART (`ns16550a`: Grail's is
//!   register-compatible), the test device (`sifive,test0` with `syscon`, for
//!   `/poweroff`'s `syscon-poweroff`, which writes 0x5555), and Grail's own devices
//!   with `camelot,*` compatibles: the disks, the network cards (with
//!   `local-mac-address`), the display, the keyboard, the pointer and sysinfo; and
//!   the real-time clock, `google,goldfish-rtc` as on `virt`;
//! - `/aliases`: `serial0`, the UART, and `/chosen`: `stdout-path`, the UART.
//!
//! The PLIC sources are Grail's (spec section 7.1), not `virt`'s: a driver reads them
//! from `interrupts` here, or from sysinfo.

use crate::bus::{Bus, RAM_BASE};
use crate::config::{MachineConfig, Paging};
use crate::devices::PLIC_SOURCES;
use crate::fdt::Writer;
use crate::firmware::{FW_BASE, FW_SIZE};
use crate::sysinfo::{DeviceEntry, DeviceType};

/// The UART's input clock as the tree gives it: QEMU's 3.6864 MHz. Grail's UART has no
/// baud rate, so a driver computing a divisor from it gets one that changes nothing.
pub const UART_CLOCK_HZ: u32 = 3_686_400;

/// The ISA every hart implements, as `riscv,isa` spells it (the Xcal extensions are
/// Grail's own and not in it; spec section 9).
pub const RISCV_ISA: &str = "rv64ima_zicntr_zicsr_zifencei";
/// The same, as the `riscv,isa-extensions` list.
pub const RISCV_ISA_EXTENSIONS: &[&str] = &["i", "m", "a", "zicntr", "zicsr", "zifencei"];

/// The tree of a machine built from `config` with the devices of `bus`, its RAM, its
/// harts, its firmware's region: a flattened device tree ([`crate::fdt`]).
pub fn generate(config: &MachineConfig, bus: &Bus) -> Vec<u8> {
    let harts = config.harts as u32;
    // Phandles: each hart's interrupt controller, 1 to N, then the PLIC and the test
    // device, as QEMU numbers them from 1.
    let intc = |hart: u32| 1 + hart;
    let plic_phandle = harts + 1;
    let test_phandle = harts + 2;
    let devices = bus.sysinfo.devices();
    let find = |kind| devices.iter().copied().find(|d: &DeviceEntry| d.kind == kind);

    let mut w = Writer::new();
    w.boot_cpuid(0);
    w.begin_node("");
    w.prop_u32("#address-cells", 2);
    w.prop_u32("#size-cells", 2);
    w.prop_str("compatible", "camelot,grail");
    w.prop_str("model", "Camelot Grail");

    if let Some(uart) = find(DeviceType::Uart) {
        // As QEMU's create_fdt_uarts(): the UART is the console (stdout-path) and
        // serial0 (the alias by which Linux's serial core numbers it port 0,
        // ttyS0). virt has no other alias, and neither has Grail.
        let uart = format!("/soc/serial@{:x}", uart.base);
        w.begin_node("aliases");
        w.prop_str("serial0", &uart);
        w.end_node();
        w.begin_node("chosen");
        w.prop_str("stdout-path", &uart);
        w.end_node();
    }

    if find(DeviceType::TestFinisher).is_some() {
        // QEMU's /poweroff: a write of 0x5555 to the test device, which stops the
        // machine with exit code 0 (Grail has no reset, so no /reboot).
        w.begin_node("poweroff");
        w.prop_str("compatible", "syscon-poweroff");
        w.prop_u32("regmap", test_phandle);
        w.prop_u32("offset", 0);
        w.prop_u32("value", 0x5555);
        w.end_node();
    }

    w.begin_node(&format!("memory@{RAM_BASE:x}"));
    w.prop_str("device_type", "memory");
    w.prop_u64s("reg", &[RAM_BASE, config.ram_size as u64]);
    w.end_node();

    w.begin_node("reserved-memory");
    w.prop_u32("#address-cells", 2);
    w.prop_u32("#size-cells", 2);
    w.prop_empty("ranges");
    w.begin_node(&format!("firmware@{FW_BASE:x}"));
    w.prop_u64s("reg", &[FW_BASE, FW_SIZE]);
    w.prop_empty("no-map");
    w.end_node();
    w.end_node();

    w.begin_node("cpus");
    w.prop_u32("#address-cells", 1);
    w.prop_u32("#size-cells", 0);
    w.prop_u32("timebase-frequency", config.cpu_hz());
    for hart in 0..harts {
        w.begin_node(&format!("cpu@{hart}"));
        w.prop_str("device_type", "cpu");
        w.prop_u32("reg", hart);
        w.prop_str("status", "okay");
        w.prop_str("compatible", "riscv");
        w.prop_str("riscv,isa", RISCV_ISA);
        w.prop_str("riscv,isa-base", "rv64i");
        w.prop_strs("riscv,isa-extensions", RISCV_ISA_EXTENSIONS);
        let mmu = match config.paging {
            Paging::Sv48 => "riscv,sv48",
            Paging::Sv39 => "riscv,sv39",
        };
        w.prop_str("mmu-type", mmu);
        w.prop_u32("phandle", 0x100 + hart);
        w.begin_node("interrupt-controller");
        w.prop_u32("#interrupt-cells", 1);
        w.prop_empty("interrupt-controller");
        w.prop_str("compatible", "riscv,cpu-intc");
        w.prop_u32("phandle", intc(hart));
        w.end_node();
        w.end_node();
    }
    w.begin_node("cpu-map");
    w.begin_node("cluster0");
    for hart in 0..harts {
        w.begin_node(&format!("core{hart}"));
        w.prop_u32("cpu", 0x100 + hart);
        w.end_node();
    }
    w.end_node();
    w.end_node();
    w.end_node();

    w.begin_node("soc");
    w.prop_u32("#address-cells", 2);
    w.prop_u32("#size-cells", 2);
    w.prop_str("compatible", "simple-bus");
    w.prop_empty("ranges");
    let reg = |w: &mut Writer, d: &DeviceEntry| w.prop_u64s("reg", &[d.base, d.size]);
    let irq = |w: &mut Writer, d: &DeviceEntry| {
        w.prop_u32("interrupts", d.irq);
        w.prop_u32("interrupt-parent", plic_phandle);
    };
    let mut net = 0;
    for d in devices {
        match d.kind {
            DeviceType::TestFinisher => {
                w.begin_node(&format!("test@{:x}", d.base));
                w.prop_strs("compatible", &["sifive,test0", "syscon"]);
                reg(&mut w, d);
                w.prop_u32("phandle", test_phandle);
                w.end_node();
            }
            DeviceType::Clint => {
                // Each hart's software (3) and timer (7) interrupts, in hart order.
                let cells: Vec<u32> = (0..harts).flat_map(|h| [intc(h), 3, intc(h), 7]).collect();
                w.begin_node(&format!("clint@{:x}", d.base));
                w.prop_strs("compatible", &["sifive,clint0", "riscv,clint0"]);
                reg(&mut w, d);
                w.prop_cells("interrupts-extended", &cells);
                w.end_node();
            }
            DeviceType::Plic => {
                // Context 2h is hart h's M-mode one (its external interrupt, 11),
                // 2h + 1 its S-mode one (9).
                let cells: Vec<u32> = (0..harts).flat_map(|h| [intc(h), 11, intc(h), 9]).collect();
                w.begin_node(&format!("plic@{:x}", d.base));
                w.prop_u32("#address-cells", 0);
                w.prop_u32("#interrupt-cells", 1);
                w.prop_strs("compatible", &["sifive,plic-1.0.0", "riscv,plic0"]);
                reg(&mut w, d);
                w.prop_empty("interrupt-controller");
                w.prop_u32("riscv,ndev", PLIC_SOURCES - 1);
                w.prop_cells("interrupts-extended", &cells);
                w.prop_u32("phandle", plic_phandle);
                w.end_node();
            }
            DeviceType::Uart => {
                w.begin_node(&format!("serial@{:x}", d.base));
                w.prop_str("compatible", "ns16550a");
                reg(&mut w, d);
                w.prop_u32("clock-frequency", UART_CLOCK_HZ);
                irq(&mut w, d);
                w.end_node();
            }
            DeviceType::Block => {
                w.begin_node(&format!("block@{:x}", d.base));
                w.prop_str("compatible", "camelot,block");
                reg(&mut w, d);
                irq(&mut w, d);
                w.end_node();
            }
            DeviceType::Net => {
                w.begin_node(&format!("ethernet@{:x}", d.base));
                w.prop_str("compatible", "camelot,net");
                reg(&mut w, d);
                irq(&mut w, d);
                if let Some(card) = bus.nets.get(net) {
                    w.prop("local-mac-address", &card.mac().0);
                }
                w.end_node();
                net += 1;
            }
            DeviceType::FbCtrl => {
                // The display controller, with its VRAM as its second range.
                w.begin_node(&format!("display@{:x}", d.base));
                w.prop_str("compatible", "camelot,display");
                match find(DeviceType::FbVram) {
                    Some(vram) => {
                        w.prop_u64s("reg", &[d.base, d.size, vram.base, vram.size]);
                        w.prop_strs("reg-names", &["control", "vram"]);
                    }
                    None => reg(&mut w, d),
                }
                irq(&mut w, d);
                w.end_node();
            }
            DeviceType::Keyboard | DeviceType::Pointer | DeviceType::Sysinfo => {
                let name = match d.kind {
                    DeviceType::Keyboard => "keyboard",
                    DeviceType::Pointer => "pointer",
                    _ => "sysinfo",
                };
                w.begin_node(&format!("{name}@{:x}", d.base));
                w.prop_str("compatible", &format!("camelot,{name}"));
                reg(&mut w, d);
                if d.irq != 0 {
                    irq(&mut w, d);
                }
                w.end_node();
            }
            DeviceType::Rtc => {
                // QEMU `virt`'s clock, which Linux's rtc-goldfish drives.
                w.begin_node(&format!("rtc@{:x}", d.base));
                w.prop_str("compatible", "google,goldfish-rtc");
                reg(&mut w, d);
                irq(&mut w, d);
                w.end_node();
            }
            // The display controller's node has it.
            DeviceType::FbVram => {}
        }
    }
    w.end_node();

    w.end_node();
    w.finish()
}
