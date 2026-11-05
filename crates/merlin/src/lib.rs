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
pub mod flow;
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// Where an unnamed struct, union or enum is declared, as clang's name for
/// it gives the place (`struct (unnamed at t.c:3:1)`). Only the
/// preprocessor knows the files' names, so the file is a number between
/// two [`FILE_MARK`]s, which it replaces with the name
/// ([`preprocess::Output::name_files`]).
pub(crate) fn place(pos: Pos) -> String {
    format!("{FILE_MARK}{}{FILE_MARK}{pos}", pos.file)
}

/// See [`place`].
pub(crate) const FILE_MARK: char = '\u{1}';

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A diagnostic, as clang gives it: an error, which stops the compilation,
/// or a warning, which does not (what clang warns of by default, in its
/// words), each with the notes that follow it (where what it is about was
/// declared, say). [`Error::render`] prints it as clang does.
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
    /// The name of clang's `-W` flag for it, which clang prints after the
    /// message (`[-Wgnu-folding-constant]`): every warning has one, and so
    /// has an error clang makes of a warning by default
    /// (`implicit-function-declaration`).
    pub flag: Option<&'static str>,
    /// The notes clang gives with it, in order (`'f' declared here`).
    pub notes: Vec<Error>,
}

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    /// An error after which clang gives up the compilation (`'x.h' file
    /// not found`): `fatal error:`.
    Fatal,
    Warning,
    /// What follows an error or a warning, to explain it.
    Note,
}

impl Error {
    pub fn new(pos: Pos, msg: impl Into<String>) -> Self {
        Error {
            pos,
            msg: msg.into(),
            file: None,
            severity: Severity::Error,
            flag: None,
            notes: Vec::new(),
        }
    }

    /// A warning, of clang's warning flag `flag`.
    pub fn warning(pos: Pos, msg: impl Into<String>, flag: &'static str) -> Self {
        Error {
            severity: Severity::Warning,
            flag: Some(flag),
            ..Error::new(pos, msg)
        }
    }

    /// A warning clang gives with no flag (`character constant too long
    /// for its type`).
    pub fn warning_unflagged(pos: Pos, msg: impl Into<String>) -> Self {
        Error {
            severity: Severity::Warning,
            ..Error::new(pos, msg)
        }
    }

    /// A fatal error: clang stops there.
    pub fn fatal(pos: Pos, msg: impl Into<String>) -> Self {
        Error {
            severity: Severity::Fatal,
            ..Error::new(pos, msg)
        }
    }

    /// The error, with clang's flag `flag`: one of the warnings clang makes
    /// an error by default.
    pub fn flagged(self, flag: &'static str) -> Self {
        Error {
            flag: Some(flag),
            ..self
        }
    }

    /// The diagnostic, with a note at `pos` after the ones it has.
    pub fn note(mut self, pos: Pos, msg: impl Into<String>) -> Self {
        self.notes.push(Error {
            severity: Severity::Note,
            ..Error::new(pos, msg)
        });
        self
    }

    pub fn is_warning(&self) -> bool {
        self.severity == Severity::Warning
    }

    /// Formats the diagnostic the way compilers and editors expect, as
    /// clang does: `file.c:line:col: error: message`, or
    /// `file.c:line:col: warning: message [-Wflag]`, where `file` names the
    /// file being compiled (one in a header names the header instead), then
    /// a line `file.c:line:col: note: message` for each note.
    pub fn render(&self, file: &str) -> String {
        let mut lines = vec![self.first_line(file)];
        lines.extend(self.notes.iter().map(|n| n.first_line(file)));
        lines.join("\n")
    }

