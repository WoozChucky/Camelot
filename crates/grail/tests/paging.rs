//! Sv48 and Sv39 on an RV64 hart (spec section 7, "Paging"): 4 KiB pages and every
//! superpage size, misaligned superpages, reserved PTE bits, non-canonical addresses,
//! the A/D updates and `sfence.vma`. riscv-tests rv64si covers the common path under
//! Sv39; these pin down the rest, Sv48 above all.
//!
//! Most tests call [`Hart::translate`] on a hart in S-mode, over page tables built in
//! RAM by [`Tables`]; the last ones run instructions on a machine.

use excalibur::Xlen;
use grail::bus::{Bus, RAM_BASE};
use grail::cpu::{Hart, Mode};
use grail::trap::{Access, Exception, Trap};
use grail::{Machine, MachineConfig};

const V: u64 = 1 << 0;
const R: u64 = 1 << 1;
const W: u64 = 1 << 2;
const X: u64 = 1 << 3;
const A: u64 = 1 << 6;
const D: u64 = 1 << 7;

/// Every permission, with A and D already set: translating changes nothing.
const RWXAD: u64 = R | W | X | A | D;

const RAM: u64 = RAM_BASE as u64;
const RAM_SIZE: usize = 4 << 20;

#[derive(Clone, Copy, Debug)]
enum Sv {
    Sv39 = 8,
    Sv48 = 9,
}

impl Sv {
    fn levels(self) -> u32 {
        match self {
            Sv::Sv39 => 3,
            Sv::Sv48 => 4,
        }
    }
}

const BOTH: [Sv; 2] = [Sv::Sv39, Sv::Sv48];

/// The page size a leaf at `level` maps: 4 KiB, 2 MiB, 1 GiB, 512 GiB.
fn page_size(level: u32) -> u64 {
    1 << (12 + 9 * level)
}

fn vpn(va: u64, level: u32) -> u64 {
    (va >> (12 + 9 * level)) & 0x1ff
}

fn read64(bus: &mut Bus, addr: u64) -> u64 {
    let lo = bus.read(addr as u32, 4).unwrap();
    let hi = bus.read(addr as u32 + 4, 4).unwrap();
    u64::from(lo) | u64::from(hi) << 32
}

fn write64(bus: &mut Bus, addr: u64, v: u64) {
    bus.write(addr as u32, 4, v as u32).unwrap();
    bus.write(addr as u32 + 4, 4, (v >> 32) as u32).unwrap();
}

/// Page tables in RAM: the root at `RAM + 1 MiB`, the tables below it in the pages
/// after it, one per new table.
struct Tables {
    sv: Sv,
    next: u64,
}

const ROOT: u64 = RAM + 0x10_0000;

impl Tables {
    fn new(sv: Sv) -> Self {
        Tables {
            sv,
            next: ROOT + 0x1000,
        }
    }

    fn satp(&self) -> u64 {
        (self.sv as u64) << 60 | ROOT >> 12
    }

    /// Maps the page of `va` at `level` (0 for 4 KiB, up to the top level for the
    /// largest superpage) to physical `pa`, with `flags` (V added). Makes the tables on
    /// the way that do not exist yet. Returns the leaf PTE's address.
    fn map(&mut self, bus: &mut Bus, va: u64, level: u32, pa: u64, flags: u64) -> u64 {
        self.map_pte(bus, va, level, (pa >> 12) << 10 | flags | V)
    }

    /// Writes `pte`, whatever it holds, as the entry for `va` at `level`.
    fn map_pte(&mut self, bus: &mut Bus, va: u64, level: u32, pte: u64) -> u64 {
        let mut table = ROOT;
        for l in (level + 1..self.sv.levels()).rev() {
            let entry = table + vpn(va, l) * 8;
            let next = read64(bus, entry);
            table = if next & V != 0 {
                (next >> 10) << 12
            } else {
                let t = self.next;
                self.next += 0x1000;
                write64(bus, entry, (t >> 12) << 10 | V);
                t
            };
        }
        let leaf = table + vpn(va, level) * 8;
        write64(bus, leaf, pte);
        leaf
    }
}

/// An RV64 hart in S-mode with paging on, its bus and its tables.
struct Paged {
    hart: Hart,
    bus: Bus,
    tables: Tables,
}

impl Paged {
    fn new(sv: Sv) -> Self {
        let bus = Bus::new(&MachineConfig::with_ram(RAM_SIZE), Vec::new());
        let mut hart = Hart::new(0, RAM, Xlen::Rv64);
        let tables = Tables::new(sv);
        hart.mode = Mode::S;
        hart.csr.satp = tables.satp();
        Paged { hart, bus, tables }
    }

