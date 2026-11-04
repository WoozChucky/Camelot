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
use std::collections::HashSet;

/// The [`Context`] of a link: every `R_RISCV_PCREL_HI20` site, keyed by its
/// address, for the `PCREL_LO12` relocations that point at them. Also the
/// addresses of the sites whose symbol has no value (undefined, or in a
/// section that is not loaded): that is reported once, at the `PCREL_HI20`,
/// so the `PCREL_LO12` paired with it says nothing more.
pub fn context(layout: &Layout) -> (Context, HashSet<u64>) {
    let mut ctx = Context::new(layout.class);
    let mut failed = HashSet::new();
    for os in &layout.sections {
        for i in &os.inputs {
            for r in &layout.objects[i.object].sections[i.section].relocs {
                if r.ty == reloc::R_RISCV_PCREL_HI20 {
                    let place = i.addr.wrapping_add(r.offset);
                    match layout.reloc_symbol(i.object, r.sym as usize) {
                        Ok(sym) => {
                            ctx.pcrel_hi20.insert(
                                place,
                                HiSite {
                                    sym,
                                    addend: r.addend,
                                },
                            );
                        }
                        Err(_) => {
                            failed.insert(place);
                        }
                    }
                }
            }
        }
    }
    (ctx, failed)
}

/// The contents of the executable's sections, relocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Images {
    /// One per [`Layout::sections`] (empty for NOBITS sections).
    pub sections: Vec<Vec<u8>>,
    /// One per [`Layout::debug`].
    pub debug: Vec<Vec<u8>>,
}

/// Returns one byte image per output section (empty for NOBITS sections),
/// with input contents copied in and relocations applied, and the same for
/// the debug sections.
pub fn relocate(layout: &Layout) -> Result<Images, Vec<Error>> {
    let mut errors = Vec::new();
    let sections = match loaded(layout) {
        Ok(s) => s,
        Err(mut e) => {
            errors.append(&mut e);
            Vec::new()
        }
    };
    let debug = debug(layout, &mut errors);
    if errors.is_empty() {
        Ok(Images { sections, debug })
    } else {
        Err(errors)
    }
}

/// The debug sections: their inputs one after another, relocated. A
/// relocation against a symbol that is not in the executable (in a section
/// `/DISCARD/` threw away or one not loaded, or a weak reference nobody
/// defines, see [`Layout::in_dropped_section`]) writes ld.lld's tombstone:
/// the debug information of code that is gone points nowhere. That is 0,
/// except in `.debug_loc` and `.debug_ranges`, where it is 1: there a pair of
/// zeros ends a DWARF 4 location or range list (and -1, the other choice,
/// selects a base address), so ld.lld writes 1, as GNU ld does in
/// `.debug_ranges`.
fn debug(layout: &Layout, errors: &mut Vec<Error>) -> Vec<Vec<u8>> {
    let ctx = Context::debug(layout.class);
    let mut images = Vec::new();
    for ds in &layout.debug {
        let tombstone: u64 = if ds.name == ".debug_loc" || ds.name == ".debug_ranges" {
            1
        } else {
            0
        };
        let mut img = vec![0; ds.size as usize];
        for i in &ds.inputs {
            let obj = &layout.objects[i.object];
            let sec = &obj.sections[i.section];
            let start = i.addr as usize;
            img[start..start + sec.data.len()].copy_from_slice(&sec.data);
            for r in &sec.relocs {
                let ty = reloc::name(r.ty);
                let at = format!("{}+{:#x}", sec.name, r.offset);
                if r.ty == reloc::R_RISCV_NONE || r.ty == reloc::R_RISCV_RELAX {
                    continue;
                }
                if r.offset >= sec.size {
                    errors.push(Error(format!(
                        "{}: {ty} at {at} is outside section {} (size {:#x})",
                        obj.name, sec.name, sec.size
                    )));
                    continue;
                }
                // S = tombstone - A, modulo 2^64, so that S + A, the value
                // written, is the tombstone for R_RISCV_32 (an offset) and
                // R_RISCV_64 (an address) alike, whatever the addend. Any
                // other type (an ADD/SUB half of a label difference) gets
                // S = 0, as ld.lld gives a symbol without a section.
                let sym = if layout.in_dropped_section(i.object, r.sym as usize) {
                    Ok(if r.ty == reloc::R_RISCV_32 || r.ty == reloc::R_RISCV_64 {
                        tombstone.wrapping_sub(r.addend as u64)
                    } else {
                        0
                    })
                } else {
                    layout.reloc_symbol(i.object, r.sym as usize)
                };
                let sym = match sym {
                    Ok(s) => s,
                    Err(e) => {
                        errors.push(Error(format!("{}: {ty} at {at} {e}", obj.name)));
                        continue;
                    }
                };
                let place = i.addr.wrapping_add(r.offset);
                let bytes = &mut img[place as usize..start + sec.data.len()];
                if let Err(e) = reloc::apply(r.ty, place, sym, r.addend, bytes, &ctx) {
                    report(errors, obj, e, sec, r);
                }
            }
        }
        images.push(img);
    }
    images
}

/// [`relocate`] for the loaded sections.
fn loaded(layout: &Layout) -> Result<Vec<Vec<u8>>, Vec<Error>> {
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

    let (ctx, failed_hi20) = context(layout);

    let mut errors = Vec::new();
    for (oi, os) in layout.sections.iter().enumerate() {
        for i in &os.inputs {
            let obj = &layout.objects[i.object];
            let sec = &obj.sections[i.section];
            for r in &sec.relocs {
                let ty = reloc::name(r.ty);
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
                if matches!(r.ty, reloc::R_RISCV_PCREL_LO12_I | reloc::R_RISCV_PCREL_LO12_S)
                    && failed_hi20.contains(&sym)
                {
                    // Its PCREL_HI20 already reported the symbol.
                    continue;
                }
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
                    report(&mut errors, obj, e, sec, r);
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

/// Reports a relocation [`reloc::apply`] refused in `obj`: one error, or
/// two for an offset out of range and odd, as ld.lld reports it.
fn report(errors: &mut Vec<Error>, obj: &Object, e: RelocError, sec: &Section, r: &Rela) {
    let ty = reloc::name(r.ty);
    let at = format!("{}+{:#x}", sec.name, r.offset);
    let sym = obj.symbol_display_name(r.sym);
    let mut push = |e| {
        errors.push(Error(format!(
            "{}: {}",
            obj.name,
            describe(e, &ty, sym, &at, sec, r)
        )))
    };
    if let RelocError::OutOfRangeMisaligned { value, align, .. } = e {
        push(e);
        push(RelocError::Misaligned { value, align });
    } else {
        push(e);
    }
}

/// The message for a relocation [`reloc::apply`] refused. `at` is
/// `section+offset`; the caller prefixes the file name.
fn describe(e: RelocError, ty: &str, sym: &str, at: &str, sec: &Section, r: &Rela) -> String {
    match e {
        RelocError::Unsupported => format!("unsupported relocation {ty} against '{sym}' at {at}"),
        RelocError::AlignPadding => format!(
            "{ty} at {at}: merlin-ld does not delete alignment padding as ld.lld does; \
             assemble with -mno-relax"
        ),
        // Two errors, the range first: [`report`] gives both.
        RelocError::OutOfRangeMisaligned { value, min, max, .. } => {
            describe(RelocError::OutOfRange { value, min, max }, ty, sym, at, sec, r)
        }
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
