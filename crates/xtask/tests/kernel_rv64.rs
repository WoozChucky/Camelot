//! The RV64 kernel's tests (#154): `kernel/kernel-rv64.elf`, a higher-half
//! kernel in Linux's riscv64 layout, booting on RV64 Grail through its boot
//! self-tests and its kernel processes, on 1 to 8 harts, and its panic
//! paths, then running the shell, an RV64 program, as the first process
//! (#155): the boot ends when it exits (`exit`, typed on the UART), and the
//! exit code is the number of failed self-tests, else the shell's status.
//! The tests of RV64 processes, their address space and the LP64 system
//! call ABI are in `processes_rv64.rs`.
//!
//! The image is linked at 0xffff_ffff_8000_0000 and loaded at 0x8020_0000,
//! and runs in S-mode on Camelot's firmware (`--bios default`): its symbols
//! are link addresses, which [`Kernel::pa`] turns into the
//! physical addresses the bus takes, and a pointer into the direct map
//! goes back through `va_pa_offset`, the kernel's own variable.
//!
//! The same tests run on the reference builds (clang -O2 and ld.lld, with
//! clang's or merlin-as's assembly) when LLVM is installed:
//! `the_rv64_reference_builds_pass_the_same_tests`, ignored by default.

mod common;

use common::rv64::{CPU_HARTID, CPU_NOFF, CPU_PAGETABLE, CPU_SIZE, CPU_THREAD, LOCK_CPU};
use grail::{Machine, MachineConfig, RunResult};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use xtask::kernel::{self, Assembler, BOOT_MAX_STEPS};
use xtask::{repo_root, tools};

/// Where the image is linked (kernel/kernel.lds.S) and loaded.
const KERNEL_LINK_ADDR: u64 = 0xffff_ffff_8000_0000;
const LOAD_ADDR: u64 = 0x8020_0000;
/// Where RAM starts: the direct map maps it at PAGE_OFFSET.
const RAM_BASE: u64 = 0x8000_0000;
/// The RAM of the tests' machines: the firmware's 2 MiB, the image's 2.1 MiB from
/// 0x8020_0000, and the device tree at the last 2 MiB boundary below the end.
const RAM: usize = 8 << 20;
/// What depends on the paging mode the kernel chose (vm.h, Linux's
/// vm-layout.rst): Sv48 on a hart that has it, Sv39 on one without.
#[derive(Clone, Copy, Debug)]
struct Mode {
    paging: grail::Paging,
    name: &'static str,
    levels: u32,
    satp_mode: u64,
    /// The direct map's base (Linux's PAGE_OFFSET).
    page_offset: u64,
    /// The vmalloc area's start, where the kernel stacks begin, and the
    /// devices' mappings after them.
    vmalloc_start: u64,
    /// The kernel's root entries in use at the end of the boot: the fixmap,
    /// the vmalloc area (the kernel stacks and the devices), the direct map
    /// of 8 MiB of RAM, the image.
    root_entries: &'static [usize],
}

const SV48: Mode = Mode {
    paging: grail::Paging::Sv48,
    name: "Sv48",
    levels: 4,
    satp_mode: 9 << 60,
    page_offset: 0xffff_af80_0000_0000,
    vmalloc_start: 0xffff_8f80_0000_0000,
    root_entries: &[282, 287, 351, 511],
};

const SV39: Mode = Mode {
    paging: grail::Paging::Sv39,
    name: "Sv39",
    levels: 3,
    satp_mode: 8 << 60,
    page_offset: 0xffff_ffd6_0000_0000,
    vmalloc_start: 0xffff_ffc6_0000_0000,
    root_entries: &[275, 280, 344, 510],
};

thread_local! {
    /// The paging mode the harts of this thread's machines have: Sv48 (and
    /// Sv39), or Sv39 only (`the_rv64_kernel_falls_back_to_sv39`).
    static MODE: std::cell::Cell<Mode> = const { std::cell::Cell::new(SV48) };
}

fn mode() -> Mode {
    MODE.get()
}
/// The kernel stacks' size and slot, and how many slots there are, at the
/// start of the vmalloc area (vm.h).
const KSTACK_SIZE: u64 = 16 * 1024;
const KSTACK_SLOT: u64 = 2 * KSTACK_SIZE;
const KSTACK_SLOTS: u64 = 64;
const PANIC_EXIT: u32 = 0xfe;

/// `boot_fault` values (kernel/main.c).
const BOOT_FAULT_NONE: u32 = 1;
const BOOT_FAULT_ILLEGAL: u32 = 2;
const BOOT_FAULT_LOAD: u32 = 3;
const BOOT_FAULT_STORE_TEXT: u32 = 4;
const BOOT_FAULT_REMAP: u32 = 5;
const BOOT_FAULT_UNMAP: u32 = 6;
const BOOT_FAULT_W_ONLY: u32 = 7;
const BOOT_FAULT_MEGAPAGE: u32 = 8;
const BOOT_FAULT_KFREE_UNALIGNED: u32 = 9;
const BOOT_FAULT_KFREE_KERNEL: u32 = 10;
const BOOT_FAULT_KFREE_PAST_RAM: u32 = 11;
const BOOT_FAULT_DOUBLE_ACQUIRE: u32 = 12;
const BOOT_FAULT_RELEASE_UNHELD: u32 = 13;
const BOOT_FAULT_YIELD_HOLDING: u32 = 14;
const BOOT_FAULT_POP_UNMATCHED: u32 = 15;
const BOOT_FAULT_POP_INTR_ON: u32 = 16;
const BOOT_FAULT_KSTACK_OVERFLOW: u32 = 17;
const BOOT_FAULT_PA_VMALLOC: u32 = 18;
const BOOT_FAULT_BOOT_STACK_OVERFLOW: u32 = 19;

