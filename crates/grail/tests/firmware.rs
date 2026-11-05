//! Firmware (spec section 7, "Firmware"; `--bios`, #174): the boot ROM's hand-off,
//! which gives every hart what QEMU's reset vector gives OpenSBI (a0 = the hart id,
//! a1 = the device tree, a2 = a `fw_dynamic_info`); the device tree Grail makes for
//! the machine, QEMU `virt`'s nodes for Grail's devices; sysinfo's reserved range;
//! Camelot's built-in firmware starting a program, or a disk's boot block, in S-mode at
//! 0x8020_0000; and what a machine with firmware refuses. The Camelot kernel on the
//! firmware is xtask's tests (kernel_rv64.rs, sbi.rs, and opensbi.rs on OpenSBI's).

use grail::boot::{BOOT_BLOCK_SIZE, BOOT_ENTRY_ADDR, FDT_ADDR_ADDR, FW_HANDOFF_PC};
use grail::bus::{BLK_BASE, SYSINFO_BASE};
use grail::cpu::Mode;
use grail::fdt::{self, Node};
use grail::firmware::{FW_BASE, FW_JUMP_ADDR, FW_SIZE};
use grail::{Bios, DiskConfig, Machine, MachineConfig, NetConfig, Paging, RunResult, Xlen};

const A0: u32 = 10;
const A1: u32 = 11;
const A6: u32 = 16;
const A7: u32 = 17;
const ZERO: u32 = 0;

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) & 0xfff) << 20 | rs1 << 15 | rd << 7 | 0x13
}
fn lui(rd: u32, imm20: u32) -> u32 {
    imm20 << 12 | rd << 7 | 0x37
}
const ECALL: u32 = 0x0000_0073;
const J_SELF: u32 = 0x0000_006f;