    /// The diagnostic's own line, without its notes.
    pub fn first_line(&self, file: &str) -> String {
        let file = self.file.as_deref().unwrap_or(file);
        let what = match self.severity {
            Severity::Error => "error",
            Severity::Fatal => "fatal error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        let flag = self.flag.map(|f| format!(" [-W{f}]")).unwrap_or_default();
        if self.pos.line > 0 {
            format!("{file}:{}: {what}: {}{flag}", self.pos, self.msg)
        } else {
            format!("{file}: {what}: {}{flag}", self.msg)
        }
    }
}

impl fmt::Display for Error {
    /// `line:col: message`, after the file's name when it is a header's:
    /// the message alone, without its notes.
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
    preprocess::preprocess(name, src, opts, load)
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
/// preprocessor goes on after an error where clang does, the parser stops
/// at the first; the checker reports every error it finds. Errors in
/// headers have their [`Error::file`] set.
pub fn parse_with(name: &str, src: &str, opts: &Options, load: Loader) -> Result<ast::Program, Vec<Error>> {
    let pp = preprocess::preprocess_recovering(name, src, opts, load)?;
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
    let pp = preprocess::preprocess_recovering(name, src, opts, load)?;
    let debug = opts.debug.as_ref().map(|d| (d, pp.files.as_slice()));
    let prog = check(&pp, opts.xlen).map_err(|errors| pp.name_files(errors))?;
    let warnings = pp.name_files(prog.warnings.clone());
    match codegen::generate_with(&prog, debug) {
        Ok(asm) => Ok((asm, warnings)),
        Err(e) => Err(warnings.into_iter().chain(pp.name_files(vec![e])).collect()),
    }
}

/// The front end after the preprocessor: tokens, syntax tree, checks. The
/// errors come with the warnings, the preprocessor's among them, in
/// clang's order ([`in_clangs_order`], [`preprocess::Output::merge`]).
fn check(pp: &preprocess::Output, xlen: types::Xlen) -> Result<ast::Program, Vec<Error>> {
    // The lexer's diagnostics: clang goes on after an error in a literal,
    // or a token that is none, and so does merlin.
    let (toks, lexed) = lexer::finish(pp.tokens.clone(), pp.end, xlen).map_err(|e| pp.merge(e))?;
    // The parser's warnings (it stops at its first error) come as it reads,
    // as the lexer's do; sema's after them.
    // Where the errors before sema's are, which leave out clang's analysis
    // of the functions done after them.
    let earlier: Vec<Pos> = pp
        .error_points()
        .chain(lexed.iter().filter(|e| !e.is_warning()).map(|e| e.pos))
        .collect();
    let (prog, parsed, checked) = match parser::parse_salvaging(&toks, xlen) {
        Ok(mut prog) => {
            prog.written.macros = pp.macros.clone();
            let parsed = std::mem::take(&mut prog.warnings);
            let checked = match sema::check_after(&mut prog, &earlier) {
                Ok(()) => std::mem::take(&mut prog.warnings),
                Err(all) => all,
            };
            (Some(prog), parsed, checked)
        }
        Err(salvaged) => {
            let (e, mut partial) = *salvaged;
            partial.written.macros = pp.macros.clone();
            let parsed = std::mem::take(&mut partial.warnings);
            // What came before a syntax error is checked as clang checks it
            // as it parses, errors too; the end of the function it is in,
            // and of the file, come after it.
            let (mut checked, after) = sema::check_cut(&mut partial, &earlier, e.pos);
            checked.push(e);
            checked.extend(after);
            (None, parsed, checked)
        }
    };
    let read = in_clangs_order(lexed, parsed);
    let diagnostics = pp.merge(in_clangs_order(read, checked));
    match prog {
        Some(mut prog) if diagnostics.iter().all(Error::is_warning) => {
            prog.warnings = diagnostics;
            Ok(prog)
        }
        _ => Err(diagnostics),
    }
}

/// The diagnostics of the lexer and those of the parser and sema, in the
/// order clang gives them: clang lexes as it parses, so a token's
/// diagnostic comes among those of the code around it, by its position,
/// while the parser's and sema's keep the order they were found in (which
/// is not always that of their positions: clang reports a loop's empty
/// body at the end of its block, say).
fn in_clangs_order(lexed: Vec<Error>, checked: Vec<Error>) -> Vec<Error> {
    let mut out = Vec::with_capacity(lexed.len() + checked.len());
    let mut lexed = lexed.into_iter().peekable();
    for d in checked {
        while let Some(l) = lexed.next_if(|l| l.pos <= d.pos) {
            out.push(l);
        }
        out.push(d);
    }
    out.extend(lexed);
    out
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
