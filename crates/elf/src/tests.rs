use super::*;

/// Builds a small ET_REL by hand: null, .text, .rela.text, .symtab, .strtab, .shstrtab.
fn tiny_object() -> Vec<u8> {
    build_object(false, Class::Elf32)
}

/// The same object with its relocations in a `.rel.text` (SHT_REL, no addends).
fn tiny_rel_object() -> Vec<u8> {
    build_object(true, Class::Elf32)
}

/// Added to the hand-built object's offsets, values, sizes and addends in
/// ELF64, so that a 64-bit field cut to 32 bits shows.
fn high(class: Class) -> u64 {
    match class {
        Class::Elf32 => 0,
        Class::Elf64 => 0x1234_5678_0000_0000,
    }
}

fn build_object(rel: bool, class: Class) -> Vec<u8> {
    fn p16(b: &mut Vec<u8>, v: u16) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn p32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    // An address-sized field: 4 bytes in ELF32, 8 in ELF64.
    let pa = |b: &mut Vec<u8>, v: u64| match class {
        Class::Elf32 => p32(b, v as u32),
        Class::Elf64 => b.extend_from_slice(&v.to_le_bytes()),
    };
    // r_info: the symbol index above the type.
    let info = |b: &mut Vec<u8>, sym: u32, ty: u32| match class {
        Class::Elf32 => p32(b, (sym << 8) | ty),
        Class::Elf64 => b.extend_from_slice(&((u64::from(sym) << 32) | u64::from(ty)).to_le_bytes()),
    };
    let text: Vec<u8> = [0x0000_0097u32, 0x0000_80e7]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let mut rela = Vec::new();
    let high = high(class);
    pa(&mut rela, high);
    info(&mut rela, 2, 19); // sym 2 (ext), R_RISCV_CALL_PLT
    if rel {
        // A second entry, so reading an addend from a REL entry would pick
        // up this one's offset.
        pa(&mut rela, high + 4);
        info(&mut rela, 0, 51); // R_RISCV_RELAX
    } else {
        pa(&mut rela, (-4i64 as u64).wrapping_sub(high));
    }
    let strtab = b"\0start\0ext\0".to_vec();
    let mut symtab = vec![0u8; class.sym_size()];
    for (name, value, info, shndx) in [
        (1u32, high + 4, (STB_GLOBAL << 4) | STT_FUNC, 1u16),
        (7, 0, STB_GLOBAL << 4, 0),
    ] {
        p32(&mut symtab, name);
        if class == Class::Elf32 {
            pa(&mut symtab, value);
            pa(&mut symtab, high + 8);
        }
        symtab.push(info);
        symtab.push(0);
        p16(&mut symtab, shndx);
        if class == Class::Elf64 {
            pa(&mut symtab, value);
            pa(&mut symtab, high + 8);
        }
    }
    let shstrtab = b"\0.text\0.rela.text\0.symtab\0.strtab\0.shstrtab\0.rel.text\0".to_vec();
    let (rel_name, rel_ty, rel_entsize) = if rel {
        (44, SHT_REL, class.rel_size())
    } else {
        (7, SHT_RELA, class.rela_size())
    };

    let mut out = vec![0u8; class.ehdr_size()];
    let mut place = |data: &[u8]| {
        let off = out.len() as u64;
        out.extend_from_slice(data);
        off
    };
    let offs = [
        place(&text),
        place(&rela),
        place(&symtab),
        place(&strtab),
        place(&shstrtab),
    ];
    let shoff = out.len() as u64;
    // (name, type, flags, offset, size, link, info, entsize)
    let shdrs = [
        (0, SHT_NULL, 0, 0, 0, 0, 0, 0),
        (1, SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR, offs[0], 8, 0, 0, 0),
        (
            rel_name,
            rel_ty,
            SHF_INFO_LINK,
            offs[1],
            rela.len() as u64,
            3,
            1,
            rel_entsize as u64,
        ),
        (
            18,
            SHT_SYMTAB,
            0,
            offs[2],
            symtab.len() as u64,
            4,
            1,
            class.sym_size() as u64,
        ),
        (26, SHT_STRTAB, 0, offs[3], strtab.len() as u64, 0, 0, 0),
        (34, SHT_STRTAB, 0, offs[4], shstrtab.len() as u64, 0, 0, 0),
    ];
    for (name, ty, flags, off, size, link, info, entsize) in shdrs {
        p32(&mut out, name);
        p32(&mut out, ty);
        for v in [flags, 0, off, size] {
            pa(&mut out, v);
        }
        p32(&mut out, link);
        p32(&mut out, info);
        pa(&mut out, 4);
        pa(&mut out, entsize);
    }
    let mut h = Vec::new();
    h.extend_from_slice(b"\x7fELF");
    h.push(match class {
        Class::Elf32 => ELFCLASS32,
        Class::Elf64 => ELFCLASS64,
    });
    h.extend_from_slice(b"\x01\x01\0\0\0\0\0\0\0\0\0");
    p16(&mut h, ET_REL);
    p16(&mut h, EM_RISCV);
    p32(&mut h, 1);
    pa(&mut h, 0); // entry
    pa(&mut h, 0); // phoff
    pa(&mut h, shoff);
    p32(&mut h, 0); // flags
    p16(&mut h, class.ehdr_size() as u16);
    p16(&mut h, class.phdr_size() as u16);
    p16(&mut h, 0);
    p16(&mut h, class.shdr_size() as u16);
    p16(&mut h, shdrs.len() as u16);
    p16(&mut h, 5);
    out[..class.ehdr_size()].copy_from_slice(&h);
    out
}