/// What the RV64 kernel prints from the banner to the ticks, with the
/// `[dbg]` line's pc masked.
const BOOT_OUTPUT_TEMPLATE: &str = concat!(
    xtask::banner!(),
    "\
Camelot v0.1.0 booting...
sbi: v2.0, implementation 0x434d4c54 version 0x1
sbi: extensions TIME IPI HSM SRST DBCN
kprintf: d=-42 u=4294967295 x=cafef00d p=0x0000000080000000 s=excalibur c=K %=%
kprintf self-test: 33/33 passed
[dbg] pc=................ a0 = 0x0000000000000000 (0)
ram: 8 MiB at 0x80000000
paging: on (MODE)
vm self-test: 26/26 passed
timer: 10 Hz tick, 10000000 Hz timebase
disk: none
tick 1
tick 2
tick 3
tick 4
tick 5
tick 6
tick 7
tick 8
tick 9
tick 10
"
);

/// The boot's output to the ticks for the mode the harts have.
fn boot_output() -> String {
    BOOT_OUTPUT_TEMPLATE.replace("(MODE)", &format!("({})", mode().name))
}

fn committed() -> Vec<u8> {
    let path = repo_root().join("kernel").join(kernel::KERNEL_ELF_RV64);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

thread_local! {
    /// The kernel the tests on this thread run: the committed one, or a
    /// reference build (`the_rv64_reference_builds_pass_the_same_tests`).
    static UNDER_TEST: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    /// A reference build's clang assembly of the user programs, which its
    /// bootable disk's /bin holds built (`rv64_bootable_image`).
    static UNDER_TEST_USER_ASM: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
}

/// The kernel under test.
fn under_test() -> Vec<u8> {
    UNDER_TEST.with(|u| u.borrow().clone()).unwrap_or_else(committed)
}

/// The kernel a bootable disk holds: the committed disk kernel, without the
/// programs (they are in the image's /bin); a reference build's own, which
/// has them, when one is under test (from a mounted disk it runs /bin's).
fn disk_kernel_under_test() -> Vec<u8> {
    UNDER_TEST.with(|u| u.borrow().clone()).unwrap_or_else(|| {
        let path = repo_root().join("kernel").join(kernel::DISK_KERNEL_RV64);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    })
}

/// What the shell prints when it starts, and its prompt.
const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const PROMPT: &str = "$ ";

/// Replaces the pc of a `[dbg] pc=` line with dots.
fn mask_dbg_pc(out: &str) -> String {
    out.split_inclusive('\n')
        .map(|line| match line.strip_prefix("[dbg] pc=") {
            Some(rest) if rest.len() >= 16 => format!("[dbg] pc=................{}", &rest[16..]),
            _ => line.to_string(),
        })
        .collect()
}

/// The RV64 kernel on a machine, and its symbols.
struct Kernel {
    m: Machine,
    elf: grail::elf::Elf,
}

impl Kernel {
    /// The committed RV64 kernel on a machine of `harts` harts and `ram`
    /// bytes of RAM, not started yet, with `exit` typed on the UART for the
    /// shell, so that a run ends.
    fn load(harts: usize, ram: usize) -> Kernel {
        Kernel::load_elf(&under_test(), harts, ram)
    }

    fn load_elf(file: &[u8], harts: usize, ram: usize) -> Kernel {
        let elf = grail::elf::parse(file).unwrap();
        let mut m = Machine::new(MachineConfig {
            harts,
            ram_size: ram,
            xlen: grail::Xlen::Rv64,
            paging: mode().paging,
            bios: grail::Bios::Default,
            ..MachineConfig::default()
        })
        .unwrap();
        m.cpu.bus.uart.capture = true;
        m.load_elf(file).unwrap();
        m.set_uart_input(kernel::typed(b"exit\n"));
        Kernel { m, elf }
    }

    fn sym(&self, name: &str) -> u64 {
        self.elf
            .symbol(name)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    /// The physical address of kernel pointer `va`: one into the image, or
    /// (once kinit() has run) one into the direct map.
    fn pa(&self, va: u64) -> u64 {
        if va >= KERNEL_LINK_ADDR {
            va - KERNEL_LINK_ADDR + LOAD_ADDR
        } else {
            va.wrapping_sub(self.read64(self.sym("va_pa_offset")))
        }
    }

    fn read32(&self, va: u64) -> u32 {
        self.m.cpu.bus.peek(self.pa(va)).unwrap()
    }

    fn read64(&self, va: u64) -> u64 {
        let pa = if va >= KERNEL_LINK_ADDR {
            va - KERNEL_LINK_ADDR + LOAD_ADDR
        } else {
            self.pa(va)
        };
        let lo = self.m.cpu.bus.peek(pa).unwrap() as u64;
        let hi = self.m.cpu.bus.peek(pa + 4).unwrap() as u64;
        lo | hi << 32
    }

    fn write32(&mut self, va: u64, v: u32) {
        let pa = self.pa(va);
        self.m.cpu.bus.load(pa, &v.to_le_bytes()).unwrap();
    }

    fn run(&mut self) -> RunResult {
        self.m.run(Some(BOOT_MAX_STEPS), |_| {})
    }

    fn printed(&self) -> String {
        String::from_utf8_lossy(&self.m.cpu.bus.uart.captured).into_owned()
    }
}

/// Boots the committed RV64 kernel with `boot_fault` set to `fault`, on one
/// hart. Returns the kernel afterwards, how the run ended and what it printed.
fn run_with_fault(fault: u32) -> (Kernel, RunResult, String) {
    let mut k = Kernel::load(1, RAM);
    let at = k.sym("boot_fault");
    k.write32(at, fault);
    let r = k.run();
    let out = k.printed();
    (k, r, out)
}

/// A panic report (trap.c): the cause line, then `name 0xvalue` pairs for
/// the trap CSRs and `name value` pairs for x1-x31, every value 16 hex
/// digits. Returns the cause and the values; skips the `  va ...` line.
fn parse_panic(out: &str) -> (String, HashMap<String, u64>) {
    let report = &out[out
        .find("panic: ")
        .unwrap_or_else(|| panic!("no panic in:\n{out}"))..];
    let mut lines = report.lines();
    let cause = lines.next().unwrap()["panic: ".len()..].to_string();
    let mut values = HashMap::new();
    for line in lines.filter(|l| !l.starts_with("  va ")) {
        let words: Vec<&str> = line.split_whitespace().collect();
        if !words.len().is_multiple_of(2) || words.is_empty() {
            continue;
        }
        for pair in words.chunks(2) {
            let hex = pair[1].strip_prefix("0x").unwrap_or(pair[1]);
            if hex.len() != 16 {
                continue;
            }
            if let Ok(v) = u64::from_str_radix(hex, 16) {
                values.insert(pair[0].to_string(), v);
            }
        }
    }
    (cause, values)
}

#[test]
fn the_committed_rv64_kernels_are_what_our_toolchain_builds() {
    let dir = repo_root().join("kernel");
    let built = kernel::build_merlin_for(&dir, None, kernel::Xlen::Rv64).unwrap();
    assert!(
        built.elf == committed(),
        "kernel/{} is stale: run cargo xtask kernel",
        kernel::KERNEL_ELF_RV64
    );
    let disk = kernel::disk_kernel(&dir, &built).unwrap();
    let path = dir.join(kernel::DISK_KERNEL_RV64);
    assert!(
        disk == std::fs::read(&path).unwrap(),
        "{} is stale: run cargo xtask kernel",
        path.display()
    );
    // The kernel embeds the RV64 user programs; the disk kernel does not.
    assert!(
        disk.len() < built.elf.len() / 2,
        "{} {}",
        disk.len(),
        built.elf.len()
    );
}

#[test]
fn the_rv64_kernel_is_linked_high_and_loaded_low() {
    let file = under_test();
    let e = elf::Elf::parse(&file).unwrap();
    assert_eq!(e.header.entry, KERNEL_LINK_ADDR);
    let parsed = grail::elf::parse(&file).unwrap();
    assert_eq!(parsed.physical_entry().unwrap(), LOAD_ADDR);
    // Every segment runs high and loads at the same distance below.
    assert!(!parsed.segments.is_empty());
    for s in &parsed.segments {
        assert_eq!(
            s.vaddr - s.paddr,
            KERNEL_LINK_ADDR - LOAD_ADDR,
            "{:#x} {:#x}",
            s.vaddr,
            s.paddr
        );
    }
    // The boot stack is 16 KiB, aligned to twice that (kernel_vec's check).
    let k = Kernel::load(1, RAM);
    let (bottom, top) = (k.sym("__stack_bottom"), k.sym("__stack_top"));
    assert_eq!(top - bottom, KSTACK_SIZE);
    assert_eq!(bottom % KSTACK_SLOT, 0);
    // boot_fault ships as none, in .data.
    assert_eq!(k.read32(k.sym("boot_fault")), BOOT_FAULT_NONE);
    assert!(k.sym("boot_fault") < k.sym("__bss_start"));
}

#[test]
fn the_rv64_kernels_have_no_m_mode_code() {
    // The M-mode layer is the firmware's (#174): no instruction of either RV64
    // kernel's text is M-mode's, no mret, no access to a CSR that only M-mode
    // has (bits 9:8 of its number are its lowest privilege, 3), and none of
    // the SBI's symbols is in it.
    for file in [under_test(), disk_kernel_under_test()] {
        let e = grail::elf::parse(&file).unwrap();
        let sym = |name: &str| e.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"));
        let (text, end) = (sym("__text_start"), sym("__rodata_start"));
        let seg = e
            .segments
            .iter()
            .find(|s| (s.vaddr..s.vaddr + s.mem_size).contains(&text))
            .unwrap();
        // The text, up to the rodata pages (its last page's padding may not be
        // in the file).
        let stop = ((end - seg.vaddr) as usize).min(seg.data.len());
        let code = &seg.data[(text - seg.vaddr) as usize..stop];
        let mut csrs = 0;
        for (i, w) in code.chunks(4).enumerate() {
            let word = u32::from_le_bytes(w.try_into().unwrap());
            let at = text + 4 * i as u64;
            match excalibur::decode(word, excalibur::Xlen::Rv64) {
                excalibur::Inst::Mret => panic!("mret at {at:#x}"),
                excalibur::Inst::Csr { csr, .. } => {
                    assert_ne!(csr >> 8 & 3, 3, "an M-mode CSR ({csr:#x}) at {at:#x}");
                    csrs += 1;
                }
                _ => {}
            }
        }
        assert!(csrs > 20, "the S-mode CSRs it uses: {csrs}");
        for name in [
            "machine_vec",
            "machine_scratch",
            "sbi_coldboot",
            "sbi_hart_init",
            "sbi_hart_wait",
            "early_trap",
        ] {
            assert!(e.symbol(name).is_none(), "{name} is in the RV64 kernel");
        }
        // It loads after the firmware's 2 MiB, where the firmware starts it.
        assert_eq!(e.physical_entry().unwrap(), LOAD_ADDR);
        assert_eq!(LOAD_ADDR, grail::firmware::FW_JUMP_ADDR);
    }
}

#[test]
fn the_rv64_kernel_boots_through_its_self_tests() {
    let mut k = Kernel::load(1, RAM);
    let r = k.run();
    let out = k.printed();
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    let rest = mask_dbg_pc(&out);
    let boot = boot_output();
    assert!(rest.starts_with(&boot), "output:\n{out}");
    let rest = &rest[boot.len()..];
    let lines: Vec<&str> = rest.lines().collect();
    // Kernel stacks freed past each hart's cache are unmapped and their
    // pages freed (vm.h, kstack_free()).
    assert_eq!(lines[0], "kstack self-test: 3/3 passed", "{rest}");
    assert!(lines[1].starts_with("kstack: "), "{rest}");
    // Then the shell, a 64-bit program, as the first process: it reads
    // `exit` and the machine stops with its status.
    assert_eq!(lines[2..], [SHELL_BANNER.trim_end(), "$ exit"], "{rest}");

    // The layout's variables: Sv48 on a hart that has it, else Sv39, with
    // Linux's values for the mode.
    let mode = mode();
    assert_eq!(k.read32(k.sym("pt_levels")), mode.levels);
    assert_eq!(k.read64(k.sym("satp_mode")), mode.satp_mode);
    assert_eq!(k.read64(k.sym("page_offset")), mode.page_offset);
    assert_eq!(k.read64(k.sym("vmalloc_start")), mode.vmalloc_start);
    assert_eq!(k.read64(k.sym("va_pa_offset")), mode.page_offset - RAM_BASE);
    assert_eq!(
        k.read64(k.sym("va_kernel_pa_offset")),
        KERNEL_LINK_ADDR - LOAD_ADDR
    );
    // The kernel's page table: a direct-map pointer, in satp's form in
    // kernel_satp, and hart 0's struct cpu has it.
    let kpt = k.read64(k.sym("kernel_pagetable"));
    assert!(
        kpt >= mode.page_offset && kpt < mode.page_offset + RAM as u64,
        "{kpt:#x}"
    );
    assert_eq!(k.read64(k.sym("kernel_satp")), mode.satp_mode | k.pa(kpt) >> 12);
    let cpus = k.sym("cpus");
    assert_eq!(k.read64(cpus + CPU_PAGETABLE), kpt);
    // The firmware's a1, the device tree Grail made, kept for the kernel
    // (Linux's dtb_early_pa).
    let fdt = k.m.cpu.bus.read_mem(grail::boot::FDT_ADDR_ADDR, 8).unwrap();
    assert_eq!(fdt, 0x8060_0000);
    assert_eq!(k.read64(k.sym("dtb_early_pa")), fdt);
    // Every lock let go, hart 0 out of push_off.
    assert_eq!(k.read32(cpus + CPU_NOFF), 0);
    for name in ["kmem_lock", "proc_lock", "uart_tx_lock", "tickslock"] {
        let lock = k.sym(name);
        assert_eq!(k.read32(lock), 0, "{name} held");
        assert_eq!(k.read64(lock + LOCK_CPU), 0, "{name}'s cpu");
    }
}

#[test]
fn the_page_table_maps_the_kernel_half_only() {
    let mut k = Kernel::load(1, RAM);
    assert_eq!(k.run(), RunResult::Exited(0));
    let kpt = k.pa(k.read64(k.sym("kernel_pagetable")));
    let root: Vec<u64> = (0..512).map(|i| k.read64_pa(kpt + 8 * i)).collect();
    // Nothing in the lower half: the identity mapping is gone.
    assert!(root[..256].iter().all(|&e| e & 1 == 0), "lower half mapped");
    // The fixmap, the vmalloc area (the kernel stacks, the devices), the
    // direct map and the image, each through a table under the root.
    let used: Vec<usize> = (256..512).filter(|&i| root[i] & 1 != 0).collect();
    assert_eq!(used, mode().root_entries);
    for &i in mode().root_entries {
        assert_eq!(root[i] & 0xf, 1, "root[{i}] = {:#x}", root[i]);
    }
    // The fixmap's entry is the early page table's own, as Linux shares its
    // fixmap tables between early_pg_dir and swapper_pg_dir.
    let early = k.pa(k.sym("early_pg_dir"));
    let fix = mode().root_entries[0];
    assert_eq!(root[fix], k.read64_pa(early + 8 * fix as u64));

    // The devices the kernel uses, ioremap()ed after the kernel stacks,
    // each in its own place: sysinfo and the UART, which the boot reached
    // through the fixmap first. Every leaf has A, D and G.
    let ioremap = mode().vmalloc_start + KSTACK_SLOTS * KSTACK_SLOT;
    for (name, pa) in [
        ("sysinfo_regs", 0x3000_2000),
        ("uart_regs", grail::bus::UART_BASE),
    ] {
        let va = k.read64(k.sym(name));
        assert!(
            va >= ioremap && va < mode().vmalloc_start + (1 << 30),
            "{name} {va:#x}"
        );
        let (got, pte) = k
            .translate(kpt, va)
            .unwrap_or_else(|| panic!("{name} {va:#x} unmapped"));
        assert_eq!(got, pa, "{name}");
        assert_eq!(pte & 0xff, 0xe7, "{name}: {pte:#x}");
    }
    // Nothing the kernel doesn't use: the CLINT and the test device are
    // M-mode's, and no driver asked for the framebuffer.
    let mapped = |pa: u64| {
        (0..(256u64 << 20) >> 12).any(|i| {
            k.translate(kpt, ioremap + (i << 12))
                .is_some_and(|(p, _)| p == pa)
        })
    };
    assert!(!mapped(grail::bus::CLINT_BASE), "the CLINT is mapped");
    assert!(!mapped(grail::bus::TEST_DEV_BASE), "the test device is mapped");
    assert!(!mapped(grail::bus::FB_CTRL_BASE), "the framebuffer is mapped");
    assert!(mapped(grail::bus::PLIC_BASE), "the PLIC is not mapped");

    // The early page table maps RAM exactly, and no devices but the
    // fixmap's: its direct map ends with RAM.
    let ram_end = mode().page_offset + RAM as u64;
    assert!(k.translate(early, ram_end - 4096).is_some());
    assert!(k.translate(early, ram_end).is_none());
    assert!(k.translate(early, mode().vmalloc_start).is_none());
    assert!(k.translate(early, mode().vmalloc_start + 0x1000_0000).is_none());
}

impl Kernel {
    fn read64_pa(&self, pa: u64) -> u64 {
        let lo = self.m.cpu.bus.peek(pa).unwrap() as u64;
        let hi = self.m.cpu.bus.peek(pa + 4).unwrap() as u64;
        lo | hi << 32
    }

    /// What the page table whose root is at physical address `root` maps
    /// `va` to: the physical address and the leaf PTE, or None.
    fn translate(&self, root: u64, va: u64) -> Option<(u64, u64)> {
        let mut table = root;
        for level in (0..mode().levels).rev() {
            let vpn = va >> (12 + 9 * level) & 0x1ff;
            let pte = self.read64_pa(table + 8 * vpn);
            if pte & 1 == 0 {
                return None;
            }
            let pa = (pte >> 10 & 0xfff_ffff_ffff) << 12;
            if pte & 0xe != 0 {
                let offset = va & ((1u64 << (12 + 9 * level)) - 1);
                return Some((pa + offset, pte));
            }
            table = pa;
        }
        None
    }
}

#[test]
fn the_rv64_kernel_runs_on_up_to_eight_harts() {
    for harts in [2, 3, 8] {
        let mut k = Kernel::load(harts, RAM);
        let r = k.run();
        let out = k.printed();
        assert_eq!(r, RunResult::Exited(0), "{harts} harts:\n{out}");
        assert!(out.contains(&format!("\nsmp: {harts} harts running\n")), "{out}");
        assert!(!out.contains("TLB"), "{out}");
        // Every hart has its struct cpu, and runs on the kernel page table.
        let cpus = k.sym("cpus");
        let kpt = k.read64(k.sym("kernel_pagetable"));
        for h in 0..harts as u64 {
            let cpu = cpus + h * CPU_SIZE;
            assert_eq!(k.read32(cpu + CPU_HARTID), h as u32);
            assert_eq!(k.read64(cpu + CPU_PAGETABLE), kpt, "hart {h}");
        }
        // The other harts run on the kernel stack smp_boot() gave each with
        // hart_start: in the vmalloc area, in the lower half of a
        // KSTACK_SLOT aligned slot, as every kernel stack is (vm.h).
        for h in 1..harts as u64 {
            let sp = k.m.cpu.hart(h as usize).x[2];
            let slot = sp & !(KSTACK_SLOT - 1);
            assert!(sp > slot && sp <= slot + KSTACK_SIZE, "{sp:#x}");
            let area = mode().vmalloc_start..=mode().vmalloc_start + KSTACK_SLOTS * KSTACK_SLOT;
            assert!(area.contains(&sp), "{sp:#x}");
            // The scheduler isn't running a thread of a process at the end.
            assert_eq!(k.read64(cpus + h * CPU_SIZE + CPU_THREAD), 0);
        }
    }
}

#[test]
fn the_rv64_kernel_configures_its_network_with_dhcp() {
    // Version 2 of the network card (64-bit ring bases, 16-byte descriptors
    // with their buffers' physical addresses) and Grail's user-mode gateway.
    let file = under_test();
    let mut m = Machine::new(MachineConfig {
        xlen: grail::Xlen::Rv64,
        paging: mode().paging,
        bios: grail::Bios::Default,
        ram_size: RAM,
        nets: vec![grail::NetConfig::user(Vec::new())],
        ..MachineConfig::default()
    })
    .unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    m.set_uart_input(kernel::typed(b"exit\n"));
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(r, RunResult::Exited(0), "{out}");
    assert!(out.contains("\nnet 0: mac 52:54:00:12:34:56, irq "), "{out}");
    assert!(
        out.contains("\nnet: 10.0.2.15/24, gateway 10.0.2.2, dns 10.0.2.3 (dhcp)\n"),
        "{out}"
    );
}

#[test]
fn the_rv64_kernel_takes_ram_from_sysinfo() {
    // 8 MiB at least: the image is 2.1 MiB with the programs it embeds, from
    // 0x8020_0000, after the firmware's 2 MiB, and the device tree goes at
    // the last 2 MiB boundary below the end of RAM, above it.
    for mib in [8usize, 10, 64] {
        let mut k = Kernel::load(1, mib << 20);
        let r = k.run();
        let out = k.printed();
        assert_eq!(r, RunResult::Exited(0), "{mib} MiB:\n{out}");
        assert!(
            out.contains(&format!("\nram: {mib} MiB at 0x80000000\n")),
            "{out}"
        );
        assert_eq!(k.read64(k.sym("ram_size")), (mib as u64) << 20);
    }
}

#[test]
fn kernel_stacks_keep_a_margin() {
    let mut k = Kernel::load(2, RAM);
    assert_eq!(k.run(), RunResult::Exited(0));
    let out = k.printed();
    let line = out.lines().find(|l| l.starts_with("kstack: ")).unwrap();
    let used: u64 = line["kstack: ".len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        line.ends_with(&format!(" of {KSTACK_SIZE} bytes used at most")),
        "{line}"
    );
    // Measured, and with three quarters of the stack to spare.
    assert!(used > 0 && used < KSTACK_SIZE / 4, "{line}");
}

#[test]
fn illegal_instruction_panics_with_a_64_bit_register_dump() {
    let (k, r, out) = run_with_fault(BOOT_FAULT_ILLEGAL);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(mask_dbg_pc(before), boot_output());
    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "illegal instruction");
    assert_eq!(v["scause"], 2);
    assert_eq!(v["stval"], 0xc000_1073);
    let func = k.sym("illegal_instruction");
    // The unimp after the 64-bit li's, which take up to 8 instructions each.
    assert!((func..func + 0x400).contains(&v["sepc"]), "sepc {:#x}", v["sepc"]);
    assert_eq!(k.read32(v["sepc"]), 0xc000_1073);
    // Every register but sp holds its number in each of its 8 bytes: the
    // trap frame's 8-byte slots, all 64 bits of each.
    for i in 1..32u8 {
        if i != 2 {
            let name = excalibur::reg::abi_name(i);
            assert_eq!(v[name], u64::from(i) * 0x0101_0101_0101_0101, "{name}");
        }
    }
    let sp = v["sp"];
    assert!(
        sp > k.sym("__stack_bottom") && sp <= k.sym("__stack_top"),
        "{sp:#x}"
    );
    assert_eq!(sp % 16, 0);
}

#[test]
fn unmapped_load_and_text_store_page_fault() {
    let (_, r, out) = run_with_fault(BOOT_FAULT_LOAD);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "load page fault");
    assert_eq!((v["scause"], v["stval"]), (13, 0x100));
    // The lower half is not mapped at all.
    assert!(out.contains("\n  va 0x0000000000000100 is not mapped\n"), "{out}");

    let (k, r, out) = run_with_fault(BOOT_FAULT_STORE_TEXT);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "store page fault");
    let kmain = k.sym("kmain");
    assert_eq!(v["stval"], kmain);
    // kmain's page at its link address, read-only, onto its physical page.
    let pte_line = out.lines().find(|l| l.starts_with("  va ")).unwrap();
    let pa = kmain - KERNEL_LINK_ADDR + LOAD_ADDR;
    assert!(
        pte_line.starts_with(&format!("  va {kmain:#018x} maps to pa {pa:#018x}, pte ")),
        "{pte_line}"
    );
    // R+X, not W, not U, and A, D and G, which every kernel leaf has (vm.h's
    // PTE_KERNEL): a hart that does not set A and D itself would fault.
    let flags = pte_line.rsplit(' ').next().unwrap();
    assert_eq!(flags, "DAG-X-RV", "{pte_line}");
}