/// An SBI SRST shutdown with Camelot's reason 0xE000_0000 | `code`, the exit code
/// (the SBI zero-extends the 32-bit reason that `lui` sign-extends), then `j .`.
fn shutdown(code: i32) -> Vec<u32> {
    vec![
        lui(A7, 0x53525),
        addi(A7, A7, 0x354), // a7 = SRST, 0x53525354
        addi(A6, ZERO, 0),   // system_reset
        addi(A0, ZERO, 0),   // a shutdown
        lui(A1, 0xe0000),
        addi(A1, A1, code), // 0xE000_0000 | code
        ECALL,
        J_SELF,
    ]
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// An ELF64 RISC-V executable of `code` at physical (and virtual) `at`, its entry.
fn elf64(at: u64, code: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; 64 + 56];
    f[..16].copy_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    f[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    f[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    f[20..24].copy_from_slice(&1u32.to_le_bytes());
    f[24..32].copy_from_slice(&at.to_le_bytes()); // e_entry
    f[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    f[52..54].copy_from_slice(&64u16.to_le_bytes());
    f[54..56].copy_from_slice(&56u16.to_le_bytes());
    f[56..58].copy_from_slice(&1u16.to_le_bytes());
    f[58..60].copy_from_slice(&64u16.to_le_bytes());
    let ph = &mut f[64..];
    ph[..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    ph[4..8].copy_from_slice(&7u32.to_le_bytes());
    ph[8..16].copy_from_slice(&120u64.to_le_bytes());
    ph[16..24].copy_from_slice(&at.to_le_bytes());
    ph[24..32].copy_from_slice(&at.to_le_bytes());
    ph[32..40].copy_from_slice(&(code.len() as u64).to_le_bytes());
    ph[40..48].copy_from_slice(&(code.len() as u64).to_le_bytes());
    ph[48..56].copy_from_slice(&4u64.to_le_bytes());
    f.extend_from_slice(code);
    f
}

/// An RV64 machine with `harts` harts, `ram` bytes and firmware `bios`.
fn rv64(harts: usize, ram: usize, bios: Bios) -> MachineConfig {
    MachineConfig {
        harts,
        ram_size: ram,
        xlen: Xlen::Rv64,
        bios,
        ..MachineConfig::default()
    }
}

/// A raw firmware file of `code`, in a file of its own.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str, data: &[u8]) -> TempFile {
        let path = std::env::temp_dir().join(format!("grail-firmware-{}-{name}", std::process::id()));
        std::fs::write(&path, data).unwrap();
        TempFile(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn step_until(m: &mut Machine, done: impl Fn(&Machine) -> bool) {
    for _ in 0..1_000_000 {
        if done(m) {
            return;
        }
        assert!(m.cpu.bus.exit.is_none(), "the machine stopped");
        m.cpu.step();
    }
    panic!("gave up at {:#x}", m.cpu.hart(0).pc);
}

fn read64(m: &Machine, addr: u64) -> u64 {
    let lo = u64::from(m.cpu.bus.peek(addr).unwrap());
    let hi = u64::from(m.cpu.bus.peek(addr + 4).unwrap());
    lo | hi << 32
}

/// The device tree at `at` in `m`'s RAM.
fn tree_at(m: &Machine, at: u64) -> fdt::Tree {
    let size = u32::from_be_bytes(read64(m, at + 4).to_le_bytes()[..4].try_into().unwrap()) as u64;
    let blob: Vec<u8> = (0..size)
        .map(|i| m.cpu.bus.read_mem(at + i, 1).unwrap() as u8)
        .collect();
    fdt::parse(&blob).unwrap()
}

fn strs(node: &Node, prop: &str) -> Vec<String> {
    node.strs(prop)
        .unwrap_or_else(|| panic!("{}: no {prop}", node.name))
}

#[test]
fn the_rom_hands_every_hart_to_the_firmware_as_qemu_does() {
    // A raw firmware of `j .`, and a program at 0x8020_0000: hart 0 reaches the
    // firmware from the stub, hart 1 from the park loop once it has an IPI, each
    // with a0 = its id, a1 = the device tree, a2 = the fw_dynamic_info, in M-mode.
    let fw = TempFile::new("jself.bin", &J_SELF.to_le_bytes());
    let mut m = Machine::new(rv64(2, 8 << 20, Bios::File(fw.0.clone()))).unwrap();
    m.load_elf(&elf64(FW_JUMP_ADDR, &bytes(&[J_SELF]))).unwrap();
    // The ROM: the entry doubleword is the hand-off, the device tree's address at
    // 0x1018 is where QEMU would put it, the last 2 MiB boundary below the end.
    assert_eq!(read64(&m, BOOT_ENTRY_ADDR), FW_HANDOFF_PC);
    let fdt = read64(&m, FDT_ADDR_ADDR);
    assert_eq!(fdt, 0x8060_0000);
    step_until(&mut m, |m| m.cpu.hart(0).pc == FW_BASE);
    let h = m.cpu.hart(0);
    assert_eq!(h.mode, Mode::M);
    let (a0, a1, a2) = (h.x[10], h.x[11], h.x[12]);
    assert_eq!((a0, a1), (0, fdt));
    // QEMU's fw_dynamic_info: the magic, version 2, the program's entry, S-mode,
    // no options, boot hart 0.
    let info: Vec<u64> = (0..6).map(|i| read64(&m, a2 + 8 * i)).collect();
    assert_eq!(info, [0x4942_534f, 2, FW_JUMP_ADDR, 1, 0, 0]);
    // Hart 1, parked, until an IPI (the firmware's coldboot sends it one).
    assert_ne!(m.cpu.hart(1).pc, FW_BASE);
    m.cpu.bus.clint.msip[1] = true;
    step_until(&mut m, |m| m.cpu.hart(1).pc == FW_BASE);
    let h = m.cpu.hart(1);
    assert_eq!((h.mode, h.x[10], h.x[11], h.x[12]), (Mode::M, 1, fdt, a2));
    // The tree is a flattened device tree, and the RAM the firmware has is
    // reserved in it and in sysinfo (version 2's SYS_RESERVED_BASE and _SIZE).
    let tree = tree_at(&m, fdt);
    let fw_node = tree.root.find("reserved-memory/firmware@80000000").unwrap();
    assert_eq!(fw_node.u64s("reg"), Some(vec![FW_BASE, FW_SIZE]));
    assert!(fw_node.prop("no-map").is_some());
    assert_eq!(m.cpu.bus.read(SYSINFO_BASE + 0x30, 8), Ok(FW_BASE));
    assert_eq!(m.cpu.bus.read(SYSINFO_BASE + 0x38, 8), Ok(FW_SIZE));
}

#[test]
fn without_firmware_the_rom_and_sysinfo_are_as_before() {
    let mut m = Machine::new(rv64(2, 8 << 20, Bios::None)).unwrap();
    m.load_elf(&elf64(FW_BASE, &bytes(&[J_SELF]))).unwrap();
    assert_eq!(read64(&m, BOOT_ENTRY_ADDR), FW_BASE);
    assert_eq!(read64(&m, FDT_ADDR_ADDR), 0, "no device tree");
    assert_eq!(m.cpu.bus.read(SYSINFO_BASE + 0x30, 8), Ok(0));
    assert_eq!(m.cpu.bus.read(SYSINFO_BASE + 0x38, 8), Ok(0));
    step_until(&mut m, |m| m.cpu.hart(0).pc == FW_BASE);
    assert_eq!(m.cpu.hart(0).mode, Mode::M);
}

#[test]
fn the_device_tree_describes_the_machine_as_qemu_virt_does() {
    let hub = grail::net::Hub::new();
    let config = MachineConfig {
        paging: Paging::Sv39,
        disks: vec![
            DiskConfig::memory(vec![0; 16 * 512]),
            DiskConfig::memory(vec![0; 8 * 512]),
        ],
        nets: vec![NetConfig::hub(&hub).with_mac(grail::net::MacAddr([2, 0xca, 0, 0, 0, 7]))],
        ..rv64(3, 64 << 20, Bios::Default)
    };
    let mut m = Machine::new(config).unwrap();
    m.load_elf(&elf64(FW_JUMP_ADDR, &bytes(&[J_SELF]))).unwrap();
    let at = read64(&m, FDT_ADDR_ADDR);
    assert_eq!(at, 0x83e0_0000, "the last 2 MiB boundary below 64 MiB");
    let tree = tree_at(&m, at);
    assert_eq!(tree.boot_cpuid, 0);
    let root = &tree.root;
    assert_eq!(root.u32("#address-cells"), Some(2));
    assert_eq!(root.u32("#size-cells"), Some(2));
    assert_eq!(
        root.find("chosen").unwrap().str("stdout-path").as_deref(),
        Some("/soc/serial@10000000")
    );
    assert_eq!(
        root.find("memory@80000000").unwrap().u64s("reg"),
        Some(vec![0x8000_0000, 64 << 20])
    );

    // The harts: their ISA, their paging, their interrupt controllers.
    let cpus = root.find("cpus").unwrap();
    assert_eq!(cpus.u32("timebase-frequency"), Some(10_000_000));
    let mut intcs = Vec::new();
    for h in 0..3 {
        let cpu = cpus.child(&format!("cpu@{h}")).unwrap();
        assert_eq!(cpu.u32("reg"), Some(h));
        assert_eq!(cpu.str("device_type").as_deref(), Some("cpu"));
        assert_eq!(cpu.str("status").as_deref(), Some("okay"));
        assert_eq!(
            cpu.str("riscv,isa").as_deref(),
            Some("rv64ima_zicntr_zicsr_zifencei")
        );
        assert_eq!(
            strs(cpu, "riscv,isa-extensions"),
            ["i", "m", "a", "zicntr", "zicsr", "zifencei"]
        );
        assert_eq!(cpu.str("mmu-type").as_deref(), Some("riscv,sv39"));
        let intc = cpu.child("interrupt-controller").unwrap();
        assert_eq!(strs(intc, "compatible"), ["riscv,cpu-intc"]);
        assert!(intc.prop("interrupt-controller").is_some());
        intcs.push(intc.u32("phandle").unwrap());
    }
    // cpu-map's cores name the cpus.
    let cluster = cpus.find("cpu-map/cluster0").unwrap();
    for h in 0..3 {
        let cpu = cluster.child(&format!("core{h}")).unwrap().u32("cpu").unwrap();
        assert_eq!(root.by_phandle(cpu).unwrap().name, format!("cpu@{h}"));
    }

    let soc = root.find("soc").unwrap();
    assert_eq!(strs(soc, "compatible"), ["simple-bus"]);
    // The CLINT: each hart's software (3) and timer (7) interrupts.
    let clint = soc.child("clint@2000000").unwrap();
    assert_eq!(strs(clint, "compatible"), ["sifive,clint0", "riscv,clint0"]);
    assert_eq!(clint.u64s("reg"), Some(vec![0x0200_0000, 0x1_0000]));
    let want: Vec<u32> = intcs.iter().flat_map(|&p| [p, 3, p, 7]).collect();
    assert_eq!(clint.cells("interrupts-extended"), Some(want));
    // The PLIC: 31 sources, an M (11) and an S (9) context a hart.
    let plic = soc.child("plic@c000000").unwrap();
    assert_eq!(strs(plic, "compatible"), ["sifive,plic-1.0.0", "riscv,plic0"]);
    assert_eq!(plic.u32("riscv,ndev"), Some(31));
    assert_eq!(plic.u32("#interrupt-cells"), Some(1));
    let want: Vec<u32> = intcs.iter().flat_map(|&p| [p, 11, p, 9]).collect();
    assert_eq!(plic.cells("interrupts-extended"), Some(want));
    let plic_phandle = plic.u32("phandle").unwrap();
    // The UART, a 16550 on source 1 (Grail's numbers, not virt's).
    let uart = soc.child("serial@10000000").unwrap();
    assert_eq!(strs(uart, "compatible"), ["ns16550a"]);
    assert_eq!(uart.u32("interrupts"), Some(1));
    assert_eq!(uart.u32("interrupt-parent"), Some(plic_phandle));
    assert!(uart.u32("clock-frequency").is_some());
    // The test device and the poweroff that writes 0x5555 to it.
    let test = soc.child("test@100000").unwrap();
    assert_eq!(strs(test, "compatible"), ["sifive,test0", "syscon"]);
    let poweroff = root.find("poweroff").unwrap();
    assert_eq!(strs(poweroff, "compatible"), ["syscon-poweroff"]);
    assert_eq!(poweroff.u32("regmap"), test.u32("phandle"));
    assert_eq!(
        (poweroff.u32("offset"), poweroff.u32("value")),
        (Some(0), Some(0x5555))
    );
    // Grail's own devices: the disks (sources 3 and 4), the card with its MAC
    // address, the display with its VRAM, the keyboard, the pointer, sysinfo.
    for (i, base) in [(0, BLK_BASE), (1, BLK_BASE + 0x1000)] {
        let disk = soc.child(&format!("block@{base:x}")).unwrap();
        assert_eq!(strs(disk, "compatible"), ["camelot,block"]);
        assert_eq!(disk.u64s("reg"), Some(vec![base, 0x1000]));
        assert_eq!(disk.u32("interrupts"), Some(3 + i));
    }
    let net = soc.child("ethernet@10009000").unwrap();
    assert_eq!(strs(net, "compatible"), ["camelot,net"]);
    assert_eq!(net.prop("local-mac-address"), Some(&[2, 0xca, 0, 0, 0, 7][..]));
    assert_eq!(net.u32("interrupts"), Some(12));
    let display = soc.child("display@30000000").unwrap();
    assert_eq!(strs(display, "reg-names"), ["control", "vram"]);
    assert_eq!(display.u64s("reg").unwrap()[2], 0x3100_0000);
    assert_eq!(display.u32("interrupts"), Some(16));
    for (name, irq) in [
        ("keyboard@30001000", Some(2)),
        ("sysinfo@30002000", None),
        ("pointer@30003000", Some(11)),
    ] {
        let node = soc.child(name).unwrap();
        assert_eq!(
            strs(node, "compatible")[0],
            format!("camelot,{}", &name[..name.find('@').unwrap()])
        );
        assert_eq!(node.u32("interrupts"), irq, "{name}");
    }
    // Sv48 harts say so.
    let mut m = Machine::new(rv64(1, 8 << 20, Bios::Default)).unwrap();
    m.load_elf(&elf64(FW_JUMP_ADDR, &bytes(&[J_SELF]))).unwrap();
    let tree = tree_at(&m, read64(&m, FDT_ADDR_ADDR));
    let cpu = tree.root.find("cpus/cpu@0").unwrap();
    assert_eq!(cpu.str("mmu-type").as_deref(), Some("riscv,sv48"));
}

#[test]
fn camelots_firmware_starts_the_program_in_s_mode() {
    // At 0x8020_0000 in S-mode, a0 = 0 and a1 = the device tree, paging off; the
    // program's SRST shutdown stops the machine with its code.
    for harts in [1, 4] {
        let mut m = Machine::new(rv64(harts, 8 << 20, Bios::Default)).unwrap();
        m.load_elf(&elf64(FW_JUMP_ADDR, &bytes(&shutdown(42)))).unwrap();
        let fdt = read64(&m, FDT_ADDR_ADDR);
        step_until(&mut m, |m| m.cpu.hart(0).pc == FW_JUMP_ADDR);
        let h = m.cpu.hart(0);
        assert_eq!((h.mode, h.x[10], h.x[11], h.csr.satp), (Mode::S, 0, fdt, 0));
        // The other harts wait in the firmware, in M-mode, for a hart_start.
        for i in 1..harts {
            let h = m.cpu.hart(i);
            assert!(
                h.mode == Mode::M && h.pc < FW_JUMP_ADDR,
                "hart {i} at {:#x}",
                h.pc
            );
        }
        assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(42));
    }
}

/// `csrr rd, csr` (`csrrs rd, csr, zero`), as `rdcycle`, `rdtime` and `rdinstret` are.
fn csrr(rd: u32, csr: u32) -> u32 {
    csr << 20 | 2 << 12 | rd << 7 | 0x73
}
const CYCLE: u32 = 0xc00;
const TIME: u32 = 0xc01;
const INSTRET: u32 = 0xc02;

#[test]
fn camelots_firmware_lets_s_mode_read_the_counters_as_opensbi_does() {
    // The device tree promises S-mode Zicntr, and OpenSBI's mstatus_init() writes
    // mcounteren all ones (Grail keeps CY, TM and IR) and scounteren TM. A program
    // that reads cycle, time and instret in S-mode exits 42: one the firmware did
    // not let would take an illegal-instruction trap to stvec 0 instead.
    let mut code = vec![csrr(A0, CYCLE), csrr(A0, TIME), csrr(A0, INSTRET)];
    code.extend(shutdown(42));
    let mut m = Machine::new(rv64(1, 8 << 20, Bios::Default)).unwrap();
    m.load_elf(&elf64(FW_JUMP_ADDR, &bytes(&code))).unwrap();
    step_until(&mut m, |m| m.cpu.hart(0).pc == FW_JUMP_ADDR);
    let h = m.cpu.hart(0);
    assert_eq!(
        (h.mode, h.csr.mcounteren, h.csr.scounteren),
        (Mode::S, 0b111, 0b010)
    );
    assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(42));
}

/// A disk whose boot block is `code`, sealed, followed by blank sectors.
fn boot_disk(code: &[u32]) -> Vec<u8> {
    let mut block = [0u8; BOOT_BLOCK_SIZE];
    let code = bytes(code);
    block[..code.len()].copy_from_slice(&code);
    grail::boot::seal(&mut block);
    let mut disk = vec![0u8; 16 * 512];
    disk[..BOOT_BLOCK_SIZE].copy_from_slice(&block);
    disk
}

#[test]
fn camelots_firmware_boots_a_disk_in_s_mode() {
    // No program: the firmware reads disk 0's boot block to 0x8020_0000 and starts
    // it in S-mode with a0 = 0, a1 = the device tree and a2 = the disk's registers.
    let config = MachineConfig {
        disks: vec![DiskConfig::memory(boot_disk(&shutdown(7)))],
        ..rv64(1, 6 << 20, Bios::Default)
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    let fdt = read64(&m, FDT_ADDR_ADDR);
    assert_eq!(fdt, 0x8040_0000);
    let info = read64(&m, grail::boot::FW_DYNAMIC_INFO_ADDR + 16);
    assert_eq!(info, 0, "fw_dynamic_info.next_addr: no program");
    step_until(&mut m, |m| m.cpu.hart(0).pc == FW_JUMP_ADDR);
    let h = m.cpu.hart(0);
    assert_eq!((h.mode, h.x[10], h.x[11], h.x[12]), (Mode::S, 0, fdt, BLK_BASE));
    assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(7));

    // A disk the firmware can't boot: why, on the UART, and the boot ROM's 0xfd.
    let mut unsealed = boot_disk(&shutdown(7));
    unsealed[BOOT_BLOCK_SIZE - 4] ^= 1;
    let mut corrupt = boot_disk(&shutdown(7));
    corrupt[0] ^= 1;
    for (disk, why) in [
        (vec![0u8; 512], "disk 0 is too small to hold a boot block"),
        (vec![0u8; 16 * 512], "disk 0 is not bootable (no boot signature)"),
        (unsealed, "disk 0 is not bootable (no boot signature)"),
        (corrupt, "the boot block on disk 0 is corrupt (bad checksum)"),
    ] {
        let config = MachineConfig {
            disks: vec![DiskConfig::memory(disk)],
            ..rv64(1, 6 << 20, Bios::Default)
        };
        let mut m = Machine::new(config).unwrap();
        m.cpu.bus.uart.capture = true;
        m.boot_from_disk().unwrap();
        assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(0xfd), "{why}");
        assert_eq!(
            String::from_utf8_lossy(&m.cpu.bus.uart.captured),
            format!("firmware: {why}\n")
        );
    }
}

#[test]
fn a_machine_refuses_firmware_it_cannot_run() {
    let program = elf64(FW_JUMP_ADDR, &bytes(&[J_SELF]));
    let refused = |config: MachineConfig, program: Option<&[u8]>| {
        let mut m = Machine::new(config).unwrap();
        match program {
            Some(p) => m.load_elf(p).unwrap_err(),
            None => m.boot_from_disk().unwrap_err(),
        }
    };
    // RV32 machines have none, and an ELF32 program keeps its machine RV32.
    let rv32 = MachineConfig {
        bios: Bios::Default,
        ..MachineConfig::with_ram(8 << 20)
    };
    let hello = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello/hello.elf");
    let elf32 = std::fs::read(&hello).unwrap();
    let err = refused(rv32.clone(), Some(&elf32));
    assert!(err.contains("RV64 machines only"), "{err}");
    // An ELF64 program makes the machine RV64, firmware and all.
    let mut m = Machine::new(rv32).unwrap();
    m.load_elf(&program).unwrap();
    assert_eq!(read64(&m, BOOT_ENTRY_ADDR), FW_HANDOFF_PC);
    // A program in the firmware's 2 MiB.
    let err = refused(rv64(1, 8 << 20, Bios::Default), Some(&elf64(FW_BASE, &[0; 4])));
    assert!(err.contains("is in the firmware's RAM"), "{err}");
    assert!(err.contains("0x80200000 or above"), "{err}");
    // No room for the device tree above the next stage: 4 MiB puts it at
    // 0x8020_0000, where the program (or the disk's boot block) is.
    let err = refused(rv64(1, 4 << 20, Bios::Default), Some(&program));
    assert!(err.contains("no room for the device tree"), "{err}");
    let disk = MachineConfig {
        disks: vec![DiskConfig::memory(boot_disk(&[J_SELF]))],
        ..rv64(1, 4 << 20, Bios::Default)
    };
    let err = refused(disk, None);
    assert!(err.contains("give the machine more RAM"), "{err}");
    // A firmware file that isn't there, or too big for its 2 MiB.
    let err = refused(
        rv64(1, 8 << 20, Bios::File("no/such/fw.bin".into())),
        Some(&program),
    );
    assert!(
        err.starts_with("cannot read the firmware no/such/fw.bin"),
        "{err}"
    );
    let big = TempFile::new("big.bin", &vec![0; FW_SIZE as usize + 4]);
    let err = refused(rv64(1, 8 << 20, Bios::File(big.0.clone())), Some(&program));
    assert!(err.contains("outside the firmware's RAM"), "{err}");
    // An ELF firmware of the wrong width.
    let err = refused(rv64(1, 8 << 20, Bios::File(hello)), Some(&program));
    assert!(
        err.contains("an RV32 (ELF32) firmware cannot run on an RV64 machine"),
        "{err}"
    );
}

#[test]
fn grail_takes_bios() {
    // The command line: --bios none, default or a file.
    let parse = |s: &str| grail::cli::parse(s.split_whitespace().map(String::from)).map(|o| o.config.bios);
    assert_eq!(parse("p.elf").unwrap(), Bios::None);
    assert_eq!(parse("--bios none p.elf").unwrap(), Bios::None);
    assert_eq!(parse("--bios default p.elf").unwrap(), Bios::Default);
    assert_eq!(
        parse("--bios fw_jump.bin p.elf").unwrap(),
        Bios::File("fw_jump.bin".into())
    );
    assert!(parse("--bios").is_err());
    // The binary: Camelot's firmware and a program that shuts down with 5.
    let program = TempFile::new("prog.elf", &elf64(FW_JUMP_ADDR, &bytes(&shutdown(5))));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--bios", "default", "--ram", "8"])
        .arg(&program.0)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Too little RAM: grail says why.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--bios", "default"])
        .arg(&program.0)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no room for the device tree"), "{err}");
}