#[test]
fn parses_hand_built_object() {
    parses_hand_built_object_of(tiny_object(), Class::Elf32);
    parses_hand_built_object_of(build_object(false, Class::Elf64), Class::Elf64);
}

fn parses_hand_built_object_of(bytes: Vec<u8>, class: Class) {
    let elf = Elf::parse(&bytes).unwrap();
    assert_eq!(elf.class, class);
    assert_eq!(elf.header.ty, ET_REL);
    assert!(elf.segments.is_empty());
    let names: Vec<&str> = elf.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["", ".text", ".rela.text", ".symtab", ".strtab", ".shstrtab"]
    );
    assert!(elf.sections[1].is_exec() && elf.sections[1].is_alloc());
    assert_eq!(
        elf.section_data(&elf.sections[1]).unwrap(),
        &[0x97, 0, 0, 0, 0xe7, 0x80, 0, 0]
    );

    let syms = elf.symbols().unwrap();
    assert_eq!(syms.len(), 3);
    assert_eq!(syms[1].name, "start");
    assert_eq!(
        (syms[1].bind(), syms[1].ty(), syms[1].section()),
        (STB_GLOBAL, STT_FUNC, Some(1))
    );
    assert!(syms[2].is_undefined() && syms[2].section().is_none());
    let high = high(class);
    assert_eq!(syms[1].size, high + 8);
    assert_eq!(elf.symbol("start"), Some(high + 4));
    assert_eq!(elf.symbol("ext"), None, "undefined symbols have no value");

    let relocs = elf.relocations(&elf.sections[2]).unwrap();
    assert_eq!(
        relocs,
        [Rela {
            offset: high,
            sym: 2,
            ty: 19,
            addend: -4 - high as i64
        }]
    );
    assert!(elf.relocations(&elf.sections[1]).is_err());
}

#[test]
fn reads_rel_entries_without_addends() {
    reads_rel_entries_without_addends_of(tiny_rel_object(), Class::Elf32);
    reads_rel_entries_without_addends_of(build_object(true, Class::Elf64), Class::Elf64);
}

fn reads_rel_entries_without_addends_of(bytes: Vec<u8>, class: Class) {
    let elf = Elf::parse(&bytes).unwrap();
    let high = high(class);
    assert_eq!(elf.sections[2].name, ".rel.text");
    assert_eq!(elf.sections[2].ty, SHT_REL);
    let relocs = elf.relocations(&elf.sections[2]).unwrap();
    assert_eq!(
        relocs,
        [
            Rela {
                offset: high,
                sym: 2,
                ty: 19,
                addend: 0
            },
            Rela {
                offset: high + 4,
                sym: 0,
                ty: 51,
                addend: 0
            }
        ]
    );
}