/// The panics of the memory API's and the locks' misuses.
#[test]
fn misuses_panic() {
    for (fault, want) in [
        (BOOT_FAULT_REMAP, "panic: map_pages: remap\n"),
        (
            BOOT_FAULT_UNMAP,
            "unmap_pages: 0x0000000000000000 is not mapped\npanic: unmap_pages: not mapped\n",
        ),
        (BOOT_FAULT_W_ONLY, "panic: map_pages: bad permissions\n"),
        (BOOT_FAULT_MEGAPAGE, "panic: walk: unexpected superpage\n"),
        (BOOT_FAULT_KFREE_UNALIGNED, "panic: kfree\n"),
        (BOOT_FAULT_KFREE_KERNEL, "panic: kfree\n"),
        (BOOT_FAULT_KFREE_PAST_RAM, "panic: kfree\n"),
        (
            BOOT_FAULT_DOUBLE_ACQUIRE,
            "panic: acquire: lock \"boot_fault\" already held by hart 0\n",
        ),
        (
            BOOT_FAULT_RELEASE_UNHELD,
            "panic: release: lock \"boot_fault\" not held by hart 0\n",
        ),
        (
            BOOT_FAULT_YIELD_HOLDING,
            "panic: sched: holding a lock other than proc_lock\n",
        ),
        (BOOT_FAULT_POP_UNMATCHED, "panic: pop_off: no push_off to match\n"),
        (BOOT_FAULT_POP_INTR_ON, "panic: pop_off: interrupts are on\n"),
    ] {
        let (_, r, out) = run_with_fault(fault);
        assert_eq!(r, RunResult::Exited(PANIC_EXIT), "fault {fault}:\n{out}");
        assert!(out.ends_with(want), "fault {fault}: want {want:?}, got:\n{out}");
    }
}

