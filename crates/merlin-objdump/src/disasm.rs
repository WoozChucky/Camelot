//! `-d`: disassembly of the executable sections, laid out like llvm-objdump:
//! a `<symbol>:` label wherever a symbol starts, one line per instruction,
//! branch targets resolved to `<symbol+offset>`, and (with `-r`) relocations
//! under the instruction they patch.

use crate::{reloc_type, reloc_value, relocation_sections, Options};
use elf::{Elf, Error, Symbol};
use excalibur::{decode, Inst, Syntax, Xlen};
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
        && !matches!(s.ty(), elf::STT_SECTION | elf::STT_FILE)
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
            relocatable: elf.header.ty == elf::ET_REL,
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

    /// Updates the state after `inst` (decoded from `w`) at `pc`. Control
    /// flow and anything llvm cannot decode forget everything; other writes
    /// forget `rd`.
    fn update(&mut self, inst: &Inst, w: u32, pc: u32) {
        if llvm_rejects_fence(w) {
            self.reset();
            return;
        }
        match *inst {
            Inst::Branch { .. }
            | Inst::Jal { .. }
            | Inst::Jalr { .. }
            | Inst::Mret
            | Inst::Sret
            | Inst::Illegal(_)
            | Inst::XcalDbg { .. }
            | Inst::XcalHalt { .. } => self.reset(),
            Inst::Auipc { rd, imm } => self.set(rd, Some(pc.wrapping_add(imm as u32))),
            Inst::Lui { rd, .. }
            | Inst::Load { rd, .. }
            | Inst::OpImm { rd, .. }
            | Inst::OpImm32 { rd, .. }
            | Inst::Op { rd, .. }
            | Inst::Op32 { rd, .. }
            | Inst::Mul { rd, .. }
            | Inst::Mul32 { rd, .. }
            | Inst::Lr { rd, .. }
            | Inst::Sc { rd, .. }
            | Inst::Amo { rd, .. }
            | Inst::Csr { rd, .. } => self.set(rd, None),
            _ => {}
        }
    }
}

/// True for the reserved MISC-MEM encodings that excalibur decodes as a
/// fence (Grail executes them as one) but llvm-objdump prints as `<unknown>`:
/// `fence` with non-zero fm, rs1 or rd (other than `fence.tso`), and
/// `fence.i` with any immediate, rs1 or rd bits set.
fn llvm_rejects_fence(w: u32) -> bool {
    const RS1_RD: u32 = 0x000f_8f80;
    if w & 0x7f != 0x0f {
        return false;
    }
    match (w >> 12) & 7 {
        0 => w & RS1_RD != 0 || (w >> 28 != 0 && w >> 20 != 0x833),
        1 => w & (0xfff0_0000 | RS1_RD) != 0,
        _ => false,
    }
}

/// A relocation ready to print under an instruction.
struct InlineReloc {
    /// Offset from the start of the section.
    offset: u32,
    text: String,
}

/// How many leading zero bytes of `bytes` llvm-objdump folds into `...`: none
/// unless there are at least 8, and then a multiple of 4, so an instruction
/// starting with a zero byte is not cut.
fn skippable_zeros(bytes: &[u8]) -> usize {
    let n = bytes.iter().take_while(|&&b| b == 0).count();
    if n < 8 {
        0
    } else {
        n & !3
    }
}

/// A line table sequence for `-l`: the section it is in (in an object), its
/// start and end, and its rows (address, path, line).
type Sequence = (Option<usize>, u32, u32, Vec<(u32, String, u32)>);

/// `-l`: where each address comes from, as llvm-objdump prints it. Before an
/// instruction, `; name():` when the function it is in is another than the
/// last one's, and `; file:line` when its line (the line table row at or
/// before it) is another, or the function is. The function is the symbol
/// LLVM's symbolizer takes: the last function, object or untyped symbol at or
/// before the address in its section (not a mapping symbol like `$x` or a
/// fake label `.L0 `), if the
/// address is inside it (a symbol of size 0 runs to the next one). Unlike
/// llvm-objdump, the line table is read without a compile unit.
struct SourceLines {
    sequences: Vec<Sequence>,
    /// The symbols a function name may come from, sorted: (section, address,
    /// size, name).
    functions: Vec<(usize, u32, u32, String)>,
    relocatable: bool,
    /// What the last instruction's lines said: its function and its line.
    last: (Option<String>, Option<(String, u32)>),
}

impl SourceLines {
    fn new(elf: &Elf) -> Result<SourceLines, Error> {
        let mut sequences = Vec::new();
        for t in elf.line_tables()? {
            for seq in &t.sequences {
                let (Some(first), Some(end)) = (seq.rows.first(), seq.rows.last()) else {
                    continue;
                };
                let rows = seq
                    .rows
                    .iter()
                    .filter(|r| !r.end_sequence)
                    .map(|r| (r.addr, t.path(r.file).to_string(), r.line))
                    .collect();
                sequences.push((seq.section, first.addr, end.addr, rows));
            }
        }
        let mut functions: Vec<(usize, u32, u32, String)> = elf
            .symbols()?
            .into_iter()
            .filter(|s| matches!(s.ty(), elf::STT_NOTYPE | elf::STT_FUNC | elf::STT_OBJECT))
            // Not RISC-V's mapping symbols (`$x`, `$d`) and fake labels (`.L0 `).
            .filter(|s| !s.name.is_empty() && !s.name.starts_with('$') && !s.name.starts_with(".L0 "))
            .filter_map(|s| {
                let sec = s.section()?;
                elf.sections
                    .get(sec)?
                    .is_alloc()
                    .then_some((sec, s.value, s.size, s.name))
            })
            .collect();
        // By address, then size: of several at one address the biggest wins.
        functions.sort_by_key(|f| (f.0, f.1, f.2));
        Ok(SourceLines {
            sequences,
            functions,
            relocatable: elf.header.ty == elf::ET_REL,
            last: (None, None),
        })
    }

