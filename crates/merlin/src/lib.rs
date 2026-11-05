//! merlin: compiles a subset of C into Excalibur assembly for merlin-as.
//!
//! Pipeline: [`preprocess`] (on the [`lexer`]'s preprocessing tokens) →
//! [`lexer::finish`] → [`parser`] → [`sema`] (name resolution, type checking)
//! → [`codegen`]. [`parse_with`] runs the front end and returns a checked,
//! typed AST; [`compile_with`] goes all the way to assembly text, one
//! translation unit at a time ([`parse`] and [`compile`] for a source with no
//! headers). The target is [`Options::xlen`] (`-march`): RV32 with the
//! ILP32 ABI, the default until #159, or RV64 with LP64. [`types`] has the C
//! types and their layout on each, [`consteval`] evaluates constant
//! expressions, and [`abi`] says how arguments and results travel (the
//! RISC-V integer calling convention).
//!
//! The preprocessor also serves assembly: [`preprocess_asm`] runs a `.S`
//! file through it as `clang -E -x assembler-with-cpp` does, for merlin-as
//! (how one source serves RV32 and RV64, with `__riscv_xlen`).
//!
//! The language (spec section 10) is a subset of C99 without floating point:
//! the C99 preprocessor, the integer types (`_Bool`, `char`, `short`, `int`,
//! `long`, `long long`, signed and unsigned), `enum`, pointers (to
//! functions too), arrays, structs and unions (passed and returned by value
//! as clang does), `typedef`, the `const` and `volatile` qualifiers, global
//! and `static` variables with initializers (designators included), compound
//! literals, string and character literals, functions with up to 127
//! parameters and variadic ones (with clang's `__builtin_va_*`; `inline`
//! accepted), every statement (`switch`, `goto` and labels included), and
//! every C operator.

pub mod abi;
pub mod ast;
pub mod codegen;
pub mod consteval;
pub mod debug;
pub mod elide;
pub mod lexer;
pub mod md5;
pub mod parser;
pub mod preprocess;
pub mod sema;
pub mod types;

pub use debug::DebugInfo;
pub use preprocess::{CmdMacro, Lang, Loader, Options};
use std::fmt;
use std::path::Path;

/// A source position: which file (0 is the file being compiled, the others
/// are the headers it includes, numbered by the preprocessor in the order
/// it first reads them), then the 1-based line and column (columns count
/// characters). Positions order by file first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub file: u32,
    pub line: u32,
    pub col: u32,
}

impl Pos {
    /// A position in the file being compiled.
    pub const fn new(line: u32, col: u32) -> Self {
        Pos { file: 0, line, col }
    }

    pub const fn in_file(file: u32, line: u32, col: u32) -> Self {
        Pos { file, line, col }
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A diagnostic: an error, which stops the compilation, or a warning, which
/// does not (what clang warns of by default, as it says it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Where the problem is, or line 0 when it isn't tied to a place in the source.
    pub pos: Pos,
    pub msg: String,
    /// The name of the file the position is in, when it is not the file
    /// being compiled (a header it includes). Set by [`compile_with`] and
    /// friends, which know the names.
    pub file: Option<String>,
    pub severity: Severity,
}

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    /// A warning, with the name of clang's flag for it
    /// (`gnu-folding-constant` for `-Wgnu-folding-constant`).
    Warning(&'static str),
}

impl Error {
    pub fn new(pos: Pos, msg: impl Into<String>) -> Self {
        Error {
            pos,
            msg: msg.into(),
            file: None,
            severity: Severity::Error,
        }
    }

    /// A warning, of clang's warning flag `flag`.
    pub fn warning(pos: Pos, msg: impl Into<String>, flag: &'static str) -> Self {
        Error {
            severity: Severity::Warning(flag),
            ..Error::new(pos, msg)
        }
    }

    pub fn is_warning(&self) -> bool {
        matches!(self.severity, Severity::Warning(_))
    }

    /// Formats the diagnostic the way compilers and editors expect, as
    /// clang does: `file.c:line:col: error: message`, or
    /// `file.c:line:col: warning: message [-Wflag]`, where `file` names the
    /// file being compiled (one in a header names the header instead).
    pub fn render(&self, file: &str) -> String {
        let file = self.file.as_deref().unwrap_or(file);
        let (what, flag) = match self.severity {
            Severity::Error => ("error", String::new()),
            Severity::Warning(flag) => ("warning", format!(" [-W{flag}]")),
        };
        if self.pos.line > 0 {
            format!("{file}:{}: {what}: {}{flag}", self.pos, self.msg)
        } else {
            format!("{file}: {what}: {}{flag}", self.msg)
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}:")?;
        }
        if self.pos.line > 0 {
            write!(f, "{}: {}", self.pos, self.msg)
        } else {
            write!(f, "{}", self.msg)
        }
    }
}

