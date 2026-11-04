//! The RV64 kernel's tests (#154): `kernel/kernel-rv64.elf`, a higher-half
//! kernel in Linux's riscv64 layout, booting on RV64 Grail through its boot
//! self-tests and its kernel processes, on 1 to 8 harts, and its panic
//! paths. It has no user space yet (#155), so the boot ends after the
//! self-tests and the exit code is the number of failed ones.
//!
//! The image is linked at 0xffff_ffff_8000_0000 and loaded at 0x8000_0000:
//! its symbols are link addresses, which [`Kernel::pa`] turns into the
//! physical addresses the bus takes, and a pointer into the direct map
//! goes back through `va_pa_offset`, the kernel's own variable.

mod common;

use common::rv64::{CPU_HARTID, CPU_NOFF, CPU_PAGETABLE, CPU_SIZE, CPU_THREAD, LOCK_CPU};
use grail::{Machine, MachineConfig, RunResult};
use std::collections::HashMap;
use xtask::kernel::{self, BOOT_MAX_STEPS};
use xtask::repo_root;

/// Where the image is linked (kernel/kernel64.ld) and loaded.
const KERNEL_LINK_ADDR: u64 = 0xffff_ffff_8000_0000;
const LOAD_ADDR: u64 = 0x8000_0000;
/// The direct map's base in Sv48 (vm.h, Linux's PAGE_OFFSET).
const PAGE_OFFSET_SV48: u64 = 0xffff_af80_0000_0000;
/// The vmalloc area's start in Sv48, where the devices' window begins.
const VMALLOC_START_SV48: u64 = 0xffff_8f80_0000_0000;
/// The kernel stacks' size and slot (vm.h).
const KSTACK_SIZE: u64 = 16 * 1024;
const KSTACK_SLOT: u64 = 2 * KSTACK_SIZE;
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

