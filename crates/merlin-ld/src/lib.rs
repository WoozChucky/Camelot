//! merlin-ld: links ELF32 RISC-V relocatable objects into an Excalibur
//! executable (`ET_EXEC`) that Grail loads directly.
//!
//! Pipeline: [`object`] (read `.o` files) → [`layout`] (symbols, section
//! merging, addresses; driven by a [`script`]) → [`relocate`] (copies section
//! contents and calls [`reloc::apply`] for every relocation) → [`write`].
//!
//! [`reloc`] is where each relocation's formula and instruction field are
//! explained; start there to see how addresses get into the code.
//! [`library`] picks the members of a library a program needs, as ld does
//! with an archive ([`link_with_library`]).

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
    /// then `.rodata`, `.data`, `.bss`).
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
    let layout = layout::build(inputs, opts)?;
    let images = relocate::relocate(&layout)?;
    let elf = write::write(&layout, &images);
    Ok(Linked { elf, layout })
}

/// Links `inputs` with the members of `library` they need (objects linked
/// only when they define something still undefined, like an archive's:
/// [`library`]), after them in library order.
pub fn link_with_library(inputs: &[Input], library: &[Input], opts: &Options) -> Result<Linked, Vec<Error>> {
    link(&library::with_needed(inputs, library)?, opts)
}
