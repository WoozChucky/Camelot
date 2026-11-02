//! The `--Map` file: where every output section, input section and named
//! symbol ended up.

use crate::layout::{Def, Layout};
use crate::object::{STT_FILE, STT_SECTION};
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
    let mut m = String::new();
    let _ = writeln!(m, "merlin-ld link map\n");
    match &layout.entry_symbol {
        Some(s) => {
            let _ = writeln!(m, "Entry point: {:#010x} ({s})\n", layout.entry);
        }
        None => {
            let _ = writeln!(m, "Entry point: {:#010x}\n", layout.entry);
        }
    }
    let _ = writeln!(m, "{:<20} {:<10} {:<10} Align", "Section", "Address", "Size");
    for os in &layout.sections {
        let _ = writeln!(
            m,
            "{:<20} {:#010x} {:#010x} {}{}",
            os.name,
            os.addr,
            os.size,
            os.align,
            if os.nobits { "  (nobits)" } else { "" }
        );
        for i in &os.inputs {
            let obj = &layout.objects[i.object];
            let sec = &obj.sections[i.section];
            let _ = writeln!(
                m,
                "    {:#010x} {:#010x} {}({})",
                i.addr, sec.size, obj.name, sec.name
            );
            let mut syms: Vec<(u32, &str)> = obj
                .symbols
                .iter()
                .enumerate()
                .filter(|(_, s)| s.shndx as usize == i.section && listed(&s.name, s.ty))
                .filter_map(|(si, s)| Some((layout.object_symbol(i.object, si)?, s.name.as_str())))
                .collect();
            syms.sort();
            for (addr, name) in syms {
                let _ = writeln!(m, "        {addr:#010x} {name}");
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
            let _ = writeln!(m, "    {v:#010x} {n}");
        }
    }
    m
}
