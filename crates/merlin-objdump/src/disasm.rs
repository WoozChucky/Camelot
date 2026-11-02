//! `-d`: disassembly of the executable sections, laid out like llvm-objdump:
//! a `<symbol>:` label wherever a symbol starts, one line per instruction,
//! branch targets resolved to `<symbol+offset>`, and (with `-r`) relocations
//! under the instruction they patch.

use crate::{reloc_type, reloc_value, relocation_sections, Options};
use elf32::{Elf, Error, Symbol};
use excalibur::{decode, Inst, Syntax};
use std::fmt::Write;

/// A symbol that can label code or name a branch target.
#[derive(Clone, Debug)]
struct Label {
    addr: u32,
    name: String,
    ty: u8,
}

/// llvm-objdump skips section and file symbols, mapping symbols (`$x`, `$d`)
/// and the assembler's label-difference temporaries (`.L0 `).
fn is_label(s: &Symbol) -> bool {
    !s.name.is_empty()
        && !matches!(s.ty(), elf32::STT_SECTION | elf32::STT_FILE)
        && !s.name.starts_with("$x")
        && !s.name.starts_with("$d")
        && !s.name.starts_with(".L0 ")
}

struct Labels {
    /// Indexed by section; each sorted by (address, name, type), so the last
    /// of several symbols at one address is the one shown.
    per_section: Vec<Vec<Label>>,
    /// Absolute and undefined symbols, the last resort for naming a target.
    absolute: Vec<Label>,
    /// In a `.o`, targets are only looked up in the section being disassembled.
    relocatable: bool,
    /// (address, section index) of every section, sorted by address.
    section_addrs: Vec<(u32, usize)>,
}

impl Labels {
    fn new(elf: &Elf) -> Result<Labels, Error> {
        let mut per_section = vec![Vec::new(); elf.sections.len()];
        let mut absolute = Vec::new();
        for s in elf.symbols()?.iter().filter(|s| is_label(s)) {
            let label = Label {
                addr: s.value,
                name: s.name.clone(),
                ty: s.ty(),
            };
            match s.section().and_then(|i| per_section.get_mut(i)) {
                Some(list) => list.push(label),
                None => absolute.push(label),
            }
        }
        let key = |l: &Label| (l.addr, l.name.clone(), l.ty);
        for list in &mut per_section {
            list.sort_by_key(key);
        }
        absolute.sort_by_key(key);
        let mut section_addrs: Vec<(u32, usize)> = elf
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| (s.addr, i))
            .collect();
        section_addrs.sort_by_key(|&(a, _)| a);
        Ok(Labels {
            per_section,
            absolute,
            relocatable: elf.header.ty == elf32::ET_REL,
            section_addrs,
        })
    }

    /// The symbol to name `target` by, while disassembling section `current`:
    /// the last symbol at or before it in the section that contains it (in an
    /// executable, the section with the highest start address not above it),
    /// falling back to absolute symbols.
    fn lookup(&self, current: usize, target: u32) -> Option<&Label> {
        let mut candidates: Vec<&[Label]> = Vec::new();
        if self.relocatable {
            candidates.push(&self.per_section[current]);
        } else {
            let n = self.section_addrs.partition_point(|&(a, _)| a <= target);
            let mut start = None;
            for &(addr, i) in self.section_addrs[..n].iter().rev() {
                if *start.get_or_insert(addr) != addr {
                    break;
                }
                candidates.push(&self.per_section[i]);
            }
        }
        candidates.push(&self.absolute);
        candidates.into_iter().find_map(|list| {
            let n = list.partition_point(|l| l.addr <= target);
            n.checked_sub(1).map(|i| &list[i])
        })
    }
}

/// Register values known from `auipc`, used to resolve `jalr` targets the
/// way llvm-objdump does (`auipc ra, 0x0` / `jalr ra` → `<puts>`).
struct RegState([Option<u32>; 32]);

impl RegState {
    fn new() -> RegState {
        let mut r = RegState([None; 32]);
        r.reset();
        r
    }

    fn reset(&mut self) {
        self.0 = [None; 32];
        self.0[0] = Some(0);
    }

    fn set(&mut self, rd: u8, v: Option<u32>) {
        if rd != 0 {
            self.0[rd as usize & 31] = v;
        }
    }

    /// Where a `jalr` goes, if its base register is known.
    fn jalr_target(&self, inst: &Inst) -> Option<u32> {
        match *inst {
            Inst::Jalr { rs1, imm, .. } => self.0[rs1 as usize & 31].map(|v| v.wrapping_add(imm as u32)),
            _ => None,
        }
    }