/// What the RV64 kernel prints from the banner to the ticks, with the
/// `[dbg]` line's pc masked.
const BOOT_OUTPUT: &str = concat!(
    xtask::banner!(),
    "\
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x0000000080000000 s=excalibur c=K %=%
kprintf self-test: 33/33 passed
[dbg] pc=................ a0 = 0x0000000000000000 (0)
ram: 4 MiB at 0x80000000
paging: on (Sv48)
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

fn committed() -> Vec<u8> {
    let path = repo_root().join("kernel").join(kernel::KERNEL_ELF_RV64);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

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
    /// bytes of RAM, not started yet.
    fn load(harts: usize, ram: usize) -> Kernel {
        Kernel::load_elf(&committed(), harts, ram)
    }

    fn load_elf(file: &[u8], harts: usize, ram: usize) -> Kernel {
        let elf = grail::elf::parse(file).unwrap();
        let mut m = Machine::new(MachineConfig {
            harts,
            ram_size: ram,
            xlen: grail::Xlen::Rv64,
            ..MachineConfig::default()
        })
        .unwrap();
        m.cpu.bus.uart.capture = true;
        m.load_elf(file).unwrap();
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
    let mut k = Kernel::load(1, grail::DEFAULT_RAM_SIZE);
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
    // No RV64 user programs yet (#156): the two are the same kernel.
    assert_eq!(disk, built.elf);
}

#[test]
fn the_rv64_kernel_is_linked_high_and_loaded_low() {
    let file = committed();
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
    let k = Kernel::load(1, grail::DEFAULT_RAM_SIZE);
    let (bottom, top) = (k.sym("__stack_bottom"), k.sym("__stack_top"));
    assert_eq!(top - bottom, KSTACK_SIZE);
    assert_eq!(bottom % KSTACK_SLOT, 0);
    // boot_fault ships as none, in .data.
    assert_eq!(k.read32(k.sym("boot_fault")), BOOT_FAULT_NONE);
    assert!(k.sym("boot_fault") < k.sym("__bss_start"));
}

#[test]
fn the_rv64_kernel_boots_through_its_self_tests() {
    let mut k = Kernel::load(1, grail::DEFAULT_RAM_SIZE);
    let r = k.run();
    let out = k.printed();
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    let rest = mask_dbg_pc(&out);
    assert!(rest.starts_with(BOOT_OUTPUT), "output:\n{out}");
    let rest = &rest[BOOT_OUTPUT.len()..];
    let lines: Vec<&str> = rest.lines().collect();
    assert!(lines[0].starts_with("kstack: "), "{rest}");
    assert_eq!(lines[1..], ["user space: not on RV64 yet"], "{rest}");

    // The layout's variables: Sv48 on a hart that has it, Linux's values.
    assert_eq!(k.read32(k.sym("pt_levels")), 4);
    assert_eq!(k.read64(k.sym("satp_mode")), 9 << 60);
    assert_eq!(k.read64(k.sym("page_offset")), PAGE_OFFSET_SV48);
    assert_eq!(k.read64(k.sym("vmalloc_start")), VMALLOC_START_SV48);
    assert_eq!(k.read64(k.sym("va_pa_offset")), PAGE_OFFSET_SV48 - LOAD_ADDR);
    assert_eq!(
        k.read64(k.sym("va_kernel_pa_offset")),
        KERNEL_LINK_ADDR - LOAD_ADDR
    );
    // The kernel's page table: a direct-map pointer, in satp's form in
    // kernel_satp, and hart 0's struct cpu has it.
    let kpt = k.read64(k.sym("kernel_pagetable"));
    assert_eq!(kpt >> 44, PAGE_OFFSET_SV48 >> 44, "{kpt:#x}");
    assert_eq!(k.read64(k.sym("kernel_satp")), 9 << 60 | k.pa(kpt) >> 12);
    let cpus = k.sym("cpus");
    assert_eq!(k.read64(cpus + CPU_PAGETABLE), kpt);
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
    let mut k = Kernel::load(1, grail::DEFAULT_RAM_SIZE);
    assert_eq!(k.run(), RunResult::Exited(0));
    let kpt = k.pa(k.read64(k.sym("kernel_pagetable")));
    let root: Vec<u64> = (0..512).map(|i| k.read64_pa(kpt + 8 * i)).collect();
    // Nothing in the lower half: the identity mapping is gone.
    assert!(root[..256].iter().all(|&e| e & 1 == 0), "lower half mapped");
    // The image (root entry 511), the direct map (351) and the vmalloc area
    // (287) are there.
    for (i, what) in [(511, "image"), (351, "direct map"), (287, "vmalloc area")] {
        assert_eq!(root[i] & 0xf, 1, "{what}: root[{i}] = {:#x}", root[i]);
    }
    let used: Vec<usize> = (256..512).filter(|&i| root[i] & 1 != 0).collect();
    assert_eq!(used, [287, 351, 511]);
}

impl Kernel {
    fn read64_pa(&self, pa: u64) -> u64 {
        let lo = self.m.cpu.bus.peek(pa).unwrap() as u64;
        let hi = self.m.cpu.bus.peek(pa + 4).unwrap() as u64;
        lo | hi << 32
    }
}

#[test]
fn the_rv64_kernel_runs_on_up_to_eight_harts() {
    for harts in [2, 3, 8] {
        let mut k = Kernel::load(harts, grail::DEFAULT_RAM_SIZE);
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
        // The other harts' stacks: a page each at its physical address for
        // M-mode, and a kernel stack in the vmalloc area, KSTACK_SLOT
        // aligned, for the scheduler.
        for h in 1..harts as u64 {
            let boot = k.read64(k.sym("hart_boot_sp") + 8 * h);
            assert!(
                (LOAD_ADDR..LOAD_ADDR + grail::DEFAULT_RAM_SIZE as u64).contains(&boot),
                "{boot:#x}"
            );
            let sp = k.read64(k.sym("hart_sp") + 8 * h);
            assert_eq!((sp - KSTACK_SIZE) % KSTACK_SLOT, 0, "{sp:#x}");
            assert!(sp > VMALLOC_START_SV48 + (4 << 30), "{sp:#x}");
            // The scheduler isn't running a thread of a process at the end.
            assert_eq!(k.read64(cpus + h * CPU_SIZE + CPU_THREAD), 0);
        }
    }
}

#[test]
fn the_rv64_kernel_takes_ram_from_sysinfo() {
    for mib in [2usize, 64] {
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
    let mut k = Kernel::load(2, grail::DEFAULT_RAM_SIZE);
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
    assert_eq!(mask_dbg_pc(before), BOOT_OUTPUT);
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
    assert!(
        pte_line.ends_with(" DA-G-X-RV") || pte_line.ends_with(" -A--X-RV"),
        "{pte_line}"
    );
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
    assert!(range[0] > VMALLOC_START_SV48, "{report}");
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

/// A bootable RV64 disk: the root image's files (kernel/fs, /dev/console) in
/// the filesystem, the RV64 bootloader in the boot block and the RV64 disk
/// kernel after the filesystem, as `camelot-fs mkfs --boot` makes one. No
/// /bin: there are no RV64 programs yet (#156).
fn rv64_bootable_image(blocks: u32) -> (Vec<u8>, camelot_fs::boot::BootInfo) {
    let root = repo_root();
    let kernel = std::fs::read(root.join("kernel").join(kernel::DISK_KERNEL_RV64)).unwrap();
    let opts = xtask::fs::bootable_layout(blocks, &kernel).unwrap();
    let mut fs = camelot_fs::mkfs::Mkfs::new(&opts).unwrap();
    fs.add_host_dir(&root.join(xtask::fs::ROOT_DIR), "/").unwrap();
    fs.mkdir(xtask::fs::DEV).unwrap();
    fs.add_device(
        &format!("{}/console", xtask::fs::DEV),
        xtask::fs::CONSOLE_MAJOR,
        0,
    )
    .unwrap();
    let mut image = fs.finish();
    let boot = xtask::boot::bootloader_rv64(&root).unwrap();
    let info = camelot_fs::boot::install(&mut image, &boot, &kernel).unwrap();
    (image, info)
}

#[test]
fn the_rv64_kernel_boots_from_disk() {
    let (image, info) = rv64_bootable_image(2048);
    for harts in [1, 4] {
        let mut m = Machine::new(MachineConfig {
            harts,
            xlen: grail::Xlen::Rv64,
            disks: vec![grail::DiskConfig::memory(image.clone())],
            ..MachineConfig::default()
        })
        .unwrap();
        m.boot_from_disk().unwrap();
        m.cpu.bus.uart.capture = true;
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
        assert!(out.contains("\npaging: on (Sv48)\n"), "{out}");
        assert!(out.contains("\ndisk 0: 4096 sectors (2048 KiB), irq "), "{out}");
        // The disk driver's DMA (physical addresses of direct-map buffers),
        // the filesystem on it, and their self-tests.
        assert!(out.contains("\nblk self-test: 16/16 passed\n"), "{out}");
        assert!(out.contains("\nfs: disk 0 mounted: "), "{out}");
        assert!(out.contains("\nfs self-test: 11/11 passed\n"), "{out}");
        assert!(!out.contains("FAILED"), "{out}");
        assert!(out.ends_with("user space: not on RV64 yet\n"), "{out}");
        if harts > 1 {
            assert!(out.contains(&format!("\nsmp: {harts} harts running\n")), "{out}");
        }
        // The ROM's entry doubleword is the kernel's physical entry point, so
        // the harts it woke came in at 0x8000_0000, with paging off.
        let lo = m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR).unwrap() as u64;
        let hi = m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR + 4).unwrap() as u64;
        assert_eq!(lo | hi << 32, LOAD_ADDR);
    }
}
