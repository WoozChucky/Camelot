//! merlin: compiles a subset of C into Excalibur assembly for merlin-as.
//!
//! Pipeline: [`preprocess`] (on the [`lexer`]'s preprocessing tokens) →
//! [`lexer::finish`] → [`parser`] → [`sema`] (name resolution, type checking)
//! → [`codegen`]. [`parse_with`] runs the front end and returns a checked,
//! typed AST; [`compile_with`] goes all the way to assembly text, one
//! translation unit at a time ([`parse`] and [`compile`] for a source with no
//! headers). The target is Excalibur, RV64 with the LP64 ABI ([`MARCH`],
//! [`ABI`]; until #159 merlin compiled for RV32 and ILP32 too). [`types`]
//! has the C types and their layout, [`consteval`] evaluates constant
//! expressions, and [`abi`] says how arguments and results travel (the
//! RISC-V integer calling convention).
//!
//! The preprocessor also serves assembly: [`preprocess_asm`] runs a `.S`
//! file through it as `clang -E -x assembler-with-cpp` does, for merlin-as.
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

/// The ISA merlin compiles for, as clang spells it for `-march=`: the one
/// merlin-as assembles.
pub const MARCH: &str = "rv64ima_zicsr_zifencei";

/// Reads the value of `-march=`: [`MARCH`], the only ISA merlin takes. Any
/// other is refused, an RV32 one as any unknown one.
pub fn parse_march(s: &str) -> Result<(), String> {
    if s == MARCH {
        Ok(())
    } else {
        Err(format!("unsupported -march '{s}' (merlin takes {MARCH})"))
    }
}

/// The ABI, as `-mabi=` names it: `lp64` (soft-float: merlin has no
/// floating point).
pub const ABI: &str = "lp64";

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
/// into the text merlin-as assembles, as `clang -E`
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
    let mut prog = check(&pp).map_err(|errors| pp.name_files(errors))?;
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
    let debug = opts.debug.as_ref().map(|d| (d, pp.files.as_slice()));
    let prog = check(&pp).map_err(|errors| pp.name_files(errors))?;
    let warnings = pp.name_files(prog.warnings.clone());
    match codegen::generate_with(&prog, debug) {
        Ok(asm) => Ok((asm, warnings)),
        Err(e) => Err(warnings.into_iter().chain(pp.name_files(vec![e])).collect()),
    }
}

/// The front end after the preprocessor: tokens, syntax tree, checks. The
/// errors come with the warnings, in the order of their positions.
fn check(pp: &preprocess::Output) -> Result<ast::Program, Vec<Error>> {
    let toks = lexer::finish(pp.tokens.clone(), pp.end).map_err(|e| vec![e])?;
    let mut prog = match parser::parse_salvaging(&toks) {
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
