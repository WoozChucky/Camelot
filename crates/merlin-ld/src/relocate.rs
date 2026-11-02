//! Builds the output section images and runs every relocation through
//! [`crate::reloc::apply`].

use crate::layout::Layout;
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
                let sym = match layout.reloc_symbol(i.object, r.sym as usize) {
                    Ok(s) => s,
                    Err(e) => {
                        errors.push(Error(format!("{}: {ty} at {at} {e}", obj.name)));
                        continue;
                    }
                };
                let place = i.addr.wrapping_add(r.offset);
                let off = (place - os.addr) as usize;
                if let Err(e) = reloc::apply(r.ty, place, sym, r.addend, &mut images[oi][off..], &ctx) {
                    let msg = match e {
                        RelocError::NotImplemented => format!(
                            "cannot apply {ty} against '{sym_name}' at {at}: relocation processing is not \
                             implemented yet (issue #7)"
                        ),
                        RelocError::Unsupported => {
                            format!("unsupported relocation {ty} against '{sym_name}' at {at}")
                        }
                        RelocError::OutOfRange => format!("relocation {ty} out of range for '{sym_name}'"),
                        RelocError::MissingPcrelHi20 => format!(
                            "{ty} at {at} refers to '{sym_name}', which is not the place of an R_RISCV_PCREL_HI20"
                        ),
                        RelocError::Other(m) => format!("relocation {ty} against '{sym_name}' at {at}: {m}"),
                    };
                    errors.push(Error(format!("{}: {msg}", obj.name)));
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