/// The `-march=` values merlin takes, and the width each one means: the two
/// ISAs merlin-as assembles, as clang spells them.
pub const MARCH: &[(&str, types::Xlen)] = &[
    ("rv32ima_zicsr_zifencei", types::Xlen::Rv32),
    ("rv64ima_zicsr_zifencei", types::Xlen::Rv64),
];

/// Reads the value of `-march=`: one of [`MARCH`].
pub fn parse_march(s: &str) -> Result<types::Xlen, String> {
    MARCH
        .iter()
        .find(|(name, _)| *name == s)
        .map(|&(_, x)| x)
        .ok_or_else(|| {
            let takes: Vec<&str> = MARCH.iter().map(|&(name, _)| name).collect();
            format!("unsupported -march '{s}' (merlin takes {})", takes.join(" or "))
        })
}

/// The ABI of each width, as `-mabi=` names it: `ilp32` for RV32, `lp64`
/// for RV64 (soft-float: merlin has no floating point).
pub fn abi_name(x: types::Xlen) -> &'static str {
    match x {
        types::Xlen::Rv32 => "ilp32",
        types::Xlen::Rv64 => "lp64",
    }
}

/// The name [`parse`] and [`compile`] give their source (for `__FILE__`).
pub const INPUT_NAME: &str = "<input>";

/// A [`Loader`] with no files: `#include` finds nothing. What [`parse`] and
/// [`compile`] use.
pub fn no_files(_: &Path) -> Option<String> {
    None
}

/// A [`Loader`] that reads the file system.
pub fn read_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Preprocesses one translation unit: the source `src` of the file `name`,
/// with headers read by `load`.
pub fn preprocess(
    name: &str,
    src: &str,
    opts: &Options,
    load: Loader,
) -> Result<preprocess::Output, Vec<Error>> {
    preprocess::preprocess(name, src, opts, load).map_err(|e| vec![e])
}

/// Preprocesses an assembly source (a `.S` file, `-x assembler-with-cpp`)
/// for [`Options::xlen`] into the text merlin-as assembles, as `clang -E`
/// writes it (see [`preprocess::Output::print_asm`]); [`Options::lang`] is
/// taken to be [`Lang::AssemblerWithCpp`]. Errors in headers have their
/// [`Error::file`] set.
pub fn preprocess_asm(name: &str, src: &str, opts: &Options, load: Loader) -> Result<String, Vec<Error>> {
    let opts = Options {
        lang: Lang::AssemblerWithCpp,
        ..opts.clone()
    };
    preprocess(name, src, &opts, load).map(|pp| pp.print_asm(opts.line_markers))
}

/// Preprocesses, lexes, parses and checks one translation unit. The
/// preprocessor and the parser stop at the first error; the checker reports
/// every error it finds. Errors in headers have their [`Error::file`] set.
pub fn parse_with(name: &str, src: &str, opts: &Options, load: Loader) -> Result<ast::Program, Vec<Error>> {
    let pp = preprocess(name, src, opts, load)?;
    let mut prog = check(&pp, opts.xlen).map_err(|errors| pp.name_files(errors))?;
    prog.warnings = pp.name_files(prog.warnings);
    Ok(prog)
}

/// Compiles one translation unit to assembly text for merlin-as. The
/// errors come with the warnings before them, if any ([`Error::severity`]).
pub fn compile_with(name: &str, src: &str, opts: &Options, load: Loader) -> Result<String, Vec<Error>> {
    compile_warned(name, src, opts, load).map(|(asm, _)| asm)
}

/// [`compile_with`], with the warnings of a compilation that succeeds.
pub fn compile_warned(
    name: &str,
    src: &str,
    opts: &Options,
    load: Loader,
) -> Result<(String, Vec<Error>), Vec<Error>> {
    let pp = preprocess(name, src, opts, load)?;
    let debug = opts.debug.as_ref().map(|d| {
        let sources = debug::Sources {
            names: &pp.files,
            md5: &pp.checksums,
        };
        (d, sources)
    });
    let prog = check(&pp, opts.xlen).map_err(|errors| pp.name_files(errors))?;
    let warnings = pp.name_files(prog.warnings.clone());
    match codegen::generate_with(&prog, debug) {
        Ok(asm) => Ok((format!("{}\n{asm}", file_directive(name)), warnings)),
        Err(e) => Err(warnings.into_iter().chain(pp.name_files(vec![e])).collect()),
    }
}

