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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Where the problem is, or line 0 when it isn't tied to a place in the source.
    pub pos: Pos,
    pub msg: String,
    /// The name of the file the position is in, when it is not the file
    /// being compiled (a header it includes). Set by [`compile_with`] and
    /// friends, which know the names.
    pub file: Option<String>,
}

impl Error {
    pub fn new(pos: Pos, msg: impl Into<String>) -> Self {
        Error {
            pos,
            msg: msg.into(),
            file: None,
        }
    }

    /// Formats the error the way compilers and editors expect:
    /// `file.c:line:col: error: message`, where `file` names the file being
    /// compiled (an error in a header names the header instead).
    pub fn render(&self, file: &str) -> String {
        let file = self.file.as_deref().unwrap_or(file);
        if self.pos.line > 0 {
            format!("{file}:{}: error: {}", self.pos, self.msg)
        } else {
            format!("{file}: error: {}", self.msg)
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
    check(&pp, opts.xlen).map_err(|errors| pp.name_files(errors))
}

/// Compiles one translation unit to assembly text for merlin-as.
pub fn compile_with(name: &str, src: &str, opts: &Options, load: Loader) -> Result<String, Vec<Error>> {
    let pp = preprocess(name, src, opts, load)?;
    let debug = opts.debug.as_ref().map(|d| (d, pp.files.as_slice()));
    check(&pp, opts.xlen)
        .and_then(|prog| codegen::generate_with(&prog, debug).map_err(|e| vec![e]))
        .map_err(|errors| pp.name_files(errors))
}

/// The front end after the preprocessor: tokens, syntax tree, checks.
fn check(pp: &preprocess::Output, xlen: types::Xlen) -> Result<ast::Program, Vec<Error>> {
    let toks = lexer::finish(pp.tokens.clone(), pp.end, xlen).map_err(|e| vec![e])?;
    let mut prog = parser::parse(&toks, xlen).map_err(|e| vec![e])?;
    sema::check(&mut prog)?;
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