    /// Updates the state after `inst` at `pc`. Control flow and anything
    /// llvm cannot decode forget everything; other writes forget `rd`.
    fn update(&mut self, inst: &Inst, pc: u32) {
        match *inst {
            Inst::Branch { .. }
            | Inst::Jal { .. }
            | Inst::Jalr { .. }
            | Inst::Mret
            | Inst::Sret
            | Inst::Illegal(_)
            | Inst::XcalDbg { .. }
            | Inst::XcalHalt { .. } => self.reset(),
            Inst::Auipc { rd, imm } => self.set(rd, Some(pc.wrapping_add(imm))),
            Inst::Lui { rd, .. }
            | Inst::Load { rd, .. }
            | Inst::OpImm { rd, .. }
            | Inst::Op { rd, .. }
            | Inst::Mul { rd, .. }
            | Inst::Csr { rd, .. } => self.set(rd, None),
            _ => {}
        }
    }
}

/// A relocation ready to print under an instruction.
struct InlineReloc {
    /// Offset from the start of the section.
    offset: u32,
    text: String,
}

pub(crate) fn disassemble(out: &mut String, elf: &Elf, opts: &Options) -> Result<(), Error> {
    let syntax = Syntax {
        aliases: opts.aliases,
        hex: true,
    };
    let mut labels = Labels::new(elf)?;
    let reloc_sections = if opts.relocs {
        relocation_sections(elf)?
    } else {
        Vec::new()
    };

    for (idx, sec) in elf.sections.iter().enumerate() {
        if !sec.is_exec() || sec.is_nobits() || sec.size == 0 {
            continue;
        }
        let data = elf.section_data(sec)?;
        let end = sec.addr.wrapping_add(sec.size);
        let _ = writeln!(out, "\nDisassembly of section {}:", sec.name);

        // A section with no symbol at its start gets one named after it.
        let syms = &mut labels.per_section[idx];
        if syms.first().is_none_or(|s| s.addr != sec.addr) {
            let label = Label {
                addr: sec.addr,
                name: sec.name.clone(),
                ty: elf32::STT_FUNC,
            };
            syms.insert(0, label);
        }
        let syms = syms.clone();

        let mut relocs: Vec<InlineReloc> = Vec::new();
        for rs in reloc_sections.iter().filter(|rs| rs.target == idx) {
            for r in &rs.relocs {
                // In an executable, relocation offsets are addresses.
                let offset = if labels.relocatable {
                    r.offset
                } else {
                    r.offset.wrapping_sub(sec.addr)
                };
                let text = format!(
                    "\t\t\t{:08x}:  {}\t{}",
                    sec.addr.wrapping_add(offset),
                    reloc_type(r.ty),
                    reloc_value(elf, &rs.symbols, r)
                );
                relocs.push(InlineReloc { offset, text });
            }
        }
        relocs.sort_by_key(|r| r.offset);
        let mut relocs = relocs.into_iter().peekable();

        // One region per distinct symbol address, labelled with the last symbol there.
        let mut i = 0;
        while i < syms.len() {
            let start = syms[i].addr;
            let mut j = i;
            while j < syms.len() && syms[j].addr == start {
                j += 1;
            }
            let label = &syms[j - 1];
            let region_end = syms.get(j).map_or(end, |s| s.addr.min(end));
            i = j;
            if start < sec.addr || start >= region_end {
                continue;
            }
            let _ = writeln!(out, "\n{start:08x} <{}>:", label.name);

            let mut regs = RegState::new();
            let mut pc = start;
            while pc < region_end {
                let off = (pc - sec.addr) as usize;
                let size = if region_end - pc >= 4 { 4 } else { 1 };
                let (raw, inst) = match data.get(off..off + size) {
                    Some(&[a, b, c, d]) => {
                        let w = u32::from_le_bytes([a, b, c, d]);
                        (format!("{w:08x}"), decode(w))
                    }
                    Some(&[b]) => (format!("{b:02x}"), Inst::Illegal(b as u32)),
                    _ => break,
                };
                let d = inst.format(Some(pc), syntax);
                let (mnemonic, operands) = match inst {
                    Inst::Illegal(b) if size == 1 => (".byte", format!("{b:#04x}")),
                    _ => (d.mnemonic, d.operands),
                };
                let _ = write!(out, "{pc:8x}:");
                if opts.raw_insn {
                    let _ = write!(out, " {raw:<13}");
                } else {
                    out.push_str("      ");
                }
                let _ = write!(out, "\t{mnemonic}");
                if !operands.is_empty() {
                    let _ = write!(out, "\t{operands}");
                }
                if let Some(l) = d
                    .target
                    .or_else(|| regs.jalr_target(&inst))
                    .and_then(|t| labels.lookup(idx, t).map(|l| (l, t - l.addr)))
                {
                    match l {
                        (l, 0) => {
                            let _ = write!(out, " <{}>", l.name);
                        }
                        (l, disp) => {
                            let _ = write!(out, " <{}+{disp:#x}>", l.name);
                        }
                    }
                }
                out.push('\n');
                regs.update(&inst, pc);

                let next = (off + size) as u32;
                while let Some(r) = relocs.next_if(|r| r.offset < next) {
                    let _ = writeln!(out, "{}", r.text);
                }
                pc += size as u32;
            }
        }
    }
    Ok(())
}