    fn map(&mut self, va: u64, level: u32, pa: u64, flags: u64) -> u64 {
        self.tables.map(&mut self.bus, va, level, pa, flags)
    }

    fn translate(&mut self, va: u64, access: Access) -> Result<u64, Trap> {
        self.hart.translate(&mut self.bus, va, access)
    }

    fn pte(&mut self, addr: u64) -> u64 {
        read64(&mut self.bus, addr)
    }

    /// Checks that `va` faults with the page fault of every access, `xtval` = `va`.
    fn assert_page_faults(&mut self, va: u64, what: &str) {
        for access in [Access::Fetch, Access::Load, Access::Store] {
            assert_eq!(
                self.translate(va, access),
                Err(Trap::new(access.page_fault(), va)),
                "{what}: {access:?} of {va:#x}"
            );
        }
    }
}

/// A virtual address, valid under both modes, whose bits are set at every level and in
/// the page offset, so that each slice of it shows up in the result.
const VA: u64 = 0x2a_b5d3_e9f7;

#[test]
fn every_page_size_translates() {
    for sv in BOTH {
        for level in 0..sv.levels() {
            // A physical page aligned to its size, in RAM where the size allows.
            let pa = match level {
                0..=2 => RAM,
                _ => 0x80_0000_0000,
            };
            let mut p = Paged::new(sv);
            p.map(VA, level, pa, RWXAD);
            let offset = VA & (page_size(level) - 1);
            for access in [Access::Fetch, Access::Load, Access::Store] {
                assert_eq!(
                    p.translate(VA, access),
                    Ok(pa + offset),
                    "{sv:?}, level {level}, {access:?}"
                );
            }
        }
    }
}

#[test]
fn the_upper_half_translates() {
    // The kernel's half of the address space: the root's upper 256 entries.
    for (sv, va) in [
        (Sv::Sv39, 0xffff_ffc0_0000_1000u64),
        (Sv::Sv48, 0xffff_8000_0000_1000),
    ] {
        let mut p = Paged::new(sv);
        let pte = p.map(va, 0, RAM + 0x5000, RWXAD);
        assert_eq!(
            pte,
            ROOT + 0x1000 * (sv.levels() as u64 - 1) + 8,
            "{sv:?}: VPN[0] = 1"
        );
        let root_entry = ROOT + 256 * 8;
        assert_ne!(p.pte(root_entry) & V, 0, "{sv:?}: the root's entry 256");
        assert_eq!(p.translate(va + 0x123, Access::Load), Ok(RAM + 0x5123), "{sv:?}");
    }
}

#[test]
fn a_misaligned_superpage_faults() {
    for sv in BOTH {
        for level in 1..sv.levels() {
            // Aligned to 4 KiB only: one of the PPN bits the levels below would supply.
            let mut p = Paged::new(sv);
            p.map(VA, level, page_size(level) + 0x1000, RWXAD);
            p.assert_page_faults(VA, &format!("{sv:?}, level {level}"));
            // Aligned to every size below its own: still misaligned.
            if level > 1 {
                let mut p = Paged::new(sv);
                p.map(VA, level, page_size(level) + page_size(level - 1), RWXAD);
                p.assert_page_faults(VA, &format!("{sv:?}, level {level}, half aligned"));
            }
        }
    }
}

#[test]
fn reserved_pte_bits_fault() {
    for sv in BOTH {
        for bit in 54..64 {
            // In a leaf.
            let mut p = Paged::new(sv);
            p.map(VA, 0, RAM, RWXAD | 1 << bit);
            p.assert_page_faults(VA, &format!("{sv:?}, leaf bit {bit}"));

            // In the root's pointer to the next table.
            let mut p = Paged::new(sv);
            p.map(VA, 0, RAM, RWXAD);
            let entry = ROOT + vpn(VA, sv.levels() - 1) * 8;
            let pointer = p.pte(entry);
            write64(&mut p.bus, entry, pointer | 1 << bit);
            p.assert_page_faults(VA, &format!("{sv:?}, pointer bit {bit}"));
        }
        // The PPN reaches bit 53: a page above the 48-bit physical address width
        // translates, and the access to it is the access fault (the next test).
        let mut p = Paged::new(sv);
        p.map(VA, 0, 1 << 55, RWXAD);
        assert_eq!(p.translate(VA, Access::Load), Ok(1 << 55 | VA & 0xfff), "{sv:?}");
    }
}