/// `.file "NAME"`, the first line of the assembly, as clang writes it with
/// or without `-g` (`AsmPrinter::doInitialization`, for an ELF target):
/// the base name of the file being compiled (`m.c` for `sub/m.c`, an
/// absolute name or `./m.c`), whatever `#line` says. The assembler makes
/// it the object's `STT_FILE` symbol, its first local symbol, which the
/// linker keeps in the executable's symbol table, so a reader can tell
/// which object each local symbol after it came from. It has nothing to do
/// with the numbered `.file`s of a line table ([`debug`]).
///
/// The base name is LLVM's `sys::path::filename` in the host's style, as
/// clang runs it on the name it was given: on Windows `\` separates names
/// too, and a drive (`C:`) is no part of the name.
fn file_directive(name: &str) -> String {
    let mut path = name;
    let unified;
    if cfg!(windows) {
        unified = name.replace('\\', "/");
        path = &unified;
        let b = path.as_bytes();
        if b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            path = &path[2..];
        }
    }
    format!("    .file {}", debug::quoted(merlin_as::dwarf::file_name(path)))
}

/// The front end after the preprocessor: tokens, syntax tree, checks. The
/// errors come with the warnings, in the order of their positions.
fn check(pp: &preprocess::Output, xlen: types::Xlen) -> Result<ast::Program, Vec<Error>> {
    let toks = lexer::finish(pp.tokens.clone(), pp.end, xlen).map_err(|e| vec![e])?;
    let mut prog = match parser::parse_salvaging(&toks, xlen) {
        Ok(prog) => prog,
        Err(salvaged) => {
            let (e, mut partial) = *salvaged;
            // The warnings of what came before a syntax error come with it,
            // as clang gives them; not the errors of what it cut short.
            let _ = sema::check(&mut partial);
            let mut errors = partial.warnings;
            errors.push(e);
            errors.sort_by_key(|e| e.pos);
            return Err(errors);
        }
    };
    if let Err(mut errors) = sema::check(&mut prog) {
        errors.extend(prog.warnings);
        errors.sort_by_key(|e| e.pos);
        return Err(errors);
    }
    Ok(prog)
}

/// Compiles the C file at `path`, with headers from the file system.
pub fn compile_file(path: &Path, opts: &Options) -> Result<String, Vec<Error>> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        vec![Error::new(
            Pos::default(),
            format!("cannot read {}: {e}", path.display()),
        )]
    })?;
    compile_with(&path.to_string_lossy(), &src, opts, &read_file)
}

/// [`parse_with`] for a source on its own: no headers, no `-D`.
pub fn parse(src: &str) -> Result<ast::Program, Vec<Error>> {
    parse_with(INPUT_NAME, src, &Options::default(), &no_files)
}

/// [`compile_with`] for a source on its own: no headers, no `-D`.
pub fn compile(src: &str) -> Result<String, Vec<Error>> {
    compile_with(INPUT_NAME, src, &Options::default(), &no_files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.file "NAME"` names the file by its base name, as clang does
    /// (checked against clang by the ignored `the_file_symbol_is_clangs`).
    #[test]
    fn the_file_directive_has_the_base_name() {
        for (name, base) in [
            ("m.c", "m.c"),
            ("./m.c", "m.c"),
            ("sub/n.c", "n.c"),
            ("sub/../m.c", "m.c"),
            ("/src/m.c", "m.c"),
            (INPUT_NAME, INPUT_NAME),
        ] {
            assert_eq!(file_directive(name), format!("    .file \"{base}\""), "{name}");
        }
        // On Windows `\` separates names too, and a drive is no part of one.
        let windows = if cfg!(windows) { "n.c" } else { "sub\\\\n.c" };
        assert_eq!(file_directive("sub\\n.c"), format!("    .file \"{windows}\""));
        let drive = if cfg!(windows) { "m.c" } else { "C:m.c" };
        assert_eq!(file_directive("C:m.c"), format!("    .file \"{drive}\""));
    }
}