    /// The line table row for `addr` in section `sec`: path and line.
    fn row(&self, sec: usize, addr: u32) -> Option<(String, u32)> {
        let (_, _, _, rows) = self.sequences.iter().find(|(s, start, end, _)| {
            (!self.relocatable || *s == Some(sec)) && (*start..*end).contains(&addr)
        })?;
        let i = rows.partition_point(|r| r.0 <= addr).checked_sub(1)?;
        Some((rows[i].1.clone(), rows[i].2))
    }

    fn print(&mut self, out: &mut String, sec: usize, addr: u32) {
        let i = self.functions.partition_point(|f| (f.0, f.1) <= (sec, addr));
        let function = i
            .checked_sub(1)
            .map(|i| &self.functions[i])
            .filter(|(s, start, size, _)| *s == sec && (*size == 0 || addr - start < *size))
            .map(|f| f.3.clone());
        let line = self.row(sec, addr).filter(|(_, l)| *l != 0);
        let new_function = function.is_some() && function != self.last.0;
        if let (true, Some(f)) = (new_function, &function) {
            let _ = writeln!(out, "; {f}():");
        }
        if let Some((path, n)) = &line {
            if new_function || self.last.1.as_ref() != Some(&(path.clone(), *n)) {
                let _ = writeln!(out, "; {path}:{n}");
            }
        }
        self.last = (function, line);
    }
}

pub(crate) fn disassemble(out: &mut String, elf: &Elf, opts: &Options) -> Result<(), Error> {
    // RV32 until #150 reads ELF64 and RV64.
    let syntax = Syntax {
        aliases: opts.aliases,
        hex: true,
        xlen: Xlen::Rv32,
    };
    let mut source = if opts.line_numbers {
        Some(SourceLines::new(elf)?)
    } else {
        None
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
        // Addresses are u64 here so a section ending at 2^32 does not wrap.
        let sec_start = sec.addr as u64;
        let end = sec_start + sec.size as u64;
        let _ = writeln!(out, "\nDisassembly of section {}:", sec.name);

        // A section with no symbol at its start gets one named after it. A
        // linker script can define symbols below the section start, so the
        // label goes where it keeps the list sorted, not necessarily first.
        let syms = &mut labels.per_section[idx];
        if !syms.iter().any(|s| s.addr == sec.addr) {
            let label = Label {
                addr: sec.addr,
                name: sec.name.clone(),
                ty: elf::STT_FUNC,
            };
            let at = syms.partition_point(|s| s.addr < sec.addr);
            syms.insert(at, label);
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
            let start = syms[i].addr as u64;
            let mut j = i;
            while j < syms.len() && syms[j].addr as u64 == start {
                j += 1;
            }
            let label = &syms[j - 1];
            let region_end = syms.get(j).map_or(end, |s| (s.addr as u64).min(end));
            i = j;
            if start < sec_start || start >= region_end {
                continue;
            }
            let _ = writeln!(out, "\n{start:08x} <{}>:", label.name);

            let mut regs = RegState::new();
            let mut pc = start;
            while pc < region_end {
                let off = (pc - sec_start) as usize;
                // Like llvm-objdump (without -z), a run of zero bytes inside
                // this region and before the next shown relocation is
                // printed as `...` instead of one line per word.
                let limit = relocs
                    .peek()
                    .map_or(region_end, |r| region_end.min(sec_start + r.offset as u64));
                let span = data.get(off..(limit.max(pc) - sec_start) as usize).unwrap_or(&[]);
                let skip = skippable_zeros(span);
                if skip > 0 {
                    out.push_str("\t\t...\n");
                    pc += skip as u64;
                    continue;
                }
                // Like llvm-objdump, a word is decoded whenever the section
                // has 4 bytes left, even if it runs past the next label.
                let size = if end - pc >= 4 { 4 } else { 1 };
                let (raw, w) = match data.get(off..off + size) {
                    Some(&[a, b, c, d]) => {
                        let w = u32::from_le_bytes([a, b, c, d]);
                        (format!("{w:08x}"), w)
                    }
                    Some(&[b]) => (format!("{b:02x}"), b as u32),
                    _ => break,
                };
                let inst = if size == 4 {
                    decode(w, Xlen::Rv32)
                } else {
                    Inst::Illegal(w)
                };
                let pc32 = pc as u32;
                if let Some(s) = source.as_mut() {
                    s.print(out, idx, pc32);
                }
                let d = inst.format(Some(u64::from(pc32)), syntax);
                let (mnemonic, operands) = match size {
                    1 => (".byte", format!("{w:#04x}")),
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
                    .map(|t| t as u32)
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
                regs.update(&inst, w, pc32);

                let next = (off + size) as u32;
                while let Some(r) = relocs.next_if(|r| r.offset < next) {
                    let _ = writeln!(out, "{}", r.text);
                }
                pc += size as u64;
            }
        }
    }
    Ok(())
}