#[test]
fn invalid_and_reserved_encodings_fault() {
    for sv in BOTH {
        // V clear, W without R, and a pointer at level 0.
        let mut p = Paged::new(sv);
        let leaf = p
            .tables
            .map_pte(&mut p.bus, VA, 0, (RAM >> 12) << 10 | RWXAD & !V);
        p.assert_page_faults(VA, &format!("{sv:?}, not valid"));
        write64(&mut p.bus, leaf, (RAM >> 12) << 10 | W | V);
        p.assert_page_faults(VA, &format!("{sv:?}, W without R"));
        write64(&mut p.bus, leaf, (RAM >> 12) << 10 | V);
        p.assert_page_faults(VA, &format!("{sv:?}, a pointer at level 0"));
    }
}

#[test]
fn non_canonical_addresses_fault_before_the_walk() {
    for (sv, top) in [(Sv::Sv39, 38), (Sv::Sv48, 47)] {
        let low = 1u64 << top; // the top bit set, the bits above it clear
        let high = !0u64 << (top + 1); // the bits above set, the top bit clear
        for va in [low, high, low | 0x1000, 1 << 63, 1 << (top + 1)] {
            let mut p = Paged::new(sv);
            // The page the walk would reach, from the address's translated bits.
            let pte = p.map(va, 0, RAM, R | W | X);
            p.assert_page_faults(va, &format!("{sv:?}"));
            assert_eq!(p.pte(pte) & (A | D), 0, "{sv:?}, {va:#x}: no walk, no A");
        }
    }
    // An address Sv48 translates but Sv39 does not.
    let va = 1u64 << 40;
    let mut p = Paged::new(Sv::Sv48);
    p.map(va, 0, RAM, RWXAD);
    assert_eq!(p.translate(va, Access::Load), Ok(RAM));
    let mut p = Paged::new(Sv::Sv39);
    p.map(va, 0, RAM, RWXAD);
    p.assert_page_faults(va, "Sv39, bit 40");
}

#[test]
fn a_and_d_are_written_back_in_the_whole_pte() {
    for sv in BOTH {
        // A PPN with bits in the PTE's upper word, which the update must keep.
        let mut p = Paged::new(sv);
        let leaf = p.map(VA, 0, 0x1234_5678_9000, R | W);
        let pte = p.pte(leaf);
        assert_ne!(pte >> 32, 0);

        assert!(p.translate(VA, Access::Fetch).is_err(), "{sv:?}: not executable");
        assert_eq!(p.pte(leaf), pte, "{sv:?}: a fault sets nothing");
        p.translate(VA, Access::Load).unwrap();
        assert_eq!(p.pte(leaf), pte | A, "{sv:?}: a load sets A");
        p.translate(VA, Access::Store).unwrap();
        assert_eq!(p.pte(leaf), pte | A | D, "{sv:?}: a store sets D");

        // A store to a clean page sets A and D at once.
        let mut p = Paged::new(sv);
        let leaf = p.map(VA, 0, RAM, R | W);
        p.translate(VA, Access::Store).unwrap();
        assert_eq!(p.pte(leaf) & (A | D), A | D, "{sv:?}");
        // A store to a read-only page faults and sets neither.
        let mut p = Paged::new(sv);
        let leaf = p.map(VA, 0, RAM, R);
        assert_eq!(
            p.translate(VA, Access::Store),
            Err(Trap::new(Exception::StorePageFault, VA))
        );
        assert_eq!(p.pte(leaf) & (A | D), 0, "{sv:?}");
    }
}

#[test]
fn an_a_and_d_update_breaks_a_reservation_on_the_pte() {
    // Software updating a PTE with lr.d/sc.d must see its sc.d fail when the walker
    // set a bit in between (spec section 7, "Paging": A/D).
    let mut p = Paged::new(Sv::Sv48);
    let leaf = p.map(VA, 0, RAM, R | W);
    p.bus.reservation_mut(0).reserve(leaf, 8);
    p.translate(VA, Access::Load).unwrap();
    assert_eq!(
        p.bus.reservation(0).held(),
        None,
        "A set: the reservation is gone"
    );

    // One that does not change the PTE (A already set) leaves it.
    p.bus.reservation_mut(0).reserve(leaf, 8);
    p.translate(VA, Access::Load).unwrap();
    assert_eq!(p.bus.reservation(0).held(), Some(leaf));
}