#[test]
fn the_boot_stack_has_a_guard_page() {
    // The page below hart 0's boot stack is past .bss, and unmapped: an
    // overflow faults there, and kernel_vec reports it like a kernel
    // stack's.
    let (k, r, out) = run_with_fault(BOOT_FAULT_BOOT_STACK_OVERFLOW);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let bottom = k.sym("__stack_bottom");
    assert_eq!(bottom % KSTACK_SLOT, 0);
    assert!(bottom - 4096 >= k.sym("__bss_end"));
    let kpt = k.pa(k.read64(k.sym("kernel_pagetable")));
    assert!(k.translate(kpt, bottom - 4096).is_none());
    assert!(k.translate(kpt, bottom).is_some());
    assert!(k.translate(kpt, bottom - 8192).is_some());
    let report = &out[out.find("panic: ").unwrap()..];
    let mut lines = report.lines();
    assert_eq!(
        lines.next(),
        Some("panic: kernel stack overflow on hart 0's boot stack"),
        "{report}"
    );
    assert_eq!(
        lines.next().unwrap().rsplit(' ').next(),
        Some(format!("{bottom:#018x}-{:#018x}", bottom + KSTACK_SIZE).as_str()),
        "{report}"
    );
    assert_eq!(lines.next(), Some("  store page fault"), "{report}");
}