#[test]
fn rejects_wrong_files() {
    let good = tiny_object();
    assert_eq!(Elf::parse(b"MZ\x90\0").err().unwrap().0, "missing magic");
    let mut bad = good.clone();
    bad[4] = 3;
    assert_eq!(
        Elf::parse(&bad).err().unwrap().0,
        "EI_CLASS 3 is neither ELFCLASS32 nor ELFCLASS64"
    );
    let mut bad = good.clone();
    bad[5] = 2;
    assert!(Elf::parse(&bad).err().unwrap().0.contains("little-endian"));
    let mut bad = good.clone();
    bad[18] = 62; // EM_X86_64
    assert!(Elf::parse(&bad).err().unwrap().0.contains("RISC-V"));
}

/// The offsets of `e_phoff`, `e_phnum` and `e_shstrndx`: the fields after
/// the three address-sized ones move up by 12 bytes in ELF64.
fn header_offsets(class: Class) -> (usize, usize, usize) {
    match class {
        Class::Elf32 => (28, 44, 50),
        Class::Elf64 => (32, 56, 62),
    }
}

#[test]
fn truncation_errors_name_the_bytes() {
    for class in [Class::Elf32, Class::Elf64] {
        let good = build_object(false, class);
        // In the file header: e_version is bytes 20..24 in both classes.
        assert_eq!(
            Elf::parse(&good[..22]).err().unwrap().0,
            "truncated file (needs bytes 0x14..0x18)"
        );
        // A section header is read whole: a cut in the middle of the third
        // one names all of its bytes.
        let shoff = Elf::parse(&good).unwrap().header.shoff as usize;
        let third = shoff + 2 * class.shdr_size();
        let cut = &good[..third + 10];
        assert_eq!(
            Elf::parse(cut).err().unwrap().0,
            format!(
                "truncated file (needs bytes {third:#x}..{:#x})",
                third + class.shdr_size()
            ),
            "{class:?}"
        );
    }
}

#[test]
fn section_names_come_only_from_a_string_table() {
    for class in [Class::Elf32, Class::Elf64] {
        let (_, _, shstrndx) = header_offsets(class);
        let good = build_object(false, class);
        // e_shstrndx naming .text (1), past the table (9) or none (0):
        // the file still parses, its sections without names.
        for index in [1u16, 9, 0] {
            let mut bad = good.clone();
            bad[shstrndx..shstrndx + 2].copy_from_slice(&index.to_le_bytes());
            let elf = Elf::parse(&bad).unwrap();
            assert_eq!(elf.sections.len(), 6, "{class:?} {index}");
            assert!(
                elf.sections.iter().all(|s| s.name.is_empty()),
                "{class:?} {index}"
            );
        }
        // A name offset past the end of the string table is an error.
        let shoff = Elf::parse(&good).unwrap().header.shoff as usize;
        let mut bad = good.clone();
        let sh_name = shoff + class.shdr_size();
        bad[sh_name..sh_name + 4].copy_from_slice(&0x99u32.to_le_bytes());
        assert_eq!(
            Elf::parse(&bad).err().unwrap().0,
            "string offset 0x99 outside its string table"
        );
    }
}

#[test]
fn parse_sections_only_reads_no_program_headers() {
    for class in [Class::Elf32, Class::Elf64] {
        let (phoff, phnum, _) = header_offsets(class);
        let good = build_object(false, class);
        // An object claiming 3 program headers far past its end.
        let mut stray = good.clone();
        stray[phnum..phnum + 2].copy_from_slice(&3u16.to_le_bytes());
        stray[phoff..phoff + 4].copy_from_slice(&0xf_fff0u32.to_le_bytes());
        assert!(Elf::parse(&stray).err().unwrap().0.starts_with("truncated file"));
        let elf = Elf::parse_sections_only(&stray).unwrap();
        assert!(elf.segments.is_empty());
        assert_eq!(elf.sections, Elf::parse(&good).unwrap().sections);
    }
}

