//! Builds the output section images and runs every relocation through
//! [`crate::reloc::apply`].
//!
//! The checks that need the object file (is the place inside its section, is
//! the symbol defined, is a `PCREL_LO12`'s auipc label in the same section)
//! happen here; the arithmetic and the range checks are in [`crate::reloc`].
//! Every problem is collected, so one run reports them all.

use crate::layout::Layout;
use crate::object::{Object, Rela, Section, SHN_LORESERVE, SHN_UNDEF};
use crate::reloc::{self, Context, HiSite, RelocError};
use crate::Error;

/// The [`Context`] of a link: every `R_RISCV_PCREL_HI20` site, keyed by its
/// address, for the `PCREL_LO12` relocations that point at them.
pub fn context(layout: &Layout) -> Context {
    let mut ctx = Context::default();
    for os in &layout.sections {
        for i in &os.inputs {
            for r in &layout.objects[i.object].sections[i.section].relocs {
                if r.ty == reloc::R_RISCV_PCREL_HI20 {
                    if let Ok(sym) = layout.reloc_symbol(i.object, r.sym as usize) {
                        ctx.pcrel_hi20.insert(
                            i.addr.wrapping_add(r.offset),
                            HiSite {
                                sym,
                                addend: r.addend,
                            },
                        );
                    }
                }
            }
        }
    }
    ctx
}

/// Returns one byte image per output section (empty for NOBITS sections),
/// with input contents copied in and relocations applied.
pub fn relocate(layout: &Layout) -> Result<Vec<Vec<u8>>, Vec<Error>> {
    let mut images: Vec<Vec<u8>> = layout
        .sections
        .iter()
        .map(|s| {
            if s.nobits {
                Vec::new()
            } else {
                vec![0; s.size as usize]
            }
        })
        .collect();
    for (img, os) in images.iter_mut().zip(&layout.sections) {
        if os.nobits {
            continue;
        }
        for i in &os.inputs {
            let sec = &layout.objects[i.object].sections[i.section];
            let off = (i.addr - os.addr) as usize;
            img[off..off + sec.data.len()].copy_from_slice(&sec.data);
        }
    }

    let ctx = context(layout);

    let mut errors = Vec::new();
    for (oi, os) in layout.sections.iter().enumerate() {
        for i in &os.inputs {
            let obj = &layout.objects[i.object];
            let sec = &obj.sections[i.section];
            for r in &sec.relocs {
                let ty = reloc::name(r.ty);
                let sym_name = obj.symbol_display_name(r.sym);
                let at = format!("{}+{:#x}", sec.name, r.offset);
                if r.ty == reloc::R_RISCV_NONE || r.ty == reloc::R_RISCV_RELAX {
                    // Hints that patch nothing: R_RISCV_RELAX marks the
                    // relocation before it as relaxable, and merlin-ld never
                    // relaxes.
                    continue;
                }
                if sec.is_nobits() || os.nobits {
                    errors.push(Error(format!(
                        "{}: {ty} at {at} patches a NOBITS section",
                        obj.name
                    )));
                    continue;
                }
                if r.offset >= sec.size {
                    errors.push(Error(format!(
                        "{}: {ty} at {at} is outside section {} (size {:#x})",
                        obj.name, sec.name, sec.size
                    )));
                    continue;
                }
                if let Some(e) = pcrel_lo12_section_error(obj, i.section, r) {
                    errors.push(Error(format!("{}: {ty} at {at} {e}", obj.name)));
                    continue;
                }
                let sym = match layout.reloc_symbol(i.object, r.sym as usize) {
                    Ok(s) => s,
                    Err(e) => {
                        errors.push(Error(format!("{}: {ty} at {at} {e}", obj.name)));
                        continue;
                    }
                };
                let place = i.addr.wrapping_add(r.offset);
                let sym = if is_jump(r.ty) && layout.is_undefined_weak(i.object, r.sym as usize) {
                    // A jump to a weak function nobody defined (code guarded
                    // by `if (f) f();`): address 0 is usually out of a
                    // branch's range, so, like ld.lld, aim it at the jump
                    // itself (an infinite loop, should it ever run).
                    place
                } else {
                    sym
                };
                // From the place to the end of *this input section*: a
                // relocation never patches the bytes of the next one.
                let start = (place - os.addr) as usize;
                let end = (i.addr - os.addr) as usize + sec.data.len();
                let bytes = &mut images[oi][start..end];
                if let Err(e) = reloc::apply(r.ty, place, sym, r.addend, bytes, &ctx) {
                    errors.push(Error(format!(
                        "{}: {}",
                        obj.name,
                        describe(e, &ty, sym_name, &at, sec, r)
                    )));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(images)
    } else {
        Err(errors)
    }
}

/// Relocations of branches, jumps and calls: S + A - P is where control goes.
fn is_jump(ty: u32) -> bool {
    matches!(
        ty,
        reloc::R_RISCV_BRANCH | reloc::R_RISCV_JAL | reloc::R_RISCV_CALL | reloc::R_RISCV_CALL_PLT
    )
}

/// The message for a relocation [`reloc::apply`] refused. `at` is
/// `section+offset`; the caller prefixes the file name.
fn describe(e: RelocError, ty: &str, sym: &str, at: &str, sec: &Section, r: &Rela) -> String {
    match e {
        RelocError::Unsupported => format!("unsupported relocation {ty} against '{sym}' at {at}"),
        RelocError::NeedsRelaxation => format!(
            "{ty} at {at} needs linker relaxation, which merlin-ld does not do; assemble with -mno-relax"
        ),
        RelocError::OutOfRange { value, min, max } => format!(
            "relocation {ty} out of range for '{sym}' at {at}: {value} is not in [{min}, {max}]"
        ),
        RelocError::Misaligned { value, align } => format!(
            "relocation {ty} against '{sym}' at {at} is misaligned: offset {value} is not a multiple of {align}"
        ),
        RelocError::Truncated { need, have } => format!(
            "relocation {ty} against '{sym}' at {at} patches {need} bytes, but section {} ends {have} bytes \
             after it",
            sec.name
        ),
        RelocError::MissingPcrelHi20 => format!(
            "{ty} at {at} refers to '{sym}', which is not the place of an R_RISCV_PCREL_HI20"
        ),
        RelocError::PcrelLo12Addend => format!(
            "{ty} against '{sym}' at {at} has addend {}; a %pcrel_lo takes the offset of its \
             R_RISCV_PCREL_HI20 and must not have its own",
            r.addend
        ),
    }
}

/// The psABI requires a `PCREL_LO12_*`'s symbol, the label on its `auipc`, to
/// be in the same section as the relocation: the pair is one address
/// computation split across two instructions. A label anywhere else is a
/// mistake; say so rather than look up an unrelated `PCREL_HI20`.
fn pcrel_lo12_section_error(obj: &Object, section: usize, r: &Rela) -> Option<String> {
    if r.ty != reloc::R_RISCV_PCREL_LO12_I && r.ty != reloc::R_RISCV_PCREL_LO12_S {
        return None;
    }
    let s = &obj.symbols[r.sym as usize];
    if s.shndx as usize == section {
        return None;
    }
    let shown = obj.symbol_display_name(r.sym);
    Some(match obj.sections.get(s.shndx as usize) {
        Some(other) if s.shndx != SHN_UNDEF && s.shndx < SHN_LORESERVE => format!(
            "refers to '{shown}' in section {}, not to the label of an auipc in {}",
            other.name, obj.sections[section].name
        ),
        _ => format!(
            "refers to '{shown}', not to the label of an auipc in {}",
            obj.sections[section].name
        ),
    })
}