#[test]
fn pa_of_a_vmalloc_address_panics() {
    // pa() of the UART's ioremap() address: in the vmalloc area, where a
    // virtual address has no physical one to compute (Linux's
    // CONFIG_DEBUG_VIRTUAL check), so it panics instead of giving a wrong one.
    let (k, r, out) = run_with_fault(BOOT_FAULT_PA_VMALLOC);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let uart = k.read64(k.sym("uart_regs"));
    let want = format!(
        "pa: {uart:#018x} is neither in the direct map nor in the kernel image\npanic: pa: no physical address\n"
    );
    assert!(out.ends_with(&want), "want {want:?}, got:\n{out}");
}

#[test]
fn a_kernel_stack_overflow_hits_the_guard_page_and_is_reported() {
    let (k, r, out) = run_with_fault(BOOT_FAULT_KSTACK_OVERFLOW);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let report = &out[out.find("panic: ").unwrap()..];
    let mut lines = report.lines();
    assert_eq!(
        lines.next(),
        Some("panic: kernel stack overflow in overflow (hart 0)"),
        "{report}"
    );
    // The stack the thread ran out of: a slot's lower half in the vmalloc
    // area, and sp just below it, in the guard page.
    let (_, v) = parse_panic(report);
    let sp_line = lines.next().unwrap();
    let words: Vec<&str> = sp_line.split_whitespace().collect();
    assert_eq!(words[0], "sp", "{sp_line}");
    let sp = u64::from_str_radix(&words[1][2..18], 16).unwrap();
    let range: Vec<u64> = words[4]
        .split('-')
        .map(|w| u64::from_str_radix(&w[2..], 16).unwrap())
        .collect();
    assert_eq!(range[1] - range[0], KSTACK_SIZE);
    assert_eq!(range[0] % KSTACK_SLOT, 0);
    assert!(range[0] >= mode().vmalloc_start, "{report}");
    assert!(
        range[1] <= mode().vmalloc_start + KSTACK_SLOTS * KSTACK_SLOT,
        "{report}"
    );
    // The fault itself: a store to the guard page, the page below the
    // stack, from the recursion (not from kernel_vec: the frame went to the
    // overflow stack instead).
    assert_eq!(lines.next(), Some("  store page fault"), "{report}");
    assert_eq!(v["scause"], 15);
    let stval = v["stval"];
    assert!(
        (range[0] - 4096..range[0]).contains(&stval),
        "stval {stval:#x}\n{report}"
    );
    assert!(
        (range[0] - 4096..range[0] + 512).contains(&sp),
        "sp {sp:#x}\n{report}"
    );
    let recurse = k.sym("recurse");
    assert!(
        (recurse..recurse + 0x100).contains(&v["sepc"]),
        "sepc {:#x}",
        v["sepc"]
    );
    assert!(
        report.contains(&format!("\n  va {stval:#018x} is not mapped\n")),
        "{report}"
    );
}

