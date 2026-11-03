use super::*;

/// Builds a small ET_REL by hand: null, .text, .rela.text, .symtab, .strtab, .shstrtab.
fn tiny_object() -> Vec<u8> {
    build_object(false)
}

/// The same object with its relocations in a `.rel.text` (SHT_REL, no addends).
fn tiny_rel_object() -> Vec<u8> {
    build_object(true)
}

fn build_object(rel: bool) -> Vec<u8> {
    fn p16(b: &mut Vec<u8>, v: u16) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn p32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let text: Vec<u8> = [0x0000_0097u32, 0x0000_80e7]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let mut rela = Vec::new();
    p32(&mut rela, 0);
    p32(&mut rela, (2 << 8) | 19); // sym 2 (ext), R_RISCV_CALL_PLT
    if rel {
        // A second entry, so reading an addend from a REL entry would pick
        // up this one's offset.
        p32(&mut rela, 4);
        p32(&mut rela, 51); // R_RISCV_RELAX
    } else {
        p32(&mut rela, -4i32 as u32);
    }
    let strtab = b"\0start\0ext\0".to_vec();
    let mut symtab = vec![0u8; 16];
    for (name, value, info, shndx) in [
        (1u32, 4u32, (STB_GLOBAL << 4) | STT_FUNC, 1u16),
        (7, 0, STB_GLOBAL << 4, 0),
    ] {
        p32(&mut symtab, name);
        p32(&mut symtab, value);
        p32(&mut symtab, 8);
        symtab.push(info);
        symtab.push(0);
        p16(&mut symtab, shndx);
    }
    let shstrtab = b"\0.text\0.rela.text\0.symtab\0.strtab\0.shstrtab\0.rel.text\0".to_vec();
    let (rel_name, rel_ty, rel_entsize) = if rel { (44, SHT_REL, 8) } else { (7, SHT_RELA, 12) };

    let mut out = vec![0u8; EHDR_SIZE];
    let mut place = |data: &[u8]| {
        let off = out.len() as u32;
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
    let shoff = out.len() as u32;
    // (name, type, flags, offset, size, link, info, entsize)
    let shdrs = [
        (0, SHT_NULL, 0, 0, 0, 0, 0, 0),
        (1, SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR, offs[0], 8, 0, 0, 0),
        (
            rel_name,
            rel_ty,
            SHF_INFO_LINK,
            offs[1],
            rela.len() as u32,
            3,
            1,
            rel_entsize,
        ),
        (18, SHT_SYMTAB, 0, offs[2], 48, 4, 1, 16),
        (26, SHT_STRTAB, 0, offs[3], strtab.len() as u32, 0, 0, 0),
        (34, SHT_STRTAB, 0, offs[4], shstrtab.len() as u32, 0, 0, 0),
    ];
    for (name, ty, flags, off, size, link, info, entsize) in shdrs {
        for v in [name, ty, flags, 0, off, size, link, info, 4, entsize] {
            p32(&mut out, v);
        }
    }
    let mut h = Vec::new();
    h.extend_from_slice(b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0");
    p16(&mut h, ET_REL);
    p16(&mut h, EM_RISCV);
    p32(&mut h, 1);
    p32(&mut h, 0); // entry
    p32(&mut h, 0); // phoff
    p32(&mut h, shoff);
    p32(&mut h, 0); // flags
    p16(&mut h, EHDR_SIZE as u16);
    p16(&mut h, PHDR_SIZE as u16);
    p16(&mut h, 0);
    p16(&mut h, SHDR_SIZE as u16);
    p16(&mut h, shdrs.len() as u16);
    p16(&mut h, 5);
    out[..EHDR_SIZE].copy_from_slice(&h);
    out
}

#[test]
fn parses_hand_built_object() {
    let bytes = tiny_object();
    let elf = Elf::parse(&bytes).unwrap();
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
    assert_eq!(elf.symbol("start"), Some(4));
    assert_eq!(elf.symbol("ext"), None, "undefined symbols have no value");

    let relocs = elf.relocations(&elf.sections[2]).unwrap();
    assert_eq!(
        relocs,
        [Rela {
            offset: 0,
            sym: 2,
            ty: 19,
            addend: -4
        }]
    );
    assert!(elf.relocations(&elf.sections[1]).is_err());
}

#[test]
fn reads_rel_entries_without_addends() {
    let bytes = tiny_rel_object();
    let elf = Elf::parse(&bytes).unwrap();
    assert_eq!(elf.sections[2].name, ".rel.text");
    assert_eq!(elf.sections[2].ty, SHT_REL);
    let relocs = elf.relocations(&elf.sections[2]).unwrap();
    assert_eq!(
        relocs,
        [
            Rela {
                offset: 0,
                sym: 2,
                ty: 19,
                addend: 0
            },
            Rela {
                offset: 4,
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
    bad[4] = 2;
    assert!(Elf::parse(&bad).err().unwrap().0.contains("32-bit"));
    let mut bad = good.clone();
    bad[5] = 2;
    assert!(Elf::parse(&bad).err().unwrap().0.contains("little-endian"));
    let mut bad = good.clone();
    bad[18] = 62; // EM_X86_64
    assert!(Elf::parse(&bad).err().unwrap().0.contains("RISC-V"));
}

/// Every truncation and a few thousand random corruptions of real files must
/// give errors, never panics, through every accessor.
#[test]
fn corrupt_files_do_not_panic() {
    fn exercise(bytes: &[u8]) {
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
    }
    let hello = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/hello/hello.elf"
    ))
    .unwrap();
    for file in [tiny_object(), hello] {
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
