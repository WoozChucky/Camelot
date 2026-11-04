//! The `--Map` file: where every output section, input section and named
//! symbol ended up.
//!
//! This is merlin-ld's own format, not ld.lld's. Addresses and sizes are
//! printed as wide as the executable's: 8 hex digits for ELF32, 16 for ELF64
//! (`0x80000000`, `0x0000000080000000`). A section loaded somewhere else
//! than it runs (`AT(lma)`) says where: `(load 0x...)`.

use crate::layout::{Def, Layout};
use crate::object::{Class, STT_FILE, STT_SECTION};
use std::fmt::Write;

/// Symbols worth listing: skips section/file symbols, assembler temporaries
/// (`.L...`) and RISC-V mapping symbols (`$x`, `$d`).
fn listed(name: &str, ty: u8) -> bool {
    !(name.is_empty()
        || name.starts_with(".L")
        || name.starts_with('$')
        || ty == STT_SECTION
        || ty == STT_FILE)
}

pub fn map(layout: &Layout) -> String {
    // `0x` and 8 or 16 digits: the width of an address in the executable.
    let width = match layout.class {
        Class::Elf32 => 10,
        Class::Elf64 => 18,
    };
    let hex = |v: u64| format!("{v:#0width$x}");
    let mut m = String::new();
    let _ = writeln!(m, "merlin-ld link map\n");
    match &layout.entry_symbol {
        Some(s) => {
            let _ = writeln!(m, "Entry point: {} ({s})\n", hex(layout.entry));
        }
        None => {
            let _ = writeln!(m, "Entry point: {}\n", hex(layout.entry));
        }
    }
    let _ = writeln!(
        m,
        "{:<20} {:<width$} {:<width$} Align",
        "Section", "Address", "Size"
    );
    for os in &layout.sections {
        let load = if os.lma != os.addr {
            format!("  (load {})", hex(os.lma))
        } else {
            String::new()
        };
        let _ = writeln!(
            m,
            "{:<20} {} {} {}{}{load}",
            os.name,
            hex(os.addr),
            hex(os.size),
            os.align,
            if os.nobits { "  (nobits)" } else { "" }
        );
        for i in &os.inputs {
            let obj = &layout.objects[i.object];
            let sec = &obj.sections[i.section];
            let _ = writeln!(
                m,
                "    {} {} {}({})",
                hex(i.addr),
                hex(sec.size),
                obj.name,
                sec.name
            );
            let mut syms: Vec<(u64, &str)> = obj
                .symbols
                .iter()
                .enumerate()
                .filter(|(_, s)| s.shndx as usize == i.section && listed(&s.name, s.ty))
                .filter_map(|(si, s)| Some((layout.object_symbol(i.object, si)?, s.name.as_str())))
                .collect();
            syms.sort();
            for (addr, name) in syms {
                let _ = writeln!(m, "        {} {name}", hex(addr));
            }
        }
    }
    let script: Vec<_> = layout
        .globals
        .iter()
        .filter_map(|(n, d)| match d {
            Def::Script(v) => Some((*v, n)),
            Def::Object { .. } => None,
        })
        .collect();
    if !script.is_empty() {
        let _ = writeln!(m, "\nLinker script symbols");
        for (v, n) in script {
            let _ = writeln!(m, "    {} {n}", hex(v));
        }
    }
    m
}