/// A bootable RV64 disk: the root image's files (kernel/fs, /dev/console,
/// the RV64 programs in /bin, `xtask::fs::build_for`) in the filesystem,
/// the RV64 bootloader in the boot block and the RV64 disk kernel after the
/// filesystem, as `camelot-fs mkfs --boot` makes one.
fn rv64_bootable_image(blocks: u32) -> (Vec<u8>, camelot_fs::boot::BootInfo) {
    let root = repo_root();
    let kernel = disk_kernel_under_test();
    let opts = xtask::fs::bootable_layout(blocks, &kernel).unwrap();
    // A reference kernel checks that /bin holds the programs it embeds: its
    // own, built from clang's assembly.
    let asm = UNDER_TEST_USER_ASM.with(|a| a.borrow().clone());
    let cc = match &asm {
        Some(dir) => xtask::userprogs::CCompiler::Clang(dir),
        None => xtask::userprogs::CCompiler::Merlin,
    };
    let mut image = xtask::fs::build_for_with(&root, &opts, kernel::Xlen::Rv64, cc).unwrap();
    let boot = xtask::boot::bootloader_rv64(&root).unwrap();
    let info = camelot_fs::boot::install(&mut image, &boot, &kernel).unwrap();
    (image, info)
}

#[test]
fn the_rv64_kernel_boots_from_disk() {
    let root = repo_root();
    let (image, info) = rv64_bootable_image(4096);
    for harts in [1, 4] {
        let mut m = Machine::new(MachineConfig {
            harts,
            xlen: grail::Xlen::Rv64,
            paging: mode().paging,
            bios: grail::Bios::Default,
            // The firmware's 2 MiB on top of the 4 MiB an RV32 kernel boots on.
            ram_size: 6 << 20,
            disks: vec![grail::DiskConfig::memory(image.clone())],
            ..MachineConfig::default()
        })
        .unwrap();
        m.boot_from_disk().unwrap();
        m.cpu.bus.uart.capture = true;
        m.set_uart_input(kernel::typed(b"hello\nls /bin\nexit\n"));
        let r = m.run(Some(2 * BOOT_MAX_STEPS), |_| {});
        let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
        // The bootloader found the kernel, loaded its segments at their
        // physical addresses and started it at the entry's: the kernel
        // turned paging on and ran its self-tests, the disk's too.
        assert_eq!(r, RunResult::Exited(0), "{harts} harts:\n{out}");
        assert!(
            out.starts_with(&format!(
                "boot: kernel at sector 0x{:08x}\n{}Camelot v0.1.0 booting...\n",
                info.kernel_sector,
                xtask::kernel::BANNER
            )),
            "{out}"
        );
        assert!(
            out.contains(&format!("\npaging: on ({})\n", mode().name)),
            "{out}"
        );
        assert!(out.contains("\ndisk 0: 8192 sectors (4096 KiB), irq "), "{out}");
        // The disk driver's DMA (physical addresses of direct-map buffers),
        // the filesystem on it, and their self-tests.
        assert!(out.contains("\nblk self-test: 16/16 passed\n"), "{out}");
        assert!(out.contains("\nfs: disk 0 mounted: "), "{out}");
        // One more check than without /bin: the programs there ("N
        // programs, the kernel none" for the disk kernel, "N of the
        // kernel's N programs" for a reference build, which embeds them):
        // every program on the disk, as and ld too, which the kernel never
        // embeds.
        let user = xtask::userprogs::user_dir(&root);
        let holds = if UNDER_TEST_USER_ASM.with(|a| a.borrow().is_some()) {
            let n = xtask::userprogs::embedded_names(&user).unwrap().len();
            format!("\nfs: /bin holds {n} of the kernel's {n} programs\n")
        } else {
            let n = xtask::userprogs::program_names(&user).unwrap().len();
            format!("\nfs: /bin holds {n} programs, the kernel none\n")
        };
        assert!(out.contains(&holds), "{out}");
        assert!(out.contains("\nfs self-test: 12/12 passed\n"), "{out}");
        assert!(!out.contains("FAILED"), "{out}");
        // Then the shell from /bin/sh on the disk, and the programs it
        // runs from there: 64-bit ELF files, read through the filesystem.
        assert!(
            out.contains(&format!(
                "{SHELL_BANNER}{PROMPT}hello\nhello from user mode\n{PROMPT}ls /bin\n"
            )),
            "{out}"
        );
        assert!(out.contains(" sh\n") && out.contains(" systest\n"), "{out}");
        assert!(out.ends_with(&format!("{PROMPT}exit\n")), "{out}");
        if harts > 1 {
            assert!(out.contains(&format!("\nsmp: {harts} harts running\n")), "{out}");
        }
        // The bootloader ran in S-mode and wrote nothing to the ROM: its
        // entry doubleword is still the hand-off to the firmware, which
        // started the other harts for the kernel (HSM hart_start).
        let lo = m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR).unwrap() as u64;
        let hi = m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR + 4).unwrap() as u64;
        assert_eq!(lo | hi << 32, grail::boot::FW_HANDOFF_PC);
    }
}

