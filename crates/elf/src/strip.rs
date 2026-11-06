//! Stripping a relocatable object of its debug information, as `strip
//! --strip-debug` (`llvm-strip -g`) does, byte for byte as llvm-strip
//! writes the result for the objects merlin-as writes. It is how a C
//! library is installed (Debian's `dh_strip --strip-debug` on static
//! libraries): `cargo xtask fs` puts the stripped `crt0.o` and `libc.a` on
//! the root image, while the host keeps the objects with their line
//! tables for merlin-dbg. A program linked from either is the same once
//! its own line tables are left out.
//!
//! What goes:
//!
//! - every debug section (`.debug_*`, `.zdebug_*`, `.gdb_index`), and the
//!   relocation sections that apply to one;
//! - the symbols defined in a section that goes (the debug sections'
//!   section symbols and their mapping symbols, `$d`), and the `STT_FILE`
//!   symbols, which strip counts as debugging information too. A symbol a
//!   kept relocation refers to cannot go: that is an error.
//!
//! The rest is kept, and laid out as llvm-strip lays it out:
//!
//! - the sections keep their order and their indices close up; their
//!   contents are placed in the order of their old offsets, each at its
//!   alignment, from the end of the ELF header, a `NOBITS` section taking
//!   no room, and the section headers follow, 8-byte aligned;
//! - the symbol table keeps its order, its `sh_info` the first non-local;
//!   relocations name the symbols' new indices;
//! - the two string tables are written anew, as LLVM's string table
//!   builder writes them: each name once, sorted by its bytes read from the
//!   end, the greatest first, and a name that ends the one before it shares
//!   its bytes (`setenv` inside `unsetenv`). An empty name is offset 0, the
//!   table's first byte, a NUL.
//!
//! Only ELF64 relocatable objects; section groups, extended section
//! indices and LLVM's address-significance tables, which merlin-as never
//! writes, are refused.

use crate::{Class, Elf, Error, Section, SHN_LORESERVE, SHT_GROUP, SHT_NOBITS, SHT_REL, SHT_RELA};
use crate::{ET_REL, SHT_SYMTAB, SHT_SYMTAB_SHNDX, STB_LOCAL, STT_FILE};
use std::collections::HashMap;

/// LLVM's address-significance table (`.llvm_addrsig`): symbol indices.
const SHT_LLVM_ADDRSIG: u32 = 0x6fff_4c03;

/// Whether strip counts a section as debug information.
pub fn is_debug_section(name: &str) -> bool {
    name.starts_with(".debug") || name.starts_with(".zdebug") || name == ".gdb_index"
}

fn bad<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

/// A string table of `names` as LLVM's builder writes one ([the module's
/// description](self)), and each name's offset in it.
pub fn string_table<'a>(names: impl IntoIterator<Item = &'a str>) -> (Vec<u8>, HashMap<&'a str, u32>) {
    let mut unique: Vec<&str> = names.into_iter().filter(|n| !n.is_empty()).collect();
    unique.sort_unstable();
    unique.dedup();
    // The greatest first, comparing the bytes from the end: a name that
    // ends another comes right after it.
    unique.sort_by(|a, b| b.bytes().rev().cmp(a.bytes().rev()));
    let mut table = vec![0u8];
    let mut at = HashMap::new();
    at.insert("", 0);
    let mut previous = "";
    for name in unique {
        if previous.ends_with(name) {
            at.insert(name, (table.len() - name.len() - 1) as u32);
            continue;
        }
        at.insert(name, table.len() as u32);
        table.extend_from_slice(name.as_bytes());
        table.push(0);
        previous = name;
    }
    (table, at)
}

