//! merlin-ld: links RISC-V relocatable objects into an Excalibur executable
//! (`ET_EXEC`) that Grail loads directly: ELF64 (RV64) objects into an ELF64
//! executable. (Until #159 it linked ELF32 (RV32) objects too.)
//!
//! Pipeline: [`object`] (read `.o` files) → [`layout`] (symbols, section
//! merging, addresses; driven by a [`script`]) → [`relocate`] (copies section
//! contents and calls [`reloc::apply`] for every relocation) → [`write`].
//!
//! [`reloc`] is where each relocation's formula and instruction field are
//! explained; start there to see how addresses get into the code.
//! [`library`] picks the members of a library a program needs, as ld.lld
//! does with an archive ([`link_files`]); `-L` and `-l` find an archive by
//! name as ld does ([`search_library`]).

pub mod layout;
pub mod library;
pub mod map;
pub mod object;
pub mod reloc;
pub mod relocate;
pub mod script;
pub mod write;

pub use layout::{Layout, OutputSection};

use std::fmt;

/// A diagnostic, already prefixed with the file it concerns
/// (`a.o: undefined reference to 'foo'`, `link.ld:3: ...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An input object file.
#[derive(Debug, Clone)]
pub struct Input {
    /// Shown in diagnostics.
    pub name: String,
    pub data: Vec<u8>,
}

/// A linker script's source text.
#[derive(Debug, Clone)]
pub struct ScriptSource {
    /// Shown in diagnostics.
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `-T`; without it the default layout is used (`.text` at 0x8000_0000,
    /// then `.rodata`, `.data`, `.bss`; [`script::DEFAULT_SCRIPT`]). For
    /// RV64 that takes pc-relative code only (`-mcmodel=medany`): `lui
    /// %hi(x)` cannot reach 0x8000_0000 there.
    pub script: Option<ScriptSource>,
    /// `-e`: an entry symbol or address. Overrides `ENTRY(...)`.
    pub entry: Option<String>,
}

/// A finished link.
#[derive(Debug)]
pub struct Linked {
    pub elf: Vec<u8>,
    pub layout: Layout,
}

/// Resolves symbols and lays out sections, without applying relocations.
pub fn layout(inputs: &[Input], opts: &Options) -> Result<Layout, Vec<Error>> {
    layout::build(inputs, opts)
}

/// Links `inputs` into an executable.
pub fn link(inputs: &[Input], opts: &Options) -> Result<Linked, Vec<Error>> {
    finish(layout::build(inputs, opts)?)
}

/// Relocates and writes a finished layout.
fn finish(layout: Layout) -> Result<Linked, Vec<Error>> {
    let images = relocate::relocate(&layout)?;
    let elf = write::write(&layout, &images);
    Ok(Linked { elf, layout })
}

/// `elf`, an executable merlin-ld wrote, without its debug information
/// (`--strip-debug`): the same bytes as the link would give with no
/// `.debug_*` sections in its inputs ([`write::strip_debug`]).
pub fn strip_debug(elf: &[u8]) -> Result<Vec<u8>, Error> {
    write::strip_debug(elf).map_err(Error)
}

/// Links `inputs` with the members of `library` they need (objects linked
/// only when they define something still undefined, like an archive's:
/// [`library`]), after them in the order ld.lld would link them.
pub fn link_with_library(inputs: &[Input], library: &[Input], opts: &Options) -> Result<Linked, Vec<Error>> {
    let files: Vec<(&Input, bool)> = inputs
        .iter()
        .map(|i| (i, false))
        .chain(library.iter().map(|i| (i, true)))
        .collect();
    link_files(&files, opts)
}

/// Links the files of a command line, in its order, each with whether it
/// is a library member (between `--start-lib` and `--end-lib`): what
/// merlin-ld's command line does. An archive (`libc.a`) stands for its
/// members, each a library member ([`library::expand_archives`]). The
/// members are linked only when needed, in the order ld.lld links them
/// ([`library`]), but the script and every file are read in command-line
/// order, as ld.lld does ([`layout::read`]): so are their errors.
pub fn link_files(files: &[(&Input, bool)], opts: &Options) -> Result<Linked, Vec<Error>> {
    let library::Expanded {
        files: expanded,
        warnings,
    } = library::expand_archives(files)?;
    let files: Vec<(&Input, bool)> = expanded.iter().map(|(i, lazy)| (i, *lazy)).collect();
    let (objects, script) = layout::read(&files, opts)?;
    let lazy: Vec<bool> = files.iter().map(|&(_, lazy)| lazy).collect();
    let entry = layout::entry_symbol(opts, &script);
    let referenced = script.referenced_symbols();
    let order = library::order(
        &objects.iter().collect::<Vec<_>>(),
        &lazy,
        entry.as_deref(),
        &referenced,
    );
    let mut objects: Vec<Option<object::Object>> = objects.into_iter().map(Some).collect();
    let objects = order.into_iter().filter_map(|i| objects[i].take()).collect();
    let mut linked = finish(layout::build_from(objects, script, opts)?)?;
    linked.layout.warnings.splice(0..0, warnings);
    Ok(linked)
}

/// Where `-lNAME` is: the first `-L` directory, in command-line order, that
/// holds `libNAME.a`, as ld finds a library when it links statically (every
/// `-L` counts, wherever it is on the command line: ld.lld and GNU ld read
/// them all first). `-l:FILE` looks for FILE itself. merlin-ld has no
/// shared libraries, so `libNAME.so` is never looked for, as with ld's
/// `-static`. Fails as ld.lld does: `unable to find library -lNAME`.
pub fn search_library(name: &str, dirs: &[String], exists: impl Fn(&str) -> bool) -> Result<String, Error> {
    let file = match name.strip_prefix(':') {
        Some(file) => file.to_string(),
        None => format!("lib{name}.a"),
    };
    for dir in dirs {
        let path = if dir.ends_with('/') {
            format!("{dir}{file}")
        } else {
            format!("{dir}/{file}")
        };
        if exists(&path) {
            return Ok(path);
        }
    }
    Err(Error(format!("unable to find library -l{name}")))
}