#[test]
fn m_mode_and_bare_do_not_translate() {
    let mut p = Paged::new(Sv::Sv48);
    p.hart.mode = Mode::M;
    assert_eq!(p.translate(1 << 47, Access::Load), Ok(1 << 47), "M-mode");
    p.hart.mode = Mode::S;
    p.hart.csr.satp = 0;
    assert_eq!(p.translate(1 << 47, Access::Load), Ok(1 << 47), "Bare");
}

// Instructions, on a machine: fetches through the tables, sfence.vma, and an access
// above the physical address width.

const A0: u32 = 10;
const A1: u32 = 11;

/// A virtual address for the code, and one for the data.
const CODE_VA: u64 = 0x4000_0000;
const DATA_VA: u64 = 0x7f_0000_0000;
const TRAP_VECTOR: u64 = RAM + 0x4000;

const LD_A0_A1: u32 = 3 << 12 | A0 << 7 | A1 << 15 | 0x03;
const SFENCE_VMA: u32 = 0x1200_0073;

/// An RV64 machine in S-mode with Sv48 on, running `code` at `CODE_VA` (mapped to
/// `RAM`), with `a1` = `DATA_VA`.
fn machine(code: &[(u32, &str)]) -> (Machine, Tables) {
    let mut m = Machine::new(MachineConfig {
        xlen: Xlen::Rv64,
        ..MachineConfig::with_ram(RAM_SIZE)
    })
    .unwrap();
    let mut tables = Tables::new(Sv::Sv48);
    for (i, &(word, text)) in code.iter().enumerate() {
        assert_eq!(
            excalibur::decode(word, Xlen::Rv64).to_string(),
            text,
            "{word:#010x}"
        );
        m.cpu.bus.write(RAM_BASE + 4 * i as u32, 4, word).unwrap();
    }
    tables.map(&mut m.cpu.bus, CODE_VA, 0, RAM, R | X | A);
    m.cpu.csr.satp = tables.satp();
    m.cpu.csr.mtvec = TRAP_VECTOR;
    m.cpu.mode = Mode::S;
    m.cpu.pc = CODE_VA;
    m.cpu.set_reg(A1 as u8, DATA_VA);
    (m, tables)
}

#[test]
fn sfence_vma_and_a_changed_mapping() {
    let (mut m, mut tables) = machine(&[
        (LD_A0_A1, "ld a0, 0(a1)"),
        (SFENCE_VMA, "sfence.vma zero, zero"),
        (LD_A0_A1, "ld a0, 0(a1)"),
    ]);
    let (page_a, page_b) = (RAM + 0x8000, RAM + 0x9000);
    write64(&mut m.cpu.bus, page_a, 0xaaaa_aaaa_0000_0001);
    write64(&mut m.cpu.bus, page_b, 0xbbbb_bbbb_0000_0002);
    tables.map(&mut m.cpu.bus, DATA_VA, 0, page_a, R | A);
    m.cpu.step();
    assert_eq!(m.cpu.x[A0 as usize], 0xaaaa_aaaa_0000_0001);

    // The kernel remaps the page and fences: the next access uses the new mapping.
    tables.map(&mut m.cpu.bus, DATA_VA, 0, page_b, R | A);
    m.cpu.step();
    assert_eq!(m.cpu.pc, CODE_VA + 8, "sfence.vma runs in S-mode");
    m.cpu.step();
    assert_eq!(m.cpu.x[A0 as usize], 0xbbbb_bbbb_0000_0002);
    assert_eq!(m.cpu.pc, CODE_VA + 12);
}

#[test]
fn a_fetch_from_an_unmapped_or_non_canonical_pc_faults() {
    for pc in [CODE_VA + 0x1000, 1 << 47] {
        let (mut m, _) = machine(&[(LD_A0_A1, "ld a0, 0(a1)")]);
        m.cpu.pc = pc;
        m.cpu.step();
        assert_eq!(m.cpu.pc, TRAP_VECTOR);
        assert_eq!((m.cpu.csr.mcause, m.cpu.csr.mtval), (12, pc), "{pc:#x}");
    }
}

#[test]
fn a_page_above_the_physical_address_width_is_an_access_fault() {
    let (mut m, mut tables) = machine(&[(LD_A0_A1, "ld a0, 0(a1)")]);
    tables.map(&mut m.cpu.bus, DATA_VA, 0, 1 << 48, R | A);
    m.cpu.step();
    assert_eq!(m.cpu.pc, TRAP_VECTOR);
    assert_eq!((m.cpu.csr.mcause, m.cpu.csr.mtval), (5, DATA_VA));
}