/// The RV64 reference build (`cargo xtask kernel --cc clang --xlen 64`) into
/// a scratch directory, with `assembler` for the `.S` files. Returns the
/// kernel and the directory of clang's assembly of the user programs.
fn reference_build(assembler: Assembler, name: &str) -> (Vec<u8>, std::path::PathBuf) {
    let clang = tools::find("clang").expect("clang (the official LLVM install, or on PATH)");
    let lld = tools::find_beside("ld.lld", &clang).expect("ld.lld next to clang");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("kernel-rv64-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = kernel::build_reference_for(
        &repo_root(),
        &clang,
        &lld,
        assembler,
        &dir,
        &dir.join("kernel.elf"),
        false,
        kernel::Xlen::Rv64,
    )
    .unwrap();
    (std::fs::read(out).unwrap(), dir.join("user"))
}

#[test]
#[ignore = "needs clang and ld.lld"]
fn the_rv64_reference_builds_pass_the_same_tests() {
    // clang -O2 and ld.lld, with clang's assembler and then with merlin-as:
    // every test above but the one that compares the committed kernel with
    // merlin's build, on this thread's kernel under test.
    for (assembler, name) in [(Assembler::Clang, "clang"), (Assembler::Merlin, "merlin-as")] {
        let (elf, user_asm) = reference_build(assembler, name);
        assert_ne!(elf, committed(), "{name}: the reference build is merlin's");
        UNDER_TEST_USER_ASM.with(|a| *a.borrow_mut() = Some(user_asm));
        UNDER_TEST.with(|u| *u.borrow_mut() = Some(elf));
        MODE.set(SV48);
        the_rv64_kernel_falls_back_to_sv39();
        MODE.set(SV48);
        the_rv64_kernel_is_linked_high_and_loaded_low();
        the_rv64_kernel_boots_through_its_self_tests();
        the_page_table_maps_the_kernel_half_only();
        the_rv64_kernel_runs_on_up_to_eight_harts();
        the_rv64_kernel_takes_ram_from_sysinfo();
        the_rv64_kernel_configures_its_network_with_dhcp();
        kernel_stacks_keep_a_margin();
        illegal_instruction_panics_with_a_64_bit_register_dump();
        unmapped_load_and_text_store_page_fault();
        misuses_panic();
        a_kernel_stack_overflow_hits_the_guard_page_and_is_reported();
        the_rv64_kernel_boots_from_disk();
    }
}

#[test]
fn the_rv64_kernel_falls_back_to_sv39() {
    // A hart without Sv48 (Grail's --paging sv39): satp refuses MODE 9, the
    // kernel's probe sees it and builds its tables for Sv39, with Linux's
    // Sv39 layout; the image's address is the same. Every test above but
    // the build's own, on such harts.
    MODE.set(SV39);
    the_rv64_kernel_boots_through_its_self_tests();
    the_page_table_maps_the_kernel_half_only();
    the_rv64_kernel_runs_on_up_to_eight_harts();
    the_rv64_kernel_takes_ram_from_sysinfo();
    the_rv64_kernel_configures_its_network_with_dhcp();
    kernel_stacks_keep_a_margin();
    illegal_instruction_panics_with_a_64_bit_register_dump();
    unmapped_load_and_text_store_page_fault();
    misuses_panic();
    a_kernel_stack_overflow_hits_the_guard_page_and_is_reported();
    the_rv64_kernel_boots_from_disk();
}