fn put16(out: &mut [u8], at: usize, v: u16) {
    out[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(out: &mut [u8], at: usize, v: u64) {
    out[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn align_to(v: u64, align: u64) -> u64 {
    let a = align.max(1);
    v.div_ceil(a) * a
}

/// `obj` without its debug information ([the module's description](self)).
pub fn strip_debug(obj: &[u8]) -> Result<Vec<u8>, Error> {
    let elf = Elf::parse(obj)?;
    if elf.class != Class::Elf64 || elf.header.ty != ET_REL {
        return bad("strip: an ELF64 relocatable object is needed");
    }
    let secs = &elf.sections;
    let n = secs.len();
    for s in secs {
        if matches!(s.ty, SHT_GROUP | SHT_SYMTAB_SHNDX | SHT_LLVM_ADDRSIG) {
            return bad(format!(
                "strip: section {} (type {:#x}) is not supported",
                s.name, s.ty
            ));
        }
    }
    // Which sections go: the debug ones, then the relocations of those.
    let mut gone = vec![false; n];
    for (i, s) in secs.iter().enumerate().skip(1) {
        gone[i] = is_debug_section(&s.name);
    }
    for (i, s) in secs.iter().enumerate().skip(1) {
        if matches!(s.ty, SHT_RELA | SHT_REL) && gone.get(s.info as usize) == Some(&true) {
            gone[i] = true;
        }
    }
    let mut index = vec![0u32; n];
    let mut kept = Vec::new();
    for i in 0..n {
        if !gone[i] {
            index[i] = kept.len() as u32;
            kept.push(i);
        }
    }
    let new_shndx = |shndx: u16| -> u16 {
        if shndx == 0 || shndx >= SHN_LORESERVE {
            shndx
        } else {
            index[shndx as usize] as u16
        }
    };

    // The symbols, and the names they keep.
    let symtab = secs.iter().position(|s| s.ty == SHT_SYMTAB);
    let shstrndx = elf.header.shstrndx as usize;
    let strtab = symtab.map(|t| secs[t].link as usize);
    if strtab == Some(shstrndx) {
        return bad("strip: symbols and sections sharing a string table are not supported");
    }
    let symbols = match symtab {
        Some(t) => elf.symbols_in(&secs[t])?,
        None => Vec::new(),
    };
    let mut sym_index: Vec<Option<u32>> = vec![None; symbols.len()];
    let mut kept_syms = Vec::new();
    for (i, s) in symbols.iter().enumerate() {
        let in_gone = s.shndx != 0 && s.shndx < SHN_LORESERVE && gone.get(s.shndx as usize) == Some(&true);
        if i == 0 || (s.ty() != STT_FILE && !in_gone) {
            sym_index[i] = Some(kept_syms.len() as u32);
            kept_syms.push(s);
        }
    }
    let (strtab_bytes, name_at) = string_table(kept_syms.iter().map(|s| s.name.as_str()));
    let (shstrtab_bytes, sec_name_at) = string_table(kept.iter().map(|&i| secs[i].name.as_str()));

    // Each kept section's new contents.
    let mut contents: Vec<Vec<u8>> = Vec::with_capacity(kept.len());
    for &i in &kept {
        let s = &secs[i];
        let data = if Some(i) == symtab {
            let mut out = vec![0u8; kept_syms.len() * 24];
            for (k, sym) in kept_syms.iter().enumerate() {
                let at = k * 24;
                put32(&mut out, at, name_at[sym.name.as_str()]);
                out[at + 4] = sym.info;
                out[at + 5] = sym.other;
                put16(&mut out, at + 6, new_shndx(sym.shndx));
                put64(&mut out, at + 8, sym.value);
                put64(&mut out, at + 16, sym.size);
            }
            out
        } else if Some(i) == strtab {
            strtab_bytes.clone()
        } else if i == shstrndx {
            shstrtab_bytes.clone()
        } else if matches!(s.ty, SHT_RELA | SHT_REL) {
            let mut out = elf.section_data(s)?.to_vec();
            let entsize = if s.ty == SHT_RELA { 24 } else { 16 };
            for at in (0..out.len() / entsize).map(|k| k * entsize) {
                let r_info = u64::from_le_bytes(out[at + 8..at + 16].try_into().unwrap());
                let (sym, ty) = ((r_info >> 32) as usize, r_info & 0xffff_ffff);
                let Some(new) = sym_index.get(sym).copied().flatten() else {
                    return bad(format!(
                        "strip: a relocation in {} refers to symbol {sym}, which goes",
                        s.name
                    ));
                };
                put64(&mut out, at + 8, (u64::from(new) << 32) | ty);
            }
            out
        } else if s.ty == SHT_NOBITS || i == 0 {
            Vec::new()
        } else {
            elf.section_data(s)?.to_vec()
        };
        contents.push(data);
    }

    // The layout: by old offset, from the end of the ELF header.
    let mut order: Vec<usize> = (1..kept.len()).collect();
    order.sort_by_key(|&k| secs[kept[k]].offset);
    let mut offsets = vec![0u64; kept.len()];
    let mut at = 64u64;
    for &k in &order {
        let s = &secs[kept[k]];
        at = align_to(at, s.align);
        offsets[k] = at;
        if s.ty != SHT_NOBITS {
            at += contents[k].len() as u64;
        }
    }
    let shoff = align_to(at, 8);
    let mut out = vec![0u8; (shoff + 64 * kept.len() as u64) as usize];
    out[..64].copy_from_slice(&obj[..64]);
    put64(&mut out, 0x28, shoff);
    put16(&mut out, 0x3c, kept.len() as u16);
    put16(&mut out, 0x3e, index[shstrndx] as u16);
    for k in 1..kept.len() {
        let i = kept[k];
        let s: &Section = &secs[i];
        if s.ty != SHT_NOBITS {
            let o = offsets[k] as usize;
            out[o..o + contents[k].len()].copy_from_slice(&contents[k]);
        }
        let (link, info) = if Some(i) == symtab {
            let first_global = kept_syms
                .iter()
                .position(|s| s.bind() != STB_LOCAL)
                .unwrap_or(kept_syms.len());
            (index[s.link as usize], first_global as u32)
        } else if matches!(s.ty, SHT_RELA | SHT_REL) {
            (index[s.link as usize], index[s.info as usize])
        } else {
            (s.link, s.info)
        };
        let size = if s.ty == SHT_NOBITS {
            s.size
        } else {
            contents[k].len() as u64
        };
        let h = (shoff + 64 * k as u64) as usize;
        put32(&mut out, h, sec_name_at[s.name.as_str()]);
        put32(&mut out, h + 4, s.ty);
        put64(&mut out, h + 8, s.flags);
        put64(&mut out, h + 16, s.addr);
        put64(&mut out, h + 24, offsets[k]);
        put64(&mut out, h + 32, size);
        put32(&mut out, h + 40, link);
        put32(&mut out, h + 44, info);
        put64(&mut out, h + 48, s.align);
        put64(&mut out, h + 56, s.entsize);
    }
    Ok(out)
}