/// Every truncation and a few thousand random corruptions of real files must
/// give errors, never panics, through every accessor.
#[test]
fn corrupt_files_do_not_panic() {
    fn exercise(bytes: &[u8]) {
        let _ = Elf::parse_sections_only(bytes);
        let Ok(elf) = Elf::parse(bytes) else { return };
        for s in &elf.sections {
            let _ = elf.section_data(s);
            let _ = elf.relocations(s);
            if s.ty == SHT_SYMTAB {
                let _ = elf.symbols_in(s);
            }
        }
        for p in &elf.segments {
            let _ = elf.segment_data(p);
        }
        let _ = elf.symbol("_start");
        let _ = elf.line_tables();
    }
    let hello = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/hello/hello.elf"
    ))
    .unwrap();
    let rv64 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/llvm/rv64/prog.elf")).unwrap();
    for file in [tiny_object(), build_object(false, Class::Elf64), hello, rv64] {
        for len in 0..file.len() {
            exercise(&file[..len]);
        }
        let mut seed = 0x1234_5678u32;
        for _ in 0..3000 {
            let mut f = file.clone();
            for _ in 0..4 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let i = (seed >> 8) as usize % f.len();
                f[i] = (seed >> 24) as u8;
            }
            exercise(&f);
        }
    }
}

#[test]
fn reads_linked_executable() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/hello/hello.elf"
    ))
    .unwrap();
    let elf = Elf::parse(&bytes).unwrap();
    assert_eq!(elf.header.ty, ET_EXEC);
    assert_eq!(elf.header.entry, 0x8000_0000);
    let load: Vec<&ProgramHeader> = elf.segments.iter().filter(|p| p.ty == PT_LOAD).collect();
    assert_eq!(load[0].vaddr, 0x8000_0000);
    assert_eq!(load[0].flags, PF_R | PF_X);
    let text = elf.sections.iter().find(|s| s.name == ".text").unwrap();
    assert_eq!(text.addr, 0x8000_0000);
    // First instruction: auipc sp, 0x10.
    assert_eq!(
        &elf.section_data(text).unwrap()[..4],
        &0x0001_0117u32.to_le_bytes()
    );
    assert_eq!(elf.symbol("_start"), Some(0x8000_0000));
    // After the 8-byte `call puts` and the two Xcal instructions (merlin-ld does
    // no relaxation, so the call stays auipc + jalr).
    assert_eq!(elf.symbol("puts"), Some(0x8000_0020));
}

#[test]
fn a_global_symbol_wins_over_a_local_of_the_same_name() {
    let sym = |name: &str, value, bind: u8, shndx| Symbol {
        name: name.into(),
        value,
        size: 0,
        info: bind << 4,
        other: 0,
        shndx,
    };
    // A `static void start(...)` in one file and the global `start` in
    // another: locals come first in an executable's .symtab.
    let syms = [
        sym("", 0, STB_LOCAL, SHN_UNDEF),
        sym("start", 0x10, STB_LOCAL, 1),
        sym("only_local", 0x14, STB_LOCAL, 1),
        sym("start", 0x20, STB_GLOBAL, 1),
        sym("weak", 0x30, STB_WEAK, 1),
        sym("ext", 0, STB_GLOBAL, SHN_UNDEF),
    ];
    let value = |name| find_symbol(&syms, name).map(|s| s.value);
    assert_eq!(value("start"), Some(0x20));
    assert_eq!(value("only_local"), Some(0x14));
    assert_eq!(value("weak"), Some(0x30));
    assert_eq!(value("ext"), None);
    assert_eq!(value("nothing"), None);
}

#[test]
fn reloc_names() {
    assert_eq!(reloc_name(19), Some("R_RISCV_CALL_PLT"));
    assert_eq!(reloc_name(23), Some("R_RISCV_PCREL_HI20"));
    assert_eq!(reloc_name(51), Some("R_RISCV_RELAX"));
    assert_eq!(reloc_name(42), None);
}
