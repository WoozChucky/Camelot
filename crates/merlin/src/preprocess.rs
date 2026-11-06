//! # The preprocessor (C99 6.10)
//!
//! Works on the lexer's preprocessing tokens ([`PpToken`]) and hands the
//! parser the tokens of the whole translation unit, every directive carried
//! out and every macro replaced. It does what C99 asks, and what clang does
//! where C leaves the behavior undefined: a directive inside the arguments
//! of a macro call is carried out (but `#include` and `#pragma`, an error
//! there), and a `defined` that a macro's expansion produces in `#if` is
//! evaluated, with clang's warning.
//!
//! ## Diagnostics
//!
//! They are clang's, in its words, at its positions, and clang goes on
//! after most of them, so merlin does too: a directive with an error is
//! dropped (a macro is not defined, an `#include` not read, a `#line` not
//! applied; an `#if` whose condition has one is false), and a macro call
//! with the wrong number of arguments leaves the macro's name as it is.
//! Only a header that is not found stops it (clang's fatal error).
//! [`preprocess_recovering`] hands on the tokens with the diagnostics so
//! that the parser goes on too; [`Output::merge`] puts its diagnostics and
//! the parser's in clang's order.
//!
//! * `#include "file"` looks in the including file's directory, then in
//!   each `-I` directory in order; `#include <file>` only in the `-I`
//!   directories (merlin has no system headers; one found as `"file"` would
//!   be is an error, and included, as clang does). The name may also come
//!   from a macro (`#include HEADER`). Includes nest up to 200 deep.
//!   `#pragma once` makes a file include only once; include guards
//!   (`#ifndef X` / `#define X` ... `#endif`) work like in any compiler.
//!   A header found next to the file that includes it is named by that
//!   file's directory, `.` for one named without a directory, as clang
//!   names it: `./sub/s.h` next to `b.c` (`__FILE__`, errors, line tables).
//! * Any other pragma means nothing to merlin (C says to ignore it) and,
//!   as in clang, is passed on, `#pragma` and its tokens unreplaced, on a
//!   line of its own ([`PpKind::Directive`], which the parser skips), and
//!   so is `#ident "string"` (`#sccs`), macros replaced.
//! * `#define NAME tokens` and `#define F(a, b) tokens` (with `...` and
//!   `__VA_ARGS__`, or GNU's `args...`), `#undef`. Defining a macro again
//!   differently (C99 6.10.3p2 asks for the same definition) is clang's
//!   warning, and the new definition holds; so is defining or undefining
//!   `__FILE__`, `__LINE__` or a predefined `__STDC...` macro.
//! * `#if`, `#elif` with `defined NAME` / `defined(NAME)` and integer
//!   constant expressions in 64 bits (C's `intmax_t`, `uintmax_t`), where any
//!   identifier left after macro replacement is 0, evaluated as clang
//!   evaluates them ([`Eval`]); `#ifdef`, `#ifndef`, `#else`, `#endif`.
//!   Groups that are skipped are only scanned for the directives that nest,
//!   so they may hold anything that lexes (an apostrophe in a sentence too).
//! * `#error` is an error with the rest of its line as written, `#warning`
//!   a warning; `#line N "name"` and GNU's line marker `# N "name" flags`
//!   renumber the lines that follow; `#` alone does nothing. Any other
//!   directive is an error.
//! * `__FILE__` and `__LINE__`, and a few predefined macros ([`predefined`]).
//!
//! ## Macro replacement
//!
//! Replacement follows Dave Prosser's algorithm, which is how C99's rules
//! (6.10.3.4 *rescanning and further replacement*) were written down: every
//! token carries a *hide set*, the names of the macros whose expansion
//! produced it. A name is replaced only if it is not in its own hide set,
//! so a macro never expands inside its own expansion (`#define x x + 1`
//! makes `x` into `x + 1`, once), yet the result is rescanned together with
//! the rest of the source, so `#define f(a) a + g` followed by `f(1)(2)`
//! calls `g` with `(2)`.
//!
//! * An object-like macro `M`: its body, each token's hide set extended by
//!   that of `M`'s token plus `M`, goes back in front of the input.
//! * A function-like macro `F` followed by `(`: the arguments are collected
//!   up to the matching `)` (across lines). In the body, a parameter after
//!   `#` becomes its argument's spelling as a string literal; a parameter
//!   next to `##` becomes its argument as written; any other parameter its
//!   argument *fully macro-replaced on its own*. Then `##` pastes the tokens
//!   on either side into one (an empty argument there is a *placemarker*,
//!   which pasting drops). The hide set is the intersection of `F`'s and
//!   the `)`'s, plus `F`: what was hidden when the name was read and is
//!   still hidden at the `)`.
//!
//! Tokens of the body take the position of the macro's name where it was
//! used, so errors in the expansion point there; tokens of an argument keep
//! their own.
//!
//! ## Assembly (`-x assembler-with-cpp`)
//!
//! `.S` files are assembly run through this preprocessor first, as clang
//! and GCC run them through theirs ([`Lang::AssemblerWithCpp`]), for
//! `#include`, `#define` and Linux's `REG_L`/`REG_S` macros. Everything
//! above holds, with what clang changes for assembly:
//!
//! * `#` at the start of a line followed by anything but a directive's name
//!   (`# a comment`, `# 2^64 - 1`, `#` then a number too, which C would read
//!   as a line marker) is no directive: the line passes through, macros
//!   replaced in it, as assembly comments must. A line with only `#` is
//!   still the null directive, and a directive's name always means the
//!   directive, so a comment may not start with `# if` or `# define`.
//!   C's comments are still comments: a `/*` in such a line (a glob such
//!   as `examples/*/`) starts one that runs to the next `*/`, lines of
//!   code in between, as in clang.
//! * `__ASSEMBLER__` is defined, and `__STDC_VERSION__` and `__STDC_HOSTED__`
//!   are what clang gives a `.S` file ([`predefined`]).
//! * Pasting two tokens that do not form one (`. ## name`) is no error: they
//!   stay two, written with nothing between them, unless the second is a
//!   token of the macro's body with a space before it (`call f ## @plt`
//!   is `call foo @plt`, as in clang). And `#` in a function-like macro's
//!   body may stand before anything (`# y` is two tokens of the expansion).
//! * Trigraphs are characters, and what is no C token (`$`, `@`, `\`)
//!   passes through ([`lexer::lex_pp_asm`]).
//!
//! The output is text for the assembler ([`Output::print_asm`]), written as
//! clang writes it, so that `merlin -E` and `clang -E` print the same thing
//! for a `.S` file: every token on the line it came from, with blank lines
//! or a line marker (`# 120 "file.S"`) to keep the numbering, and markers
//! where an `#include`d file starts (`# 1 "dir/asm.h" 1`) and ends (`# 5
//! "file.S" 2`); see [`Output::print_asm`] for the spacing. merlin-as reads
//! the markers as the comments they are, and [`source_line`] maps a line of
//! the output back to the file and line it came from, for its errors.

use crate::lexer::{self, PpKind, PpToken};
use crate::md5::md5;
use crate::{Error, Pos, Severity};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

/// A `-D` or `-U` option, as written after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CmdMacro {
    /// `-D NAME` (`#define NAME 1`), `-D NAME=VALUE` (`#define NAME VALUE`),
    /// `-D 'F(x)=VALUE'` (`#define F(x) VALUE`).
    Define(String),
    /// `-U NAME`: `#undef NAME`.
    Undef(String),
}

/// What the command line tells the compiler: the preprocessor's options,
/// the target and `-g`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// `-I`: where `#include` looks, in order (after the including file's
    /// own directory, for `#include "..."`).
    pub include_dirs: Vec<PathBuf>,
    /// `-D` and `-U`, in command-line order.
    pub macros: Vec<CmdMacro>,
    /// `-g`: debug information, for the code generator.
    pub debug: Option<crate::DebugInfo>,
    /// `-x`: what the input is.
    pub lang: Lang,
    /// Whether preprocessed assembly has line markers: `-P` turns them off,
    /// for text that is no assembly, such as a linker script
    /// ([`Output::print_asm`]).
    pub line_markers: bool,
}

impl Default for Options {
    /// No `-I`, `-D`, `-g` or `-P`, C.
    fn default() -> Options {
        Options {
            include_dirs: Vec::new(),
            macros: Vec::new(),
            debug: None,
            lang: Lang::C,
            line_markers: true,
        }
    }
}

/// The language of the input, as clang's `-x` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    /// `-x c`: C, compiled (`.c` files).
    C,
    /// `-x assembler-with-cpp`: assembly, only preprocessed (`.S` files);
    /// see the module's documentation.
    AssemblerWithCpp,
}

impl Lang {
    /// The language of `-x NAME`.
    pub fn from_name(name: &str) -> Option<Lang> {
        match name {
            "c" => Some(Lang::C),
            "assembler-with-cpp" => Some(Lang::AssemblerWithCpp),
            _ => None,
        }
    }

    /// The language clang gives a file by its name: `.S` (and `.sx`) are
    /// assembler-with-cpp, anything else C.
    pub fn of_file(name: &str) -> Lang {
        if name.ends_with(".S") || name.ends_with(".sx") {
            Lang::AssemblerWithCpp
        } else {
            Lang::C
        }
    }
}

/// Reads the file at a path for `#include`, or `None` if there is none.
pub type Loader<'a> = &'a dyn Fn(&Path) -> Option<String>;

/// Macros every translation unit starts with. merlin compiles freestanding
/// C99 for RISC-V, LP64, with plain `char` unsigned, so it says what clang
/// says for `--target=riscv64 -mabi=lp64` (`-ffreestanding -std=c99`)
/// about those, and `__merlin__` says who is compiling. The ISA is
/// `rv64ima_zicsr_zifencei` ([`crate::MARCH`]), and the code model
/// `medany`, the one merlin's code follows (globals and
/// calls through `auipc`, pc-relative, what clang's `-mcmodel=medany`
/// writes): clang's macros for that `-march` and `-mcmodel=medany`.
///
/// Assembly (`lang`) also has `__ASSEMBLER__`, and the C standard clang
/// gives a `.S` file when told none (as the sources' clang commands build
/// them): C17, hosted.
pub fn predefined(lang: Lang) -> Vec<(&'static str, &'static str)> {
    let mut macros = match lang {
        Lang::C => vec![
            ("__STDC__", "1"),
            ("__STDC_VERSION__", "199901L"),
            ("__STDC_HOSTED__", "0"),
        ],
        Lang::AssemblerWithCpp => vec![
            ("__STDC__", "1"),
            ("__STDC_VERSION__", "201710L"),
            ("__STDC_HOSTED__", "1"),
            ("__ASSEMBLER__", "1"),
        ],
    };
    macros.extend([("__merlin__", "1"), ("__ELF__", "1"), ("__riscv", "1")]);
    // The ISA, as clang 23 says it: each extension as `__riscv_<name>` with
    // its version (2.1 is 2001000; M brings Zmmul, A brings Zaamo and
    // Zalrsc), and what they add up to (`mul`, `div`, `atomic`...);
    // soft-float, no misaligned accesses.
    macros.extend([
        ("__riscv_i", "2001000"),
        ("__riscv_m", "2000000"),
        ("__riscv_a", "2001000"),
        ("__riscv_zicsr", "2000000"),
        ("__riscv_zifencei", "2000000"),
        ("__riscv_zmmul", "1000000"),
        ("__riscv_zaamo", "1000000"),
        ("__riscv_zalrsc", "1000000"),
        ("__riscv_mul", "1"),
        ("__riscv_div", "1"),
        ("__riscv_muldiv", "1"),
        ("__riscv_atomic", "1"),
        ("__riscv_arch_test", "1"),
        ("__riscv_float_abi_soft", "1"),
        ("__riscv_misaligned_avoid", "1"),
        ("__riscv_v_intrinsic", "1000000"),
        ("__riscv_cmodel_medany", "1"),
    ]);
    macros.extend([("__riscv_xlen", "64"), ("_LP64", "1"), ("__LP64__", "1")]);
    // The sizes and limits of the integer types, and the types <stddef.h>
    // and <stdint.h> are made of, spelled as clang spells them: LP64 builds
    // size_t, ptrdiff_t, intptr_t and int64_t from long.
    macros.extend([
        ("__CHAR_BIT__", "8"),
        ("__CHAR_UNSIGNED__", "1"),
        ("__SIZEOF_SHORT__", "2"),
        ("__SIZEOF_INT__", "4"),
        ("__SIZEOF_LONG_LONG__", "8"),
        ("__SCHAR_MAX__", "127"),
        ("__SHRT_MAX__", "32767"),
        ("__INT_MAX__", "2147483647"),
        ("__LONG_LONG_MAX__", "9223372036854775807LL"),
        ("__INT8_TYPE__", "signed char"),
        ("__INT16_TYPE__", "short"),
        ("__INT32_TYPE__", "int"),
        ("__UINT8_TYPE__", "unsigned char"),
        ("__UINT16_TYPE__", "unsigned short"),
        ("__UINT32_TYPE__", "unsigned int"),
        ("__INT32_MAX__", "2147483647"),
        ("__UINT32_MAX__", "4294967295U"),
    ]);
    macros.extend([
        ("__SIZEOF_LONG__", "8"),
        ("__SIZEOF_POINTER__", "8"),
        ("__SIZEOF_SIZE_T__", "8"),
        ("__SIZEOF_PTRDIFF_T__", "8"),
        ("__LONG_MAX__", "9223372036854775807L"),
        ("__SIZE_TYPE__", "long unsigned int"),
        ("__PTRDIFF_TYPE__", "long int"),
        ("__INTPTR_TYPE__", "long int"),
        ("__UINTPTR_TYPE__", "long unsigned int"),
        ("__INT64_TYPE__", "long int"),
        ("__UINT64_TYPE__", "long unsigned int"),
        ("__SIZE_MAX__", "18446744073709551615UL"),
        ("__PTRDIFF_MAX__", "9223372036854775807L"),
        ("__INTPTR_MAX__", "9223372036854775807L"),
        ("__UINTPTR_MAX__", "18446744073709551615UL"),
        ("__INT64_MAX__", "9223372036854775807L"),
        ("__UINT64_MAX__", "18446744073709551615UL"),
    ]);
    macros
}

/// How deeply `#include`s may nest (clang's limit too).
const MAX_INCLUDE_DEPTH: usize = 200;

/// Every directive's name clang knows. In assembly, `#` and any other word
/// starts no directive.
pub const DIRECTIVES: &[&str] = &[
    "define",
    "undef",
    "include",
    "include_next",
    "import",
    "if",
    "ifdef",
    "ifndef",
    "elif",
    "elifdef",
    "elifndef",
    "else",
    "endif",
    "line",
    "error",
    "warning",
    "pragma",
    "ident",
    "sccs",
    "assert",
    "unassert",
    "embed",
    "__include_macros",
];

/// The preprocessed translation unit.
#[derive(Debug)]
pub struct Output {
    /// Every token, macros replaced, directives gone.
    pub tokens: Vec<PpToken>,
    /// Where a macro's expansion is: the positions of the tokens it made
    /// (where it was used), which clang's warnings of what is written
    /// leave out ([`crate::ast::Written`]).
    pub macros: std::collections::HashSet<Pos>,
    /// The end of the file being compiled (where end of file is reported).
    pub end: Pos,
    /// The name of each file, indexed by [`Pos::file`]: 0 is the file being
    /// compiled, then headers (named by the directory they were found in
    /// and the name in the `#include`) and the predefined and command-line
    /// macros (`<built-in>`, `<command line>`).
    pub files: Vec<String>,
    /// The MD5 checksum of each file, by [`Pos::file`] as `files`: of its
    /// bytes as read, for the debug information's `.file` directives. A
    /// name `#line` gives has none, as in clang.
    pub checksums: Vec<Option<[u8; 16]>>,
    /// How each token is laid out, for [`Output::print_asm`].
    layout: Vec<Layout>,
    /// Where files start and end and `#line` renumbers, after how many tokens.
    marks: Vec<(usize, Mark)>,
    /// The diagnostics of preprocessing (`'N' macro redefined`), in order:
    /// warnings, and from [`preprocess_recovering`] errors too.
    pub diagnostics: Vec<Error>,
    /// For each diagnostic, where the last token that came out before it
    /// is ([`Pos::default`] for none): see [`Output::merge`].
    after: Vec<Pos>,
}

/// Where a token of the output goes, for [`Output::print_asm`].
#[derive(Clone, Copy, Debug)]
struct Layout {
    /// It starts a line (a macro's expansion starts one where its name did).
    bol: bool,
    /// The line and column it is shown at: its own, or where the outermost
    /// macro that produced it was used.
    at: Pos,
    /// Where its characters are written (see [`PTok::spelled`]).
    spelled: Option<(Pos, Pos)>,
    /// Whitespace before it on its line (see [`PTok::lead`]).
    lead: bool,
}

/// A change of file in the output: a line marker.
#[derive(Clone, Copy, Debug)]
enum Mark {
    /// An `#include`d file starts; the `#include` is on `line` of the file
    /// that includes it.
    Enter { line: u32, file: u32 },
    /// It has ended: the file that included it goes on at `line`.
    Exit { line: u32, file: u32 },
    /// `#line`: the next line is `line` of `file`.
    Rename { line: u32, file: u32 },
}

impl Output {
    /// Of each error of preprocessing, where the last token that came out
    /// before it is (see [`Output::merge`]): clang finds it when its parser
    /// reads the token after that one.
    pub fn error_points(&self) -> impl Iterator<Item = Pos> + '_ {
        self.diagnostics
            .iter()
            .zip(&self.after)
            .filter(|(d, _)| !d.is_warning())
            .map(|(_, &after)| after)
    }

    /// Names the file of every error not in the file being compiled.
    pub fn name_files(&self, mut errors: Vec<Error>) -> Vec<Error> {
        name_files(&self.files, &mut errors);
        errors
    }

    /// The preprocessor's diagnostics and those of what reads its tokens
    /// (`later`: the lexer's, the parser's, the checker's, in clang's order
    /// among themselves), in the order clang gives them. clang preprocesses
    /// as its parser asks for tokens, so a diagnostic of the preprocessor
    /// comes when the parser reads the token after it; and its parser
    /// reports an error at a token once it has read the next one too. So
    /// each of the preprocessor's comes before what is reported at or after
    /// the last token that came out before it: `F(1)` with too few
    /// arguments is an error at the `)`, then one at `F`, which the parser
    /// takes for a type's name.
    pub fn merge(&self, later: Vec<Error>) -> Vec<Error> {
        let mut later = later.into_iter().peekable();
        let mut out = Vec::new();
        for (d, after) in self.diagnostics.iter().zip(&self.after) {
            while let Some(e) = later.next_if(|e| e.pos < *after) {
                out.push(e);
            }
            out.push(d.clone());
        }
        out.extend(later);
        out
    }

    /// The text of preprocessed assembly, for the assembler, as clang's
    /// `-E` writes it (`PrintPreprocessedOutput.cpp`): a line marker
    /// `# 1 "file.S"` first, then each token on the line it came from (a
    /// macro's expansion on the line of the macro's name):
    ///
    /// * To get to the line of a token that starts one, a newline ends the
    ///   current line, and the blank lines in between are written out if
    ///   there are at most 8 lines to go; past that (or going back) a line
    ///   marker `# N "file"` says where the next line is.
    /// * The first token of a new line is indented to its column (`#` in
    ///   column 1 gets one space, so that it reads as no directive), and so
    ///   is the first token of a file, on the line its marker gives. After
    ///   the marker of `#line` or of the end of an `#include`d file, though,
    ///   a token on the marker's line gets one space if it had any before
    ///   it, as on any other line.
    /// * Between tokens on a line: one space where the source had
    ///   whitespace, and one where two tokens that were not written next to
    ///   each other would otherwise read as something else ([`avoid_concat`]).
    /// * Entering an `#include`d file writes `# 1 "name" 1`, leaving it
    ///   `# N "file" 2` with the line after the `#include`, and `#line` a
    ///   marker for the line it gives.
    ///
    /// clang also writes markers for its `<built-in>` and `<command line>`
    /// macros, which merlin leaves out.
    ///
    /// Without `line_markers` (`-P`, as clang's `-E -P` writes it, for text
    /// that is no assembly, such as a linker script) there are no markers
    /// and no blank lines: a token that starts a line goes on a new line if
    /// the current one has tokens, or if it is the very next line (one
    /// newline); otherwise it goes on as if on the same line. Only the first
    /// token on a line that a newline started is indented to its column.
    pub fn print_asm(&self, line_markers: bool) -> String {
        let mut p = Printer {
            out: String::new(),
            line: 1,
            file: 0,
            files: &self.files,
            started: false,
            entered: true,
            markers: line_markers,
        };
        p.line_marker(1, "");
        let mut marks = self.marks.iter().peekable();
        let (mut prev, mut prev_prev): (Option<usize>, Option<usize>) = (None, None);
        for (i, (t, l)) in self.tokens.iter().zip(&self.layout).enumerate() {
            while let Some((_, mark)) = marks.next_if(|(at, _)| *at <= i) {
                p.mark(*mark);
            }
            // A token that starts a line is indented if the output moved to a
            // new line for it, or if it is on the first line of a file that
            // was just entered (clang's `IsFirstInFile`): there the output is
            // at the start of a line already, after the marker (or nothing).
            let new_line = l.bol && {
                let moved = p.move_to_line(l.at.line);
                let first_in_file = std::mem::take(&mut p.entered) && l.at.line == 1;
                moved || first_in_file
            };
            if new_line {
                let mut col = l.at.col;
                if col == 1 && l.lead {
                    col = 2;
                }
                if col <= 1 && t.is_punct("#") {
                    p.out.push(' ');
                }
                for _ in 1..col {
                    p.out.push(' ');
                }
            } else if l.lead
                || (p.started
                    && prev.is_some_and(|k| {
                        avoid_concat(
                            prev_prev.map(|j| (&self.tokens[j], &self.layout[j])),
                            (&self.tokens[k], &self.layout[k]),
                            (t, l),
                        )
                    }))
            {
                p.out.push(' ');
            }
            p.out.push_str(&t.text);
            p.started = true;
            p.entered = false;
            prev_prev = prev;
            prev = Some(i);
        }
        for (_, mark) in marks {
            p.mark(*mark);
        }
        // clang ends the output with a newline whatever came last, which
        // after a marker (or nothing) is an empty line.
        p.out.push('\n');
        p.out
    }
}

/// The state of [`Output::print_asm`].
struct Printer<'a> {
    out: String,
    /// The line of the file being written that the output is on.
    line: u32,
    /// That file.
    file: u32,
    files: &'a [String],
    /// Tokens have been written on the current line.
    started: bool,
    /// No token has been written since a file was entered: the next one, if
    /// it is on the file's first line, is indented, even on the line of the
    /// marker.
    entered: bool,
    /// Line markers are written (no `-P`).
    markers: bool,
}

impl Printer<'_> {
    /// `# N "file"` and `extra` (` 1` entering a file, ` 2` leaving one),
    /// on a line of its own: the next line is line `line`.
    /// Without markers (`-P`), only the newline that ends a line with tokens
    /// (clang's `startNewLineIfNeeded`).
    fn line_marker(&mut self, line: u32, extra: &str) {
        if std::mem::take(&mut self.started) {
            self.out.push('\n');
        }
        if !self.markers {
            return;
        }
        let name = escape(&self.files[self.file as usize]);
        self.out.push_str(&format!("# {line} \"{name}\"{extra}\n"));
    }

    /// Goes to line `line` of the current file (clang's `MoveToLine`): one
    /// newline to the next line, newlines up to 8 lines ahead, else a line
    /// marker; without markers (`-P`), one newline if the current line has
    /// tokens, else none. Whether the output is now at the start of a line.
    fn move_to_line(&mut self, line: u32) -> bool {
        let moved = if self.line == line {
            false
        } else if line == self.line + 1 {
            self.out.push('\n');
            true
        } else if !self.markers {
            if self.started {
                self.out.push('\n');
            }
            self.started
        } else {
            if line > self.line && line - self.line <= 8 {
                for _ in self.line..line {
                    self.out.push('\n');
                }
            } else {
                self.line_marker(line, "");
            }
            true
        };
        if moved {
            self.started = false;
        }
        self.line = line;
        moved
    }

    fn mark(&mut self, mark: Mark) {
        match mark {
            Mark::Enter { line, file } => {
                self.move_to_line(line);
                self.file = file;
                self.line = 1;
                self.line_marker(1, " 1");
                self.entered = true;
            }
            Mark::Exit { line, file } => {
                self.file = file;
                self.line = line;
                self.line_marker(line, " 2");
                self.entered = false;
            }
            Mark::Rename { line, file } => {
                self.file = file;
                self.line = line;
                self.line_marker(line, "");
                self.entered = false;
            }
        }
    }
}

/// Whether `tok` needs a space after `prev` (and `prev_prev` before it),
/// though the source had none: clang's `TokenConcatenation::AvoidConcat`,
/// for C. Never for two tokens written next to each other (in the source
/// or in one macro's body), which read as two already; otherwise when
/// writing them together could read as other tokens, judged by `prev`'s
/// kind and the first character of `tok`, as clang judges it: `-` then
/// `-` or `>`, a number then a letter, a digit, `_`, `.`, `+` or `-`, an
/// identifier then an identifier or a number, an operator then `=`...
fn avoid_concat(
    prev_prev: Option<(&PpToken, &Layout)>,
    (prev, pl): (&PpToken, &Layout),
    (tok, tl): (&PpToken, &Layout),
) -> bool {
    if let (Some((_, pend)), Some((tpos, _))) = (pl.spelled, tl.spelled) {
        if pend == tpos {
            return false;
        }
    }
    let first = tok.text.chars().next().unwrap_or(' ');
    let punct = |p: &PpToken| match p.kind {
        PpKind::Punct(s) => s,
        _ => "",
    };
    let (p, t) = (punct(prev), punct(tok));
    // `&=`, `+=`, `==`, `<<=`...: an operator then `=` or `==`.
    if matches!(
        p,
        "&" | "+" | "-" | "/" | "<" | ">" | "|" | "%" | "*" | "!" | "<<" | ">>" | "^" | "="
    ) && matches!(t, "=" | "==")
    {
        return true;
    }
    match prev.kind {
        PpKind::Ident => match tok.kind {
            PpKind::Number => first != '.',
            PpKind::Ident => true,
            // L"x", u8"x": a string's prefix.
            PpKind::Str | PpKind::Char => matches!(prev.text.as_str(), "L" | "u" | "U" | "u8"),
            _ => false,
        },
        PpKind::Number => first.is_ascii_alphanumeric() || matches!(first, '_' | '.' | '+' | '-'),
        PpKind::Punct(_) => match p {
            "." => {
                (first == '.' && prev_prev.is_some_and(|(pp, _)| pp.is_punct("."))) || first.is_ascii_digit()
            }
            "&" => first == '&',
            "+" => first == '+',
            "-" => first == '-' || first == '>',
            "/" => first == '*' || first == '/',
            "<" => matches!(first, '<' | ':' | '%'),
            ">" => first == '>',
            "|" => first == '|',
            "%" => first == '>' || first == ':',
            ":" => first == '>',
            "#" => matches!(first, '#' | '@' | '%'),
            _ => false,
        },
        _ => false,
    }
}

/// The file and line a line of [`Output::print_asm`]'s text comes from,
/// as its line markers say (line numbers from 1); `None` for a marker's
/// own line, or before the first.
pub fn source_line(text: &str, line: usize) -> Option<(String, usize)> {
    let mut at: Option<(String, usize)> = None;
    for (n, l) in text.lines().enumerate() {
        if let Some(marker) = line_marker(l) {
            if n + 1 == line {
                return None;
            }
            at = Some(marker);
            continue;
        }
        if n + 1 == line {
            return at;
        }
        if let Some((_, k)) = &mut at {
            *k += 1;
        }
    }
    None
}

/// A line marker's line and file: `# 12 "file.S"` and any flags after it.
fn line_marker(l: &str) -> Option<(String, usize)> {
    let rest = l.strip_prefix("# ")?;
    let (n, rest) = rest.split_once(' ')?;
    let n: usize = n.parse().ok()?;
    let rest = rest.strip_prefix('"')?;
    let mut name = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => name.push(chars.next()?),
            c => name.push(c),
        }
    }
    Some((name, n))
}

fn name_files(files: &[String], errors: &mut [Error]) {
    for e in errors {
        if e.pos.file != 0 && e.file.is_none() {
            e.file = files.get(e.pos.file as usize).cloned();
        }
        if e.msg.contains(crate::FILE_MARK) {
            e.msg = name_places(files, &e.msg);
        }
        name_files(files, &mut e.notes);
    }
}

/// `msg` with the file of each place in it ([`crate::place`]) named.
fn name_places(files: &[String], msg: &str) -> String {
    let mut parts = msg.split(crate::FILE_MARK);
    let mut out = parts.next().unwrap_or_default().to_string();
    while let (Some(file), Some(rest)) = (parts.next(), parts.next()) {
        let name = file.parse::<usize>().ok().and_then(|n| files.get(n));
        out.push_str(name.map_or("", String::as_str));
        out.push(':');
        out.push_str(rest);
    }
    out
}

/// Preprocesses the source `src` of the file `name` (its name for
/// `__FILE__` and errors; `#include "..."` looks next to it), for what
/// wants no tokens if there is an error (`merlin -E`, assembly): `Err` has
/// every diagnostic, in order, if one is an error; [`Output::diagnostics`]
/// has the warnings. A diagnostic in a header has its [`Error::file`] set.
pub fn preprocess(name: &str, src: &str, opts: &Options, load: Loader) -> Result<Output, Vec<Error>> {
    let out = preprocess_recovering(name, src, opts, load)?;
    if out.diagnostics.iter().any(|d| !d.is_warning()) {
        return Err(out.diagnostics);
    }
    Ok(out)
}

/// Preprocesses as [`preprocess`] does, going on after an error as clang
/// does (see the module's documentation), for the parser to go on too: the
/// tokens come with the errors and warnings in [`Output::diagnostics`].
/// Only what stops clang stops it (a header not found, and in merlin a
/// lexical error in the file being compiled): `Err` has the diagnostics
/// before it, then that one.
pub fn preprocess_recovering(
    name: &str,
    src: &str,
    opts: &Options,
    load: Loader,
) -> Result<Output, Vec<Error>> {
    let mut pp = Pp {
        opts,
        load,
        files: vec![File {
            name: name.to_string(),
            path: normalize(Path::new(name)),
            md5: Some(md5(src.as_bytes())),
        }],
        macros: HashMap::new(),
        once: HashSet::new(),
        stack: Vec::new(),
        pending: VecDeque::new(),
        isolated: false,
        conds: Vec::new(),
        builtin_file: 0,
        asm: opts.lang == Lang::AssemblerWithCpp,
        guards: HashMap::new(),
        emitted: 0,
        last_out: Pos::default(),
        marks: Vec::new(),
        carry: (false, false, false),
        skipping: false,
        diagnostics: Vec::new(),
        after: Vec::new(),
    };
    let result = pp.run(src);
    let mut diagnostics = std::mem::take(&mut pp.diagnostics);
    let checksums = pp.files.iter().map(|f| f.md5).collect();
    let files: Vec<String> = pp.files.into_iter().map(|f| f.name).collect();
    name_files(&files, &mut diagnostics);
    match result {
        Ok((toks, end)) => {
            let layout = toks
                .iter()
                .map(|t| Layout {
                    bol: t.t.bol,
                    at: t.site.unwrap_or(t.t.pos),
                    spelled: t.spelled,
                    lead: t.lead,
                })
                .collect();
            let macros = toks
                .iter()
                .filter(|t| t.site.is_some())
                .map(|t| t.t.pos)
                .collect();
            Ok(Output {
                macros,
                tokens: toks.into_iter().map(|t| t.t).collect(),
                end,
                files,
                checksums,
                layout,
                marks: pp.marks,
                diagnostics,
                after: pp.after,
            })
        }
        Err(mut e) => {
            name_files(&files, std::slice::from_mut(&mut e));
            diagnostics.push(e);
            Err(diagnostics)
        }
    }
}

/// Writes tokens back as text (`merlin -E`): a line break where the source
/// had one between them (a macro's expansion stays on the line it was
/// used on), a space where it had whitespace or a comment, and a space
/// where two tokens that were not next to each other in the source would
/// read as something else when written together (`-M` with `#define M -`
/// is `- -`, not `--`), so that reading the output again gives the same
/// tokens.
pub fn spell(tokens: &[PpToken]) -> String {
    let mut out = String::new();
    let mut last: Option<(u32, u32, &PpToken)> = None;
    for t in tokens {
        let here = (t.pos.file, t.pos.line);
        match last {
            Some((file, line, prev)) if (file, line) == here => {
                if t.space || merges(prev, t) {
                    out.push(' ');
                }
            }
            Some(_) => out.push('\n'),
            None => {}
        }
        out.push_str(&t.text);
        last = Some((here.0, here.1, t));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Whether `a` and `b`, written with nothing between them, would not read
/// back as the same two tokens (`-` `-` is `--`, `1` `x` is `1x`, `/` `/`
/// starts a comment).
fn merges(a: &PpToken, b: &PpToken) -> bool {
    let text = format!("{}{}", a.text, b.text);
    match lexer::lex_pp(&text, a.pos.file) {
        Ok((toks, _)) => !(toks.len() == 2 && toks[0].text == a.text && toks[1].text == b.text),
        Err(_) => true,
    }
}

/// A lexical form of a path for telling files apart: `.` dropped, `..`
/// applied where it can be, separators made uniform.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// The names of the macros whose expansion produced a token.
type HideSet = Rc<Vec<String>>;

fn hidden(hs: &HideSet, name: &str) -> bool {
    hs.iter().any(|n| n == name)
}

fn hide_union(a: &HideSet, b: &HideSet) -> HideSet {
    if b.is_empty() {
        return a.clone();
    }
    let mut v = (**a).clone();
    for n in b.iter() {
        if !v.contains(n) {
            v.push(n.clone());
        }
    }
    Rc::new(v)
}

fn hide_intersection(a: &HideSet, b: &HideSet) -> HideSet {
    Rc::new(a.iter().filter(|n| b.contains(n)).cloned().collect())
}

/// A token and its hide set, and where it comes from, for
/// [`Output::print_asm`].
#[derive(Clone, Debug)]
struct PTok {
    t: PpToken,
    hide: HideSet,
    /// For a token a macro's expansion produced: where the outermost macro
    /// was used (clang's *expansion location*), the line and column it is
    /// shown at.
    site: Option<Pos>,
    /// Where its characters are written, in a file or in a macro's body;
    /// `None` for one the preprocessor made (`#x`, `a ## b`, `__LINE__`).
    /// Two tokens written next to each other need no space between them.
    spelled: Option<(Pos, Pos)>,
    /// clang's *leading space*: whitespace (or a comment) before it on its
    /// own line, which [`PpToken::space`] also says for a newline. Macro
    /// expansion moves it about as it moves `space`.
    lead: bool,
    /// It is a token of a function-like macro's body (where clang does not
    /// warn of a `defined` in `#if`, see [`Eval::defined`]).
    function_body: bool,
}

impl PTok {
    /// A token as read from a file.
    fn read(t: PpToken) -> PTok {
        let spelled = Some((t.pos, t.end));
        // What comes before the first token of a line is on that line only
        // if it is not in column 1.
        let lead = t.space && (!t.bol || t.pos.col > 1);
        PTok {
            t,
            hide: HideSet::default(),
            site: None,
            spelled,
            lead,
            function_body: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MacroKind {
    Object,
    /// The parameter names, and whether the last one takes the variadic
    /// arguments: `__VA_ARGS__` for `...` (C99), `args` for GNU's `args...`.
    Function {
        params: Vec<String>,
        variadic: bool,
    },
    /// `__FILE__`: the current file's name, as a string literal.
    File,
    /// `__LINE__`: the current line, as a number.
    Line,
}

#[derive(Clone, Debug)]
struct Macro {
    kind: MacroKind,
    body: Vec<PpToken>,
    /// Where it was defined.
    pos: Pos,
}

impl Macro {
    /// Whether two definitions are the same (C99 6.10.3p2): the same
    /// parameters and the same body, token for token, with whitespace
    /// between the same tokens.
    fn same(&self, other: &Macro) -> bool {
        self.kind == other.kind
            && self.body.len() == other.body.len()
            && self
                .body
                .iter()
                .zip(&other.body)
                .enumerate()
                .all(|(i, (a, b))| a.kind == b.kind && a.text == b.text && (i == 0 || a.space == b.space))
    }

    /// The index of the parameter a body token names, if it names one.
    fn param(&self, t: &PpToken) -> Option<usize> {
        let MacroKind::Function { params, .. } = &self.kind else {
            return None;
        };
        let name = t.ident()?;
        params.iter().position(|p| p == name)
    }
}

struct File {
    /// For `__FILE__` and errors.
    name: String,
    /// For `#pragma once` and for finding the files it includes.
    path: PathBuf,
    /// Its contents' MD5 ([`Output::checksums`]).
    md5: Option<[u8; 16]>,
}

/// A file being read.
struct Cursor {
    toks: Vec<PpToken>,
    i: usize,
    /// Its text, for the lines `#error` and `#warning` print as written.
    text: Rc<str>,
    /// Its entry in [`Pp::files`] (`#line "name"` makes another).
    file: u32,
    /// Added to every line number (`#line`).
    line_delta: i64,
    /// How many conditionals were open when it was entered: the ones it
    /// opens must be closed in it.
    conds: usize,
    /// For an `#include`d file: the line the file that includes it goes on
    /// at, after the directive.
    back: Option<u32>,
    /// How many line markers say a file was entered (flag 1) and not left
    /// yet (flag 2): what flag 2 may leave.
    entered_by_markers: u32,
}

/// What reading the arguments of a macro call gives ([`Pp::arguments`]).
enum Call {
    /// The arguments, and the `)` that ends them.
    Args(Vec<Vec<PTok>>, PTok),
    /// Too many or too few: the macro's name stands for itself, as in
    /// clang, and what was read is gone.
    Wrong,
    /// The input ended before the `)`: the call is gone.
    Unterminated,
}

/// An open `#if` (or `#ifdef`, `#ifndef`).
struct Cond {
    pos: Pos,
    /// One of its groups has been taken: the rest are skipped.
    taken: bool,
    /// Its `#else` has been seen.
    seen_else: bool,
}

struct Pp<'a> {
    opts: &'a Options,
    load: Loader<'a>,
    files: Vec<File>,
    macros: HashMap<String, Macro>,
    /// Files marked `#pragma once`.
    once: HashSet<PathBuf>,
    /// The files being read: the one compiled, and the headers it is
    /// including, innermost last.
    stack: Vec<Cursor>,
    /// Tokens to read before the files: macro expansions waiting to be
    /// rescanned.
    pending: VecDeque<PTok>,
    /// Expanding a list of tokens on its own (a macro argument, an `#if`
    /// line): nothing past `pending` is read.
    isolated: bool,
    conds: Vec<Cond>,
    /// The file of the predefined macros, `<built-in>`.
    builtin_file: u32,
    /// Preprocessing assembly ([`Lang::AssemblerWithCpp`]).
    asm: bool,
    /// The include guard of each file read that has one (see [`guard`]).
    guards: HashMap<PathBuf, String>,
    /// How many tokens have come out so far, where [`Pp::marks`] are.
    emitted: usize,
    /// Where the last token that came out is.
    last_out: Pos,
    /// The diagnostics so far, and where the last token out was when each
    /// came ([`Output::merge`]).
    diagnostics: Vec<Error>,
    after: Vec<Pos>,
    /// Where files start and end, after how many tokens.
    marks: Vec<(usize, Mark)>,
    /// In a group `#if` skips ([`Pp::skip_group`]).
    skipping: bool,
    /// A macro that expanded to nothing started a line, or had a space
    /// before it: the next token takes that over (as in clang, so that
    /// `EMPTY li a0, 1` is indented like `li a0, 1`). Its `bol`, `space`
    /// and `lead`.
    carry: (bool, bool, bool),
}

impl Pp<'_> {
    /// Splits a file into tokens, as C or as assembly.
    fn lex(&self, text: &str, file: u32) -> Result<(Vec<PpToken>, Pos), Error> {
        lexer::lex_pp_in(text, file, self.asm)
    }

    fn run(&mut self, src: &str) -> Result<(Vec<PTok>, Pos), Error> {
        // The predefined macros, then -D and -U, as directives of their own
        // files.
        let mut builtin: String = predefined(self.opts.lang)
            .iter()
            .map(|(name, value)| format!("#define {name} {value}\n"))
            .collect();
        builtin.push_str("#define __FILE__\n#define __LINE__\n");
        self.builtin_file = self.read_directives("<built-in>", &builtin)?;
        self.macros.get_mut("__FILE__").expect("defined above").kind = MacroKind::File;
        self.macros.get_mut("__LINE__").expect("defined above").kind = MacroKind::Line;
        let cmd: String = self
            .opts
            .macros
            .iter()
            .map(|m| match m {
                CmdMacro::Define(d) => match d.split_once('=') {
                    Some((name, value)) => format!("#define {name} {value}\n"),
                    None => format!("#define {d} 1\n"),
                },
                CmdMacro::Undef(name) => format!("#undef {name}\n"),
            })
            .collect();
        if !cmd.is_empty() {
            self.read_directives("<command line>", &cmd)?;
        }

        let (toks, end) = self.lex(src, 0)?;
        self.enter(toks, src, 0);
        let mut out = Vec::new();
        while let Some(t) = self.next()? {
            self.last_out = t.t.pos;
            out.push(t);
            self.emitted += 1;
        }
        Ok((out, end))
    }

    /// Reads a made-up file holding only directives; its number.
    fn read_directives(&mut self, name: &str, text: &str) -> Result<u32, Error> {
        let id = self.new_file(name.to_string(), PathBuf::from(name), Some(md5(text.as_bytes())));
        let (toks, _) = lexer::lex_pp(text, id)?;
        self.enter(toks, text, id);
        match self.next()? {
            Some(t) => Err(Error::new(t.t.pos, "expected a directive")),
            None => Ok(id),
        }
    }

    fn new_file(&mut self, name: String, path: PathBuf, md5: Option<[u8; 16]>) -> u32 {
        self.files.push(File { name, path, md5 });
        (self.files.len() - 1) as u32
    }

    fn enter(&mut self, toks: Vec<PpToken>, text: &str, file: u32) {
        self.stack.push(Cursor {
            toks,
            i: 0,
            text: text.into(),
            file,
            line_delta: 0,
            conds: self.conds.len(),
            back: None,
            entered_by_markers: 0,
        });
    }

    /// Records a diagnostic and goes on.
    fn report(&mut self, e: Error) {
        self.diagnostics.push(e);
        self.after.push(self.last_out);
    }

    /// Records the diagnostics a function of the lexer collected.
    fn report_all(&mut self, diagnostics: Vec<Error>) {
        for d in diagnostics {
            self.report(d);
        }
    }

    // ---- reading tokens ----

    /// The next token of the innermost file, if it has one, without taking it.
    fn peek_file(&self) -> Option<&PpToken> {
        let cur = self.stack.last()?;
        cur.toks.get(cur.i)
    }

    /// Takes the next token of the innermost file (there must be one), with
    /// its position as `#line` has it, and gives the lexer's warnings of
    /// what came before it ([`PpToken::lexed`]), but in a group `#if`
    /// skips, which clang reads raw.
    fn take_file(&mut self) -> PpToken {
        let cur = self.stack.last_mut().expect("a file");
        let mut t = cur.toks[cur.i].clone();
        cur.i += 1;
        let mut lexed = std::mem::take(&mut t.lexed);
        let positions = [&mut t.pos, &mut t.end]
            .into_iter()
            .chain(lexed.iter_mut().map(|w| &mut w.pos));
        for p in positions {
            p.file = cur.file;
            p.line = (i64::from(p.line) + cur.line_delta) as u32;
        }
        if !self.skipping {
            for w in lexed {
                self.report(w);
            }
        }
        t
    }

    /// Whether the innermost file's next token starts a directive: a `#`
    /// that starts a line, and in assembly only one followed by a
    /// directive's name or by nothing (`# a comment` is assembly).
    fn at_directive(&self) -> bool {
        let Some(cur) = self.stack.last() else {
            return false;
        };
        if !cur.toks.get(cur.i).is_some_and(|t| t.bol && t.is_punct("#")) {
            return false;
        }
        !self.asm
            || match cur.toks.get(cur.i + 1) {
                None => true,
                Some(n) if n.bol => true,
                Some(n) => n.ident().is_some_and(|name| DIRECTIVES.contains(&name)),
            }
    }

    /// The tokens of the innermost file up to the end of the line.
    fn rest_of_line(&mut self) -> Vec<PpToken> {
        let mut line = Vec::new();
        while self.peek_file().is_some_and(|t| !t.bol) {
            line.push(self.take_file());
        }
        line
    }

    /// The rest of the line after the token `name` (a directive's name in
    /// the innermost file), as written: what clang's `#error` and
    /// `#warning` print. Spaces after the name are left out; comments stay,
    /// and a backslash at the end of a line joins the next one to it.
    fn raw_rest(&self, name: &PpToken) -> String {
        let cur = self.stack.last().expect("a file");
        // The line as the lexer numbered it, before `#line`.
        let line = (i64::from(name.end.line) - cur.line_delta) as usize;
        let mut lines = cur.text.split('\n').skip(line - 1);
        let first = lines.next().unwrap_or("");
        // Columns count bytes.
        let start = (name.end.col as usize - 1).min(first.len());
        let mut text = String::new();
        let mut part = &first[start..];
        loop {
            let part_line = part.strip_suffix('\r').unwrap_or(part);
            match (part_line.strip_suffix('\\'), lines.next()) {
                (Some(joined), Some(next)) => {
                    text.push_str(joined);
                    part = next;
                }
                _ => {
                    text.push_str(part_line);
                    break;
                }
            }
        }
        text.trim_start_matches(' ').to_string()
    }

    /// Leaves the innermost file. clang reports every conditional it left
    /// open, the innermost first, and closes them.
    fn leave(&mut self) {
        let cur = self.stack.pop().expect("a file");
        while self.conds.len() > cur.conds {
            let pos = self.conds.pop().expect("checked").pos;
            self.report(Error::new(pos, "unterminated conditional directive"));
        }
        if let (Some(line), Some(parent)) = (cur.back, self.stack.last()) {
            let file = parent.file;
            self.marks.push((self.emitted, Mark::Exit { line, file }));
        }
    }

    /// The next token before macro replacement: from an expansion waiting
    /// to be rescanned, or else from the files, carrying out directives on
    /// the way. Inside the arguments of a call of the macro `call`, the
    /// arguments go on past the end of a header, but not past the end of
    /// the file being compiled.
    fn next_raw(&mut self, call: Option<&PpToken>) -> Result<Option<PTok>, Error> {
        loop {
            if let Some(t) = self.pending.pop_front() {
                return Ok(Some(t));
            }
            if self.isolated || self.stack.is_empty() {
                return Ok(None);
            }
            if self.peek_file().is_none() {
                if call.is_some() && self.stack.len() == 1 {
                    return Ok(None);
                }
                self.leave();
                continue;
            }
            if self.at_directive() {
                if let Some(name) = call {
                    if self.embedded_include(name) {
                        continue;
                    }
                }
                // An error drops the directive, and preprocessing goes on;
                // only a fatal one stops it.
                if let Err(e) = self.directive() {
                    if e.severity == Severity::Fatal {
                        return Err(e);
                    }
                    self.report(e);
                }
                continue;
            }
            let mut t = PTok::read(self.take_file());
            // A `#` that starts no directive (in assembly) is written with a
            // space before it, even in column 1, so that it reads as none
            // again (clang's output does this).
            if t.t.bol && t.t.is_punct("#") {
                t.lead = true;
            }
            return Ok(Some(t));
        }
    }

    /// Inside the arguments of a call of the macro `name`, a directive that
    /// would bring in other text (`#include`, `#pragma`...) is an error, and
    /// dropped, as clang has it (C leaves any directive there undefined;
    /// clang carries out the others). Whether the next line was one.
    fn embedded_include(&mut self, name: &PpToken) -> bool {
        let cur = self.stack.last().expect("a file");
        let directive = match cur.toks.get(cur.i + 1) {
            Some(t) if !t.bol => t.ident().unwrap_or("").to_string(),
            _ => String::new(),
        };
        if !matches!(
            directive.as_str(),
            "include" | "include_next" | "import" | "__include_macros" | "pragma" | "embed"
        ) {
            return false;
        }
        self.take_file(); // #
        let line = self.rest_of_line();
        self.report(
            Error::new(
                line[0].pos,
                format!("embedding a #{directive} directive within macro arguments is not supported"),
            )
            .note(
                name.pos,
                format!("expansion of macro '{}' requested here", name.text),
            ),
        );
        true
    }

    /// Whether the next token is `(`, without reading past the innermost file.
    fn lparen_follows(&self) -> bool {
        match self.pending.front() {
            Some(t) => t.t.is_punct("("),
            None if self.isolated => false,
            // A `(` on a later line counts too (a `#` there would start a
            // directive, and is no `(`).
            None => self.peek_file().is_some_and(|t| t.is_punct("(")),
        }
    }

    /// The next token after macro replacement, or `None` at the end.
    fn next(&mut self) -> Result<Option<PTok>, Error> {
        loop {
            let Some(mut t) = self.next_raw(None)? else {
                return Ok(None);
            };
            if t.t.kind == PpKind::Directive {
                // A line of its own: what an empty expansion leaves is for
                // the token after it.
                return Ok(Some(t));
            }
            let (bol, space, lead) = std::mem::take(&mut self.carry);
            t.t.bol |= bol;
            t.t.space |= space;
            t.lead |= lead;
            let Some(name) = t.t.ident() else {
                return Ok(Some(t));
            };
            if hidden(&t.hide, name) {
                return Ok(Some(t));
            }
            let Some(m) = self.macros.get(name) else {
                return Ok(Some(t));
            };
            let m = m.clone();
            let name = name.to_string();
            let expansion = match &m.kind {
                MacroKind::File => {
                    let file = &self.files[t.t.pos.file as usize].name;
                    let text = format!("\"{}\"", escape(file));
                    vec![self.made(&t, PpKind::Str, text)]
                }
                MacroKind::Line => vec![self.made(&t, PpKind::Number, t.t.pos.line.to_string())],
                MacroKind::Object => {
                    let hide = hide_union(&t.hide, &Rc::new(vec![name]));
                    self.substitute(&m, &[], &hide, &t, t.t.end)
                }
                MacroKind::Function { .. } => {
                    if !self.lparen_follows() {
                        return Ok(Some(t));
                    }
                    let (args, rparen) = match self.arguments(&t.t, &m)? {
                        Call::Args(args, rparen) => (args, rparen),
                        Call::Wrong => return Ok(Some(t)),
                        Call::Unterminated => continue,
                    };
                    let hide = hide_union(&hide_intersection(&t.hide, &rparen.hide), &Rc::new(vec![name]));
                    self.substitute(&m, &args, &hide, &t, rparen.t.end)
                }
            };
            if expansion.is_empty() {
                self.carry = (t.t.bol, t.t.space, t.lead);
            }
            for t in expansion.into_iter().rev() {
                self.pending.push_front(t);
            }
        }
    }

    /// A token made by the preprocessor where the token `at` was.
    fn made(&self, at: &PTok, kind: PpKind, text: String) -> PTok {
        PTok {
            t: PpToken {
                kind,
                text,
                pos: at.t.pos,
                end: at.t.end,
                bol: at.t.bol,
                space: at.t.space,
                lexed: Vec::new(),
            },
            hide: at.hide.clone(),
            site: Some(at.site.unwrap_or(at.t.pos)),
            spelled: None,
            lead: at.lead,
            function_body: false,
        }
    }

    /// Replaces a list of tokens on its own, as if it were the rest of the
    /// file: a macro argument, or a directive's line. Nothing stops it: no
    /// directive is read, and the diagnostics of the expansion are reported.
    fn expand_alone(&mut self, toks: Vec<PTok>) -> Vec<PTok> {
        let saved = std::mem::replace(&mut self.pending, toks.into());
        let was_isolated = std::mem::replace(&mut self.isolated, true);
        let carry = std::mem::take(&mut self.carry);
        let mut out = Vec::new();
        while let Some(t) = self.next().expect("only a directive can stop the preprocessor") {
            out.push(t);
        }
        self.pending = saved;
        self.isolated = was_isolated;
        self.carry = carry;
        out
    }

    /// After the name of the function-like macro `m`, reads `( arguments )`
    /// as clang's `ReadMacroCallArgumentList` does: each argument's tokens
    /// (split at commas outside parentheses; the commas of the variadic
    /// part stay in it) and the `)`. A wrong number of arguments is an
    /// error at the first extra one, or at the `)` for too few; the end of
    /// the input before the `)` is one at the name.
    fn arguments(&mut self, name: &PpToken, m: &Macro) -> Result<Call, Error> {
        let MacroKind::Function { params, variadic } = &m.kind else {
            unreachable!("a function-like macro");
        };
        let defined_here = |e: Error| e.note(m.pos, format!("macro '{}' defined here", name.text));
        let open = self.next_raw(Some(name))?.expect("lparen_follows saw the '('");
        let mut args: Vec<Vec<PTok>> = vec![Vec::new()];
        // Where each argument starts: its first token, or the `(` or `,`
        // before it if it has none.
        let mut starts: Vec<Pos> = Vec::new();
        let mut separator = open.t.pos;
        let mut depth = 0;
        let rparen = loop {
            let Some(t) = self.next_raw(Some(name))? else {
                let e = Error::new(name.pos, "unterminated function-like macro invocation");
                self.report(defined_here(e));
                return Ok(Call::Unterminated);
            };
            let arg = args.len() - 1;
            if t.t.is_punct("(") {
                depth += 1;
            } else if t.t.is_punct(")") {
                if depth == 0 {
                    if starts.len() == arg {
                        starts.push(separator);
                    }
                    break t;
                }
                depth -= 1;
            } else if t.t.is_punct(",") && depth == 0 && !(*variadic && args.len() == params.len()) {
                if starts.len() == arg {
                    starts.push(separator);
                }
                separator = t.t.pos;
                args.push(Vec::new());
                continue;
            }
            if starts.len() == arg {
                starts.push(t.t.pos);
            }
            args[arg].push(t);
        };
        // `F()` passes no argument; one empty argument where one is wanted.
        if args.len() == 1 && args[0].is_empty() {
            args.clear();
        }
        let wanted = params.len();
        if !variadic && args.len() > wanted {
            let e = Error::new(
                starts[wanted],
                "too many arguments provided to function-like macro invocation",
            );
            self.report(defined_here(e));
            return Ok(Call::Wrong);
        }
        // Too few is fine for `F()` where one argument is wanted, and where
        // only the variadic one is missing (`F(x)` for `F(x, ...)`).
        let fine = args.len() + 1 == wanted && (args.is_empty() || *variadic)
            || (*variadic && args.is_empty() && wanted == 2);
        if args.len() < wanted && !fine {
            let e = Error::new(
                rparen.t.pos,
                "too few arguments provided to function-like macro invocation",
            );
            self.report(defined_here(e));
            return Ok(Call::Wrong);
        }
        args.resize(wanted, Vec::new());
        Ok(Call::Args(args, rparen))
    }

    /// The expansion of macro `m` used at `at` (ending at `end`), with these
    /// arguments, every token's hide set extended by `hide`.
    ///
    /// The spaces between the tokens are clang's (which `-E` output shows):
    /// a body token keeps the one before it in the definition, except right
    /// after `##`; what an argument gives starts with the space before the
    /// parameter in the body, not with the one it had in the call (`[x]`
    /// with `( a )` is `[a]`); and an empty argument hands its space on to
    /// what comes next. The first token takes the spacing of the macro's
    /// name, and whether it started a line.
    fn substitute(
        &mut self,
        m: &Macro,
        args: &[Vec<PTok>],
        hide: &HideSet,
        at_tok: &PTok,
        end: Pos,
    ) -> Vec<PTok> {
        let site = Some(at_tok.site.unwrap_or(at_tok.t.pos));
        let at = &at_tok.t;
        let function_body = matches!(m.kind, MacroKind::Function { .. });
        // A body token, where the macro was used.
        let here = |t: &PpToken| PTok {
            t: PpToken {
                pos: at.pos,
                end,
                bol: false,
                lexed: Vec::new(),
                ..t.clone()
            },
            hide: HideSet::default(),
            site,
            spelled: Some((t.pos, t.end)),
            lead: t.space,
            function_body,
        };
        let placemarker = || PTok {
            t: PpToken {
                kind: PpKind::Placemarker,
                text: String::new(),
                pos: at.pos,
                end,
                bol: false,
                space: false,
                lexed: Vec::new(),
            },
            hide: HideSet::default(),
            site,
            spelled: None,
            lead: false,
            function_body,
        };
        let (at_lead, at_bol) = (at_tok.lead, at_tok.t.bol);
        let mut out: Vec<PTok> = Vec::new();
        // A `##` was just seen: the next tokens' first is pasted to the last one out.
        let mut paste = false;
        // The next token out gets a space before it (clang's `NextTokGetsSpace`).
        let mut space = false;
        let body = &m.body;
        let mut i = 0;
        while i < body.len() {
            let t = &body[i];
            if i > 0 && t.space && !body[i - 1].is_punct("##") {
                space = true;
            }
            let next_is_paste = body.get(i + 1).is_some_and(|n| n.is_punct("##"));
            // After a `##` that forms no token (assembly), a token of the
            // body keeps the space it had before it (`call f ## @plt` is
            // `call foo @plt`, as in clang); an argument's first token has
            // none (`f ## g` with `@plt` is `foo@plt`).
            let own_space = t.space && m.param(t).is_none();
            // In a function-like macro of C, `#` is always before a
            // parameter (`define` checks); in assembly it may be a token.
            let stringized = body
                .get(i + 1)
                .and_then(|n| m.param(n))
                .filter(|_| t.is_punct("#"));
            let mut piece: Vec<PTok> = if t.is_punct("##") {
                paste = true;
                space = false;
                i += 1;
                continue;
            } else if let Some(p) = stringized {
                // `#param`: the argument's spelling, as a string literal.
                let arg = &args[p];
                i += 1;
                let mut s = here(t);
                s.t.kind = PpKind::Str;
                s.spelled = None;
                let (text, lone_backslash) = stringize(arg);
                if lone_backslash {
                    let last = arg.last().expect("a backslash is a token").t.pos;
                    self.report(Error::warning_unflagged(
                        last,
                        "invalid string literal, ignoring final '\\'",
                    ));
                }
                s.t.text = text;
                vec![s]
            } else if let Some(p) = m.param(t) {
                let arg = &args[p];
                if paste || next_is_paste {
                    // Next to `##`: as written, or a placemarker if empty.
                    if arg.is_empty() {
                        vec![placemarker()]
                    } else {
                        arg.clone()
                    }
                } else {
                    self.expand_alone(arg.clone())
                }
            } else {
                vec![here(t)]
            };
            i += 1;
            // What comes out for this body token starts with the space it
            // gets, and (for an argument) no longer starts a line; the rest
            // of an argument keeps its own, as in clang, line starts too.
            if let Some(first) = piece.first_mut().filter(|f| f.t.kind != PpKind::Placemarker) {
                first.t.space = std::mem::take(&mut space);
                first.lead = first.t.space;
                first.t.bol = false;
            }
            let mut piece = piece.into_iter();
            if paste {
                paste = false;
                let left = out.pop().expect("'##' is never first in a body");
                match piece.next() {
                    Some(right) => out.extend(self.paste(left, right, own_space, at.pos)),
                    None => out.push(left), // an argument that expanded to nothing
                }
            }
            out.extend(piece);
        }
        // Placemarkers have done their job; the first token takes the
        // spacing of the macro's name.
        out.retain(|t| t.t.kind != PpKind::Placemarker);
        for t in &mut out {
            t.hide = hide_union(&t.hide, hide);
            t.site = site;
        }
        if let Some(first) = out.first_mut() {
            first.t.space = at.space;
            first.lead = at_lead;
            first.t.bol = at_bol;
        }
        out
    }

    /// `left ## right` in the expansion of a macro used at `at`: the two
    /// spellings joined, which must read as one token. When they do not, C
    /// has an error and clang goes on with the two tokens, the left one
    /// where the macro was used (where clang reports what the parser says
    /// of it), the right one where it was. In assembly that is no error:
    /// `right` has a space before it if it is a token of the macro's body
    /// written with one (`space`: `call f ## @plt` is `call foo @plt`), else
    /// none (`. ## name` is `.name`), as clang leaves them.
    fn paste(&mut self, mut left: PTok, mut right: PTok, space: bool, at: Pos) -> Vec<PTok> {
        if left.t.kind == PpKind::Placemarker {
            return vec![right];
        }
        if right.t.kind == PpKind::Placemarker {
            return vec![left];
        }
        let text = format!("{}{}", left.t.text, right.t.text);
        let one = match lexer::lex_pp_in(&text, left.t.pos.file, self.asm) {
            Ok((toks, _))
                if toks.len() == 1 && toks[0].text == text && !matches!(toks[0].kind, PpKind::Invalid(_)) =>
            {
                Some(toks[0].kind.clone())
            }
            _ => None,
        };
        let Some(kind) = one else {
            if self.asm {
                right.t.space = space;
                right.lead = space;
            } else {
                self.report(Error::new(
                    at,
                    format!("pasting formed '{text}', an invalid preprocessing token"),
                ));
                left.t.pos = at;
            }
            return vec![left, right];
        };
        vec![PTok {
            t: PpToken { kind, text, ..left.t },
            hide: left.hide,
            site: left.site,
            spelled: None,
            lead: left.lead,
            function_body: left.function_body,
        }]
    }

    // ---- directives ----

    /// Carries out the directive starting at the innermost file's next
    /// token, a `#` at the start of a line. An error drops what is left of
    /// it ([`Pp::next_raw`] reports it).
    fn directive(&mut self) -> Result<(), Error> {
        let hash = self.take_file();
        let line = self.rest_of_line();
        let Some(first) = line.first() else {
            return Ok(()); // `#` alone: the null directive
        };
        let args = &line[1..];
        match first.ident().unwrap_or("") {
            "define" => self.define(first, args),
            "undef" => self.undef(first, args),
            "include" => self.include(first, args),
            "if" => {
                let value = self.condition(first, args);
                self.open_cond(first.pos, value);
                Ok(())
            }
            "ifdef" | "ifndef" => {
                // Without a name the group is skipped (an `#else` may follow).
                let value = match self.macro_name(first, args, false) {
                    Ok(name) => {
                        self.extra_tokens(&args[1..], &first.text, false);
                        self.macros.contains_key(&name) == (first.text == "ifdef")
                    }
                    Err(e) => {
                        self.report(e);
                        false
                    }
                };
                self.open_cond(first.pos, value);
                Ok(())
            }
            "elif" => {
                // The group that ends here was taken: the others are skipped,
                // this condition not even evaluated.
                if self.innermost_cond(first)?.seen_else {
                    self.report(Error::new(first.pos, "#elif after #else"));
                }
                self.skip_group();
                Ok(())
            }
            "else" => {
                self.extra_tokens(args, "else", false);
                let cond = self.innermost_cond(first)?;
                if std::mem::replace(&mut cond.seen_else, true) {
                    self.report(Error::new(first.pos, "#else after #else"));
                }
                self.skip_group();
                Ok(())
            }
            "endif" => {
                self.extra_tokens(args, "endif", false);
                self.innermost_cond(first)?;
                self.conds.pop();
                Ok(())
            }
            "error" => Err(Error::new(first.pos, self.raw_rest(first))),
            "warning" => {
                let msg = self.raw_rest(first);
                self.report(Error::warning(first.pos, msg, "#warnings"));
                Ok(())
            }
            "pragma" => {
                if matches!(args, [t] if t.ident() == Some("once")) {
                    let file = self.stack.last().expect("a file").file;
                    // Of the file being compiled it means nothing: clang
                    // warns, at `once`.
                    if self.stack.len() == 1 {
                        let msg = "#pragma once in main file";
                        self.report(Error::warning(args[0].pos, msg, "pragma-once-outside-header"));
                    }
                    self.once.insert(self.files[file as usize].path.clone());
                    return Ok(());
                }
                // Any other means nothing to merlin (C says to ignore it),
                // and goes on as clang passes it on: `#pragma`, its tokens
                // unreplaced, one space where the source had any.
                let mut text = String::from("#pragma");
                for (i, t) in args.iter().enumerate() {
                    if i == 0 || t.space {
                        text.push(' ');
                    }
                    text.push_str(&t.text);
                }
                self.pass_on(&hash, text);
                Ok(())
            }
            "ident" | "sccs" => {
                // `#ident "string"` (macros replaced; anything after the
                // string is ignored, clang warns of it), passed on as
                // `#ident` in both spellings, as clang does.
                let expanded = self.expand_alone(read(args));
                match expanded.first() {
                    Some(s) if s.t.kind == PpKind::Str => {
                        let text = format!("#ident {}", s.t.text);
                        self.pass_on(&hash, text);
                        Ok(())
                    }
                    other => Err(Error::new(
                        other.map_or(first.end, |t| t.t.pos),
                        "invalid #ident directive",
                    )),
                }
            }
            "line" => self.line(first, args),
            // `# 33 "file"`: GNU's line marker, what `-E` writes (no
            // directive in assembly, see `at_directive`).
            _ if first.kind == PpKind::Number => self.line_marker(first, args),
            _ => Err(Error::new(first.pos, "invalid preprocessing directive")),
        }
    }

    /// Passes a directive on to the output as one line of `text`, in column
    /// 1 of the line of its `#` ([`PpKind::Directive`]).
    fn pass_on(&mut self, hash: &PpToken, text: String) {
        let pos = Pos { col: 1, ..hash.pos };
        self.pending.push_back(PTok {
            t: PpToken {
                kind: PpKind::Directive,
                text,
                pos,
                end: hash.end,
                bol: true,
                space: false,
                lexed: Vec::new(),
            },
            hide: HideSet::default(),
            site: None,
            spelled: None,
            lead: false,
            function_body: false,
        });
    }

    /// clang's warning for tokens after what a directive takes, macros
    /// replaced first for `expand` (`#include "a.h" EMPTY` has none).
    fn extra_tokens(&mut self, rest: &[PpToken], directive: &str, expand: bool) {
        let first = if expand {
            self.expand_alone(read(rest)).first().map(|t| t.t.pos)
        } else {
            rest.first().map(|t| t.pos)
        };
        if let Some(pos) = first {
            self.report(Error::warning(
                pos,
                format!("extra tokens at end of #{directive} directive"),
                "extra-tokens",
            ));
        }
    }

    /// The name a `#define`, `#undef`, `#ifdef` or `#ifndef` is about; one
    /// may not define or undefine (`define_undef`) `defined`.
    fn macro_name(&self, directive: &PpToken, args: &[PpToken], define_undef: bool) -> Result<String, Error> {
        let Some(t) = args.first() else {
            return Err(Error::new(directive.end, "macro name missing"));
        };
        match t.ident() {
            Some("defined") if define_undef => {
                Err(Error::new(t.pos, "'defined' cannot be used as a macro name"))
            }
            Some(name) => Ok(name.to_string()),
            None => Err(Error::new(t.pos, "macro name must be an identifier")),
        }
    }

    /// Whether a macro is one C itself defines: `__FILE__`, `__LINE__`, or
    /// a predefined one whose name starts with `__STDC`. C99 6.10.8p4 says
    /// not to define them again or undefine them; clang warns, and lets it
    /// be done.
    fn language_builtin(&self, m: &Macro, name: &str) -> bool {
        matches!(m.kind, MacroKind::File | MacroKind::Line)
            || (m.pos.file == self.builtin_file && name.starts_with("__STDC"))
    }

    fn undef(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let name = self.macro_name(directive, args, true)?;
        self.extra_tokens(&args[1..], "undef", false);
        if self
            .macros
            .get(&name)
            .is_some_and(|m| self.language_builtin(m, &name))
        {
            self.report(Error::warning(
                args[0].pos,
                "undefining builtin macro",
                "builtin-macro-redefined",
            ));
        }
        self.macros.remove(&name);
        Ok(())
    }

    fn define(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let name = self.macro_name(directive, args, true)?;
        let name_pos = args[0].pos;
        let rest = &args[1..];
        // Where the line ends, for what is missing at its end.
        let eod = args[args.len() - 1].end;
        // `F(` with no space before the `(` makes a function-like macro.
        let (kind, body) = if rest.first().is_some_and(|t| t.is_punct("(") && !t.space) {
            let (params, variadic, close) = parameters(rest, eod)?;
            (MacroKind::Function { params, variadic }, rest[close..].to_vec())
        } else {
            (MacroKind::Object, rest.to_vec())
        };
        let mut m = Macro {
            kind,
            body,
            pos: name_pos,
        };
        // What the body may hold, checked in clang's order: in a
        // function-like macro `#` before a parameter (in assembly before
        // anything but the end of the line: then it is no operator, a token
        // of no kind, as clang makes it), then `##` between two tokens.
        if matches!(m.kind, MacroKind::Function { .. }) {
            for i in 0..m.body.len() {
                let next = m.body.get(i + 1);
                if m.body[i].is_punct("#") && !next.is_some_and(|n| m.param(n).is_some()) {
                    if !self.asm || next.is_none() {
                        let at = next.map_or(eod, |n| n.pos);
                        return Err(Error::new(at, "'#' is not followed by a macro parameter"));
                    }
                    m.body[i].kind = PpKind::Invalid(None);
                }
            }
        }
        if let Some(t) = m.body.first().filter(|t| t.is_punct("##")) {
            return Err(Error::new(
                t.pos,
                "'##' cannot appear at start of macro expansion",
            ));
        }
        if let Some(t) = m.body.last().filter(|t| t.is_punct("##")) {
            return Err(Error::new(t.pos, "'##' cannot appear at end of macro expansion"));
        }
        // Defining a macro again: the same definition is fine (C99
        // 6.10.3p2), another one clang's warning; the new one holds.
        let again = self.macros.get(&name).and_then(|prev| {
            if self.language_builtin(prev, &name) {
                Some(Error::warning(
                    name_pos,
                    "redefining builtin macro",
                    "builtin-macro-redefined",
                ))
            } else if !prev.same(&m) {
                Some(
                    Error::warning(name_pos, format!("'{name}' macro redefined"), "macro-redefined")
                        .note(prev.pos, "previous definition is here"),
                )
            } else {
                None
            }
        });
        if let Some(e) = again {
            self.report(e);
        }
        self.macros.insert(name, m);
        Ok(())
    }

    fn include(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let eod = args.last().map_or(directive.end, |t| t.end);
        let (header, angled, pos) = match header_name(args, eod)? {
            Some((header, angled, pos, used)) => {
                self.extra_tokens(&args[used..], "include", true);
                (header, angled, pos)
            }
            None => {
                // `#include MACRO`: replace, then it must be one of the two forms.
                let expanded: Vec<PpToken> = self.expand_alone(read(args)).into_iter().map(|t| t.t).collect();
                let Some((header, angled, pos, used)) = header_name(&expanded, eod)? else {
                    return Err(Error::new(
                        args.first().map_or(directive.end, |t| t.pos),
                        "expected \"FILENAME\" or <FILENAME>",
                    ));
                };
                self.extra_tokens(&expanded[used..], "include", false);
                (header, angled, pos)
            }
        };
        if header.is_empty() {
            return Err(Error::new(pos, "empty filename"));
        }
        let not_found = || Error::fatal(pos, format!("'{header}' file not found"));
        let (name, path, text) = match self.find(&header, angled) {
            Some(found) => found,
            // clang looks for an `<a.h>` it does not find as `"a.h"` too,
            // and includes what it finds, with an error.
            None if angled => {
                let found = self.find(&header, false).ok_or_else(not_found)?;
                self.report(Error::new(
                    pos,
                    format!("'{header}' file not found with <angled> include; use \"quotes\" instead"),
                ));
                found
            }
            None => return Err(not_found()),
        };
        if self.once.contains(&path) {
            return Ok(());
        }
        // A file wholly inside `#ifndef X` ... `#endif` is not read again
        // once X is defined (clang's and GCC's multiple-include
        // optimization): nothing of it would come out, and no line marker
        // says it was entered.
        if self
            .guards
            .get(&path)
            .is_some_and(|g| self.macros.contains_key(g))
        {
            return Ok(());
        }
        if self.stack.len() >= MAX_INCLUDE_DEPTH {
            return Err(Error::new(pos, "#include nested too deeply"));
        }
        let id = self.new_file(name, path.clone(), Some(md5(text.as_bytes())));
        let (toks, _) = self.lex(&text, id)?;
        if let Some(g) = guard(&toks) {
            self.guards.insert(path, g);
        }
        // The file that includes it goes on after the directive's line.
        let back = args.last().map_or(directive.end.line, |t| t.end.line) + 1;
        self.marks.push((
            self.emitted,
            Mark::Enter {
                line: directive.pos.line,
                file: id,
            },
        ));
        self.enter(toks, &text, id);
        self.stack.last_mut().expect("entered").back = Some(back);
        Ok(())
    }

    /// The file an `#include` names: `"file"` next to the including file
    /// first, then in each `-I` directory in order; `<file>` only in those.
    /// Its name (by the directory it was found in), path and text.
    fn find(&self, header: &str, angled: bool) -> Option<(String, PathBuf, String)> {
        let cur = self.stack.last().expect("a file").file as usize;
        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        if !angled {
            let dir = self.files[cur]
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            // Named by the including file's directory, `.` for one named
            // without (clang: `./sub/s.h` next to `b.S`, the `__FILE__` in it
            // too; on Windows clang writes `.\sub/s.h`, merlin `/` on both).
            let shown = Path::new(&self.files[cur].name)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| ".".to_string());
            dirs.push((shown, dir));
        }
        for d in &self.opts.include_dirs {
            dirs.push((d.to_string_lossy().into_owned(), d.clone()));
        }
        dirs.into_iter().find_map(|(shown, dir)| {
            let path = normalize(&dir.join(header));
            let text = (self.load)(&path)?;
            let name = if shown.is_empty() {
                header.to_string()
            } else {
                format!("{}/{header}", shown.trim_end_matches(['/', '\\']))
            };
            Some((name, path, text))
        })
    }

    /// `#line N` or `#line N "name"` (after macro replacement): the next
    /// line is line N (of a file called `name`).
    fn line(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let eod = args.last().map_or(directive.end, |t| t.end);
        let toks: Vec<PpToken> = self.expand_alone(read(args)).into_iter().map(|t| t.t).collect();
        let missing = "#line directive requires a positive integer argument";
        let n = self.line_value(toks.first(), eod, missing, "#line")?;
        let rename = match toks.get(1) {
            None => None,
            Some(t) if t.kind == PpKind::Str && t.text.starts_with('"') => {
                let name = self.file_name(t)?;
                self.extra_tokens(&toks[2..], "line", false);
                Some(name)
            }
            Some(t) => return Err(Error::new(t.pos, "invalid filename for #line directive")),
        };
        self.renumber(directive, n, rename);
        Ok(())
    }

    /// `# N "name" flags`, GNU's line marker, which `-E` writes: what
    /// `#line N "name"` does. Its flags say that a file starts there (1) or
    /// ends (2), and that it is a system header (3, 4); merlin checks them
    /// as clang does, and does nothing else with them.
    fn line_marker(&mut self, digits: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let eod = args.last().map_or(digits.end, |t| t.end);
        let missing = "line marker directive requires a positive integer argument";
        let n = self.line_value(Some(digits), eod, missing, "GNU line marker")?;
        let toks: Vec<PpToken> = self.expand_alone(read(args)).into_iter().map(|t| t.t).collect();
        let rename = match toks.first() {
            None => None,
            Some(t) if t.kind == PpKind::Str && t.text.starts_with('"') => {
                let name = self.file_name(t)?;
                let entered = self.marker_flags(&toks[1..], eod)?;
                self.stack.last_mut().expect("a file").entered_by_markers = entered;
                Some(name)
            }
            Some(t) => return Err(Error::new(t.pos, "invalid filename for line marker directive")),
        };
        self.renumber(digits, n, rename);
        Ok(())
    }

    /// The flags of a line marker, after its file name, checked as clang's
    /// `ReadLineMarkerFlags` does: 1 (a file starts) or 2 (one that a
    /// marker started ends), then 3, then 4, each at most once and in that
    /// order. How many files markers have started and not ended after them.
    fn marker_flags(&mut self, flags: &[PpToken], eod: Pos) -> Result<u32, Error> {
        let invalid = "invalid flag line marker directive";
        let mut entered = self.stack.last().expect("a file").entered_by_markers;
        let Some(first) = flags.first() else {
            return Ok(entered);
        };
        let mut i = 0;
        let mut value = self.line_value(Some(first), eod, invalid, "#line")?;
        if value == 1 || value == 2 {
            if value == 1 {
                entered += 1;
            } else if entered == 0 {
                return Err(Error::new(
                    first.pos,
                    "invalid line marker flag '2': cannot pop empty include stack",
                ));
            } else {
                entered -= 1;
            }
            i += 1;
            let Some(t) = flags.get(i) else {
                return Ok(entered);
            };
            value = self.line_value(Some(t), eod, invalid, "#line")?;
        }
        // Then 3, then 4, and nothing after.
        for want in [3, 4] {
            if value != want {
                return Err(Error::new(flags[i].pos, invalid));
            }
            i += 1;
            match flags.get(i) {
                None => return Ok(entered),
                Some(t) if want == 3 => value = self.line_value(Some(t), eod, invalid, "#line")?,
                Some(t) => return Err(Error::new(t.pos, invalid)),
            }
        }
        unreachable!("4 is the last flag")
    }

    /// The number of a `#line` or a line marker, or of a flag, as clang's
    /// `GetLineValue` reads it: digits, in decimal even with a leading 0
    /// (which clang warns of), that fit in 32 bits; `missing` is the error
    /// for anything else, `what` names the directive in the other messages.
    fn line_value(
        &mut self,
        tok: Option<&PpToken>,
        eod: Pos,
        missing: &str,
        what: &str,
    ) -> Result<u32, Error> {
        let Some(t) = tok.filter(|t| t.kind == PpKind::Number) else {
            return Err(Error::new(tok.map_or(eod, |t| t.pos), missing));
        };
        let mut value: u32 = 0;
        for (k, c) in t.text.chars().enumerate() {
            let Some(d) = c.to_digit(10) else {
                let at = Pos {
                    col: t.pos.col + k as u32,
                    ..t.pos
                };
                return Err(Error::new(
                    at,
                    format!("{what} directive requires a simple digit sequence"),
                ));
            };
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(d))
                .ok_or_else(|| Error::new(t.pos, missing))?;
        }
        if t.text.starts_with('0') && value != 0 {
            self.report(Error::warning_unflagged(
                t.pos,
                format!("{what} directive interprets number as decimal, not octal"),
            ));
        }
        Ok(value)
    }

    /// The file name a `#line` or a line marker gives, a string literal.
    fn file_name(&mut self, t: &PpToken) -> Result<String, Error> {
        let mut warnings = Vec::new();
        let bytes = lexer::decode(&t.text, t.pos, &mut warnings)?;
        self.report_all(warnings);
        // The name is a string nothing evaluates, which has only the simple
        // escapes (`\\`, `\n`...) and universal character names: an octal
        // or hex one, or one C has not, is clang's error, at its backslash.
        let body = &t.text[1..t.text.len() - 1];
        let mut chars = body.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c != '\\' {
                continue;
            }
            let Some((_, e)) = chars.next() else { break };
            let mut seq = format!("\\{e}");
            match e {
                '\'' | '"' | '?' | '\\' | 'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' => continue,
                '0'..='7' => {
                    for _ in 0..2 {
                        match chars.peek() {
                            Some(&(_, d @ '0'..='7')) => {
                                seq.push(d);
                                chars.next();
                            }
                            _ => break,
                        }
                    }
                }
                'x' => {
                    while let Some(&(_, d)) = chars.peek().filter(|(_, d)| d.is_ascii_hexdigit()) {
                        seq.push(d);
                        chars.next();
                    }
                }
                // A universal character name is one character, which it may have.
                'u' | 'U' => continue,
                _ => {}
            }
            let at = Pos {
                col: t.pos.col + 1 + i as u32,
                ..t.pos
            };
            return Err(Error::new(
                at,
                format!("invalid escape sequence '{seq}' in an unevaluated string literal"),
            ));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// After the directive at `directive`, the next line is line `n` (of a
    /// file called `rename`).
    fn renumber(&mut self, directive: &PpToken, n: u32, rename: Option<String>) {
        // The directive's own line, as the lexer numbered it.
        let cur = self.stack.last().expect("a file");
        let written = i64::from(directive.pos.line) - cur.line_delta;
        let path = self.files[cur.file as usize].path.clone();
        // A name `#line` gives has no checksum, as clang's: its contents are
        // not what is read (`getPresumedLoc` gives no file for it).
        let renamed = rename.map(|name| self.new_file(name, path, None));
        let cur = self.stack.last_mut().expect("a file");
        cur.line_delta = i64::from(n) - (written + 1);
        if let Some(id) = renamed {
            cur.file = id;
        }
        let file = cur.file;
        self.marks.push((self.emitted, Mark::Rename { line: n, file }));
    }

    // ---- conditionals ----

    /// The innermost `#if` of the current file, for `#elif`, `#else` and `#endif`.
    fn innermost_cond(&mut self, directive: &PpToken) -> Result<&mut Cond, Error> {
        let opened_before = self.stack.last().expect("a file").conds;
        if self.conds.len() <= opened_before {
            return Err(Error::new(
                directive.pos,
                format!("#{} without #if", directive.text),
            ));
        }
        Ok(self.conds.last_mut().expect("checked"))
    }

    fn open_cond(&mut self, pos: Pos, value: bool) {
        self.conds.push(Cond {
            pos,
            taken: value,
            seen_else: false,
        });
        if !value {
            self.skip_group();
        }
    }

    /// Skips a group of the innermost conditional, and any that follow it,
    /// until one is taken (an `#elif` whose condition holds, or `#else`, if
    /// none was taken before) or its `#endif` closes it. Only directives
    /// are looked at, to follow the nesting of conditionals.
    fn skip_group(&mut self) {
        self.skipping = true;
        self.skip_lines();
        self.skipping = false;
    }

    /// [`Pp::skip_group`]'s lines.
    fn skip_lines(&mut self) {
        // Where the conditionals that open in the skipped lines are.
        let mut nested: Vec<Pos> = Vec::new();
        loop {
            match self.peek_file() {
                None => {
                    // clang reports every conditional left open at the end
                    // of the file, the innermost first ([`Pp::leave`] those
                    // being skipped).
                    for pos in nested.into_iter().rev() {
                        self.report(Error::new(pos, "unterminated conditional directive"));
                    }
                    return;
                }
                Some(t) if !(t.bol && t.is_punct("#")) => {
                    self.take_file();
                    continue;
                }
                Some(_) => {}
            }
            self.take_file(); // #
            let line = self.rest_of_line();
            let Some(first) = line.first() else { continue };
            let args = &line[1..];
            match first.ident() {
                Some("if" | "ifdef" | "ifndef") => nested.push(first.pos),
                Some("endif") if !nested.is_empty() => {
                    nested.pop();
                }
                Some("endif") => {
                    self.extra_tokens(args, "endif", false);
                    self.conds.pop();
                    return;
                }
                Some(d @ ("elif" | "else")) if nested.is_empty() => {
                    let Cond { taken, seen_else, .. } = *self.conds.last().expect("open");
                    if seen_else {
                        self.report(Error::new(first.pos, format!("#{d} after #else")));
                    }
                    if d == "else" {
                        let cond = self.conds.last_mut().expect("open");
                        cond.seen_else = true;
                        if !taken {
                            cond.taken = true;
                            self.extra_tokens(args, "else", false);
                            return;
                        }
                    } else if !taken && self.condition(first, args) {
                        self.conds.last_mut().expect("open").taken = true;
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// The value of an `#if` or `#elif` condition, which [`Eval`] reads as
    /// the rest of the line is replaced: false if it has an error, as in
    /// clang.
    fn condition(&mut self, directive: &PpToken, args: &[PpToken]) -> bool {
        let saved = std::mem::replace(&mut self.pending, read(args).into());
        let was_isolated = std::mem::replace(&mut self.isolated, true);
        let carry = std::mem::take(&mut self.carry);
        let end = args.last().map_or(directive.end, |t| t.end);
        let value = Eval {
            pp: self,
            tok: None,
            end,
        }
        .directive_expression();
        self.pending = saved;
        self.isolated = was_isolated;
        self.carry = carry;
        value.unwrap_or_else(|e| {
            self.report(e);
            false
        })
    }
}

/// Tokens of a directive's line, to be replaced on their own.
fn read(toks: &[PpToken]) -> Vec<PTok> {
    toks.iter().cloned().map(PTok::read).collect()
}

/// The parameters of a function-like macro, as clang's
/// `ReadMacroParameterList` reads them, from `rest`, which starts at the
/// `(`, on a line that ends at `eod`: their names, whether the last one is
/// variadic, and where the body starts. C99's `...` is the parameter
/// `__VA_ARGS__`; GNU's `args...` makes `args` the variadic one.
fn parameters(rest: &[PpToken], eod: Pos) -> Result<(Vec<String>, bool, usize), Error> {
    let at = |i: usize| rest.get(i).map_or(eod, |t| t.pos);
    let rparen_after = |i: usize| {
        if rest.get(i).is_some_and(|t| t.is_punct(")")) {
            Ok(())
        } else {
            Err(Error::new(at(i), "missing ')' in macro parameter list"))
        }
    };
    let mut params: Vec<String> = Vec::new();
    let mut i = 1;
    loop {
        let Some(t) = rest.get(i) else {
            return Err(Error::new(eod, "missing ')' in macro parameter list"));
        };
        i += 1;
        if t.is_punct(")") {
            if params.is_empty() {
                return Ok((params, false, i));
            }
            return Err(Error::new(t.pos, "expected identifier in macro parameter list"));
        }
        if t.is_punct("...") {
            rparen_after(i)?;
            params.push("__VA_ARGS__".to_string());
            return Ok((params, true, i + 1));
        }
        let Some(p) = t.ident() else {
            return Err(Error::new(t.pos, "invalid token in macro parameter list"));
        };
        if params.iter().any(|q| q == p) {
            return Err(Error::new(t.pos, format!("duplicate macro parameter name '{p}'")));
        }
        params.push(p.to_string());
        match rest.get(i) {
            Some(t) if t.is_punct(")") => return Ok((params, false, i + 1)),
            Some(t) if t.is_punct(",") => i += 1,
            Some(t) if t.is_punct("...") => {
                rparen_after(i + 1)?;
                return Ok((params, true, i + 2));
            }
            _ => return Err(Error::new(at(i), "expected comma in macro parameter list")),
        }
    }
}

/// The include guard of a file, from its tokens: `X` when its first line is
/// `#ifndef X` (or `#if !defined X`, `#if !defined(X)`) and the `#endif`
/// that closes it is its last, with no `#elif` or `#else` of that `#if`
/// between them, as clang's multiple-include optimization asks.
fn guard(toks: &[PpToken]) -> Option<String> {
    let mut lines: Vec<&[PpToken]> = Vec::new();
    let mut start = 0;
    for k in 1..=toks.len() {
        if k == toks.len() || toks[k].bol {
            lines.push(&toks[start..k]);
            start = k;
        }
    }
    let directive = |line: &[PpToken]| match line {
        [h, d, ..] if h.is_punct("#") => d.ident().map(str::to_string),
        _ => None,
    };
    let name = match lines.first()? {
        [_, _, x] if directive(lines[0]).as_deref() == Some("ifndef") => x.ident()?,
        [_, _, not, d, x] | [_, _, not, d, _, x, _]
            if directive(lines[0]).as_deref() == Some("if")
                && not.is_punct("!")
                && d.ident() == Some("defined")
                && (lines[0].len() == 5 || (lines[0][4].is_punct("(") && lines[0][6].is_punct(")"))) =>
        {
            x.ident()?
        }
        _ => return None,
    };
    let mut depth = 0;
    for (k, line) in lines.iter().enumerate() {
        match directive(line).as_deref() {
            Some("if" | "ifdef" | "ifndef") => depth += 1,
            Some("elif" | "elifdef" | "elifndef" | "else") if depth == 1 => return None,
            Some("endif") => {
                depth -= 1;
                if depth == 0 {
                    return (k == lines.len() - 1).then(|| name.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// `"file"` or `<file>` at the start of an `#include`'s tokens, on a line
/// that ends at `eod`: the name, whether it was `<file>`, where it is and
/// how many tokens it took; `None` if neither form is there.
fn header_name(toks: &[PpToken], eod: Pos) -> Result<Option<(String, bool, Pos, usize)>, Error> {
    let Some(first) = toks.first() else {
        return Ok(None);
    };
    if first.kind == PpKind::Str {
        // Not a string literal: the characters between the quotes, as they are.
        let name = first.text[1..first.text.len() - 1].to_string();
        return Ok(Some((name, false, first.pos, 1)));
    }
    if !first.is_punct("<") {
        return Ok(None);
    }
    let Some(close) = toks.iter().position(|t| t.is_punct(">")) else {
        return Err(Error::new(eod, "expected '>'").note(first.pos, "to match this '<'"));
    };
    let mut name = String::new();
    for (k, t) in toks[1..close].iter().enumerate() {
        if k > 0 && t.space {
            name.push(' ');
        }
        name.push_str(&t.text);
    }
    Ok(Some((name, true, first.pos, close + 1)))
}

/// `\` and `"` escaped, to go between double quotes.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `#arg`: the argument's spelling as a string literal, with the `\` and
/// `"` of string literals and character constants in it escaped. Other
/// backslashes stay as they are, so an argument that ends in one (`s(\)`)
/// would have no valid literal (C leaves it undefined): clang leaves that
/// backslash out, and warns. Whether it did.
fn stringize(arg: &[PTok]) -> (String, bool) {
    let mut s = String::from("\"");
    for (k, t) in arg.iter().enumerate() {
        if k > 0 && t.t.space {
            s.push(' ');
        }
        if matches!(t.t.kind, PpKind::Str | PpKind::Char) {
            s.push_str(&escape(&t.t.text));
        } else {
            s.push_str(&t.t.text);
        }
    }
    // An odd number of backslashes at the end would escape the closing quote.
    let trailing = s.len() - s.trim_end_matches('\\').len();
    let lone = trailing % 2 == 1;
    if lone {
        s.pop();
    }
    s.push('"');
    (s, lone)
}

/// A value in `#if` arithmetic: 64 bits, signed (`intmax_t`) or not
/// (`uintmax_t`).
#[derive(Clone)]
struct Val {
    bits: u64,
    unsigned: bool,
    /// The identifier it is, if it is one alone: one followed by `(` is no
    /// macro (`function-like macro 'f' is not defined`).
    ident: Option<(Pos, String)>,
}

impl Val {
    fn new(bits: u64, unsigned: bool) -> Val {
        Val {
            bits,
            unsigned,
            ident: None,
        }
    }

    fn truth(b: bool) -> Val {
        Val::new(u64::from(b), false)
    }

    fn negative(&self) -> bool {
        !self.unsigned && (self.bits as i64) < 0
    }
}

/// The precedence of `?`, where the top of a condition starts, and of `,`,
/// which only parentheses or the middle of `?:` may hold (clang's).
const QUESTION: u8 = 4;
const COMMA: u8 = 3;

/// The precedence of the binary operator `tok` in `#if`, as clang has them:
/// the higher binds tighter; `)`, `:` and the end of the line (`None`) end
/// what is being read. `None` for a token that is no binary operator.
fn precedence(tok: Option<&PTok>) -> Option<u8> {
    let Some(t) = tok else {
        return Some(0);
    };
    let PpKind::Punct(op) = t.t.kind else {
        return None;
    };
    Some(match op {
        "*" | "/" | "%" => 14,
        "+" | "-" => 13,
        "<<" | ">>" => 12,
        "<" | "<=" | ">" | ">=" => 11,
        "==" | "!=" => 10,
        "&" => 9,
        "^" => 8,
        "|" => 7,
        "&&" => 6,
        "||" => 5,
        "?" => QUESTION,
        "," => COMMA,
        ":" => 2,
        ")" => 0,
        _ => return None,
    })
}

/// Evaluates an `#if` expression (C99 6.10.1) as clang's
/// `PPExpressions.cpp` does, with its diagnostics: an integer constant
/// expression over 64-bit values, every operator but assignments, `++`,
/// `--` and `sizeof`, read by precedence climbing ([`precedence`]). The
/// tokens come one at a time from the line, macros replaced as they come
/// ([`Pp::next`]), but the name after `defined`. `&&`, `||` and `?:`
/// evaluate only what they need (`live` false below them): division by
/// zero there is no error, an overflow no warning.
struct Eval<'p, 'a> {
    pp: &'p mut Pp<'a>,
    /// The next token, `None` at the end of the line.
    tok: Option<PTok>,
    /// Where the line ends, for what is missing there.
    end: Pos,
}

impl Eval<'_, '_> {
    /// Reads the next token, macros replaced.
    fn advance(&mut self) -> Result<(), Error> {
        self.tok = self.pp.next()?;
        Ok(())
    }

    fn here(&self) -> Pos {
        self.tok.as_ref().map_or(self.end, |t| t.t.pos)
    }

    fn at(&self, p: &str) -> bool {
        self.tok.as_ref().is_some_and(|t| t.t.is_punct(p))
    }

    /// The whole line (clang's `EvaluateDirectiveExpression`): an
    /// expression, then the end of the line. Anything else there is an
    /// error, but the value stands (`#if 1 )` is true).
    fn directive_expression(&mut self) -> Result<bool, Error> {
        self.advance()?;
        let mut v = self.value(true)?;
        if self.tok.is_some() {
            self.binary(&mut v, QUESTION, true)?;
            if let Some(t) = &self.tok {
                let e = Error::new(t.t.pos, "expected end of line in preprocessor expression");
                self.pp.report(e);
            }
        }
        Ok(v.bits != 0)
    }

    /// A value (clang's `EvaluateValue`): a number, a character constant,
    /// an identifier (0), `defined`, a unary operator and its operand, or
    /// an expression in parentheses.
    fn value(&mut self, live: bool) -> Result<Val, Error> {
        let Some(t) = self.tok.clone() else {
            return Err(Error::new(self.end, "expected value in expression"));
        };
        let pos = t.t.pos;
        match t.t.kind {
            PpKind::Ident if t.t.text == "defined" => self.defined(&t),
            PpKind::Ident => {
                self.advance()?;
                Ok(Val {
                    ident: Some((pos, t.t.text)),
                    ..Val::new(0, false)
                })
            }
            PpKind::Number => {
                let v = self.number(&t.t, live)?;
                self.advance()?;
                Ok(v)
            }
            PpKind::Char => {
                let mut warnings = Vec::new();
                let c = lexer::char_constant(&t.t.text, pos, &mut warnings)?;
                self.pp.report_all(warnings);
                let lexer::Tok::Num(v, _) = c else {
                    unreachable!("a character constant is a number")
                };
                self.advance()?;
                Ok(Val::new(v as i64 as u64, false))
            }
            PpKind::Punct("(") => {
                self.advance()?;
                let mut v = self.value(live)?;
                if !self.at(")") {
                    self.binary(&mut v, 1, live)?;
                    if !self.at(")") {
                        return Err(Error::new(self.here(), "expected ')' in preprocessor expression")
                            .note(pos, "to match this '('"));
                    }
                }
                self.advance()?;
                v.ident = None;
                Ok(v)
            }
            PpKind::Punct(op @ ("+" | "-" | "~" | "!")) => {
                self.advance()?;
                let v = self.value(live)?;
                Ok(match op {
                    "+" => Val::new(v.bits, v.unsigned),
                    "-" => {
                        // Only the most negative value overflows.
                        if live && !v.unsigned && v.bits == i64::MIN as u64 {
                            self.overflow(pos);
                        }
                        Val::new(v.bits.wrapping_neg(), v.unsigned)
                    }
                    "~" => Val::new(!v.bits, v.unsigned),
                    _ => Val::truth(v.bits == 0),
                })
            }
            PpKind::Punct(")") => Err(Error::new(pos, "expected value in expression")),
            _ => Err(Error::new(
                pos,
                "invalid token at start of a preprocessor expression",
            )),
        }
    }

    /// An integer literal: clang's errors for a floating one and for wrong
    /// digits or suffixes. One too large for 64 bits is an error too, and
    /// its low 64 bits, unsigned, go on; one too large for a signed value
    /// is unsigned, which clang warns of in decimal.
    fn number(&mut self, t: &PpToken, live: bool) -> Result<Val, Error> {
        if floating(&t.text) {
            return Err(Error::new(
                t.pos,
                "floating point literal in preprocessor expression",
            ));
        }
        let lit = lexer::integer(&t.text, t.pos)?;
        Ok(match lit.value {
            None => {
                self.pp.report(Error::new(
                    t.pos,
                    "integer literal is too large to be represented in any integer type",
                ));
                Val::new(wrapped(&t.text), true)
            }
            Some(bits) if !lit.unsigned && bits > i64::MAX as u64 => {
                if live && lit.decimal {
                    self.pp.report(Error::warning(
                        t.pos,
                        "integer literal is too large to be represented in a signed integer type, \
                         interpreting as unsigned",
                        "implicitly-unsigned-literal",
                    ));
                }
                Val::new(bits, true)
            }
            Some(bits) => Val::new(bits, lit.unsigned),
        })
    }

    /// `defined NAME` or `defined ( NAME )`, the name not replaced (clang's
    /// `EvaluateDefined`): 1 if it is a macro. A `defined` that a macro's
    /// expansion produced is undefined behavior (C99 6.10.1p4), which clang
    /// evaluates all the same, and warns of, but in a function-like macro's
    /// body (the only way to write some tests).
    fn defined(&mut self, defined: &PTok) -> Result<Val, Error> {
        let mut next = self.pp.next_raw(None)?;
        let open = match &next {
            Some(p) if p.t.is_punct("(") => {
                let open = p.t.pos;
                next = self.pp.next_raw(None)?;
                Some(open)
            }
            _ => None,
        };
        let name = match &next {
            None => return Err(Error::new(self.end, "macro name missing")),
            Some(n) => match n.t.ident() {
                Some(name) => name.to_string(),
                None => return Err(Error::new(n.t.pos, "macro name must be an identifier")),
            },
        };
        if let Some(open) = open {
            match self.pp.next_raw(None)? {
                Some(close) if close.t.is_punct(")") => {}
                other => {
                    let at = other.map_or(self.end, |t| t.t.pos);
                    return Err(Error::new(at, "missing ')' after 'defined'").note(open, "to match this '('"));
                }
            }
        }
        let value = self.pp.macros.contains_key(&name);
        self.advance()?;
        if defined.site.is_some() && !defined.function_body {
            self.pp.report(Error::warning(
                defined.t.pos,
                "macro expansion producing 'defined' has undefined behavior",
                "expansion-to-defined",
            ));
        }
        Ok(Val::truth(value))
    }

    /// The precedence of the operator after the value `v`; an error if the
    /// token there is none.
    fn precedence_after(&self, v: &Val) -> Result<u8, Error> {
        if let Some(p) = precedence(self.tok.as_ref()) {
            return Ok(p);
        }
        let t = self.tok.as_ref().expect("the end of the line has a precedence");
        if let (true, Some((pos, name))) = (t.t.is_punct("("), &v.ident) {
            return Err(Error::new(
                *pos,
                format!("function-like macro '{name}' is not defined"),
            ));
        }
        Err(Error::new(
            t.t.pos,
            "token is not a valid binary operator in a preprocessor subexpression",
        ))
    }

    /// The binary operators after `lhs` whose precedence is at least `min`
    /// (clang's `EvaluateDirectiveSubExpr`), applied to it: each takes as
    /// its right side a value and the operators after it that bind tighter,
    /// but `?`, whose middle runs to its `:` and whose end groups to the
    /// right.
    fn binary(&mut self, lhs: &mut Val, min: u8, live: bool) -> Result<(), Error> {
        let mut prec = self.precedence_after(lhs)?;
        while prec >= min {
            let Some(PpKind::Punct(op)) = self.tok.as_ref().map(|t| t.t.kind.clone()) else {
                unreachable!("only operators bind")
            };
            let at = self.here();
            // The right side of `0 && x`, `1 || x` and `0 ? x : y` is not
            // evaluated.
            let rhs_live = match op {
                "&&" | "?" => live && lhs.bits != 0,
                "||" => live && lhs.bits == 0,
                _ => live,
            };
            self.advance()?;
            let mut rhs = self.value(rhs_live)?;
            let this = prec;
            prec = self.precedence_after(&rhs)?;
            let rhs_min = if op == "?" { COMMA } else { this + 1 };
            if prec >= rhs_min {
                self.binary(&mut rhs, rhs_min, rhs_live)?;
                prec = self.precedence_after(&rhs)?;
            }
            *lhs = match op {
                "?" => {
                    if !self.at(":") {
                        return Err(Error::new(self.here(), "expected ':'").note(at, "to match this '?'"));
                    }
                    self.advance()?;
                    let after_live = live && lhs.bits == 0;
                    let mut after = self.value(after_live)?;
                    self.binary(&mut after, this, after_live)?;
                    prec = self.precedence_after(&after)?;
                    let unsigned = rhs.unsigned || after.unsigned;
                    let v = if lhs.bits != 0 { rhs } else { after };
                    Val::new(v.bits, unsigned)
                }
                ":" => return Err(Error::new(at, "':' without preceding '?'")),
                _ => self.apply(op, lhs, &rhs, live, at)?,
            };
        }
        Ok(())
    }

    /// clang's warning for a signed result that does not fit.
    fn overflow(&mut self, at: Pos) {
        self.pp.report(Error::warning_unflagged(
            at,
            "integer overflow in preprocessor expression",
        ));
    }

    /// `l op r`, the operator at `at`, with C's usual arithmetic
    /// conversions (unsigned if either side is; not for shifts, which take
    /// the left side's type, nor for `&&`, `||` and `,`), wrapping, with
    /// clang's warnings where it is evaluated (`live`): for a negative
    /// value made unsigned, and for a signed result that overflows.
    fn apply(&mut self, op: &str, l: &Val, r: &Val, live: bool, at: Pos) -> Result<Val, Error> {
        let unsigned = !matches!(op, "<<" | ">>" | "&&" | "||" | ",") && (l.unsigned || r.unsigned);
        if live && unsigned {
            for (side, v) in [("left", l), ("right", r)] {
                if v.negative() {
                    self.pp.report(Error::warning_unflagged(
                        at,
                        format!(
                            "{side} side of operator converted from negative value to unsigned: {} to {}",
                            v.bits as i64, v.bits
                        ),
                    ));
                }
            }
        }
        let (a, b) = (l.bits, r.bits);
        let (sa, sb) = (a as i64, b as i64);
        let arith = |bits: u64| Val::new(bits, unsigned);
        // Whether a signed `+`, `-` or `*` overflows.
        let signed_overflow = |checked: Option<i64>| !unsigned && checked.is_none();
        let mut overflow = false;
        let v = match op {
            "||" => Val::truth(a != 0 || b != 0),
            "&&" => Val::truth(a != 0 && b != 0),
            "," => Val::new(b, r.unsigned),
            "|" => arith(a | b),
            "^" => arith(a ^ b),
            "&" => arith(a & b),
            "==" => Val::truth(a == b),
            "!=" => Val::truth(a != b),
            "<" => Val::truth(if unsigned { a < b } else { sa < sb }),
            ">" => Val::truth(if unsigned { a > b } else { sa > sb }),
            "<=" => Val::truth(if unsigned { a <= b } else { sa <= sb }),
            ">=" => Val::truth(if unsigned { a >= b } else { sa >= sb }),
            "<<" => {
                let (bits, ov) = shift_left(a, b, l.unsigned);
                overflow = ov;
                Val::new(bits, l.unsigned)
            }
            ">>" => {
                // A shift by the width or more shifts by one less.
                overflow = b >= 64;
                let n = b.min(63) as u32;
                let bits = if l.unsigned { a >> n } else { (sa >> n) as u64 };
                Val::new(bits, l.unsigned)
            }
            "+" => {
                overflow = signed_overflow(sa.checked_add(sb));
                arith(a.wrapping_add(b))
            }
            "-" => {
                overflow = signed_overflow(sa.checked_sub(sb));
                arith(a.wrapping_sub(b))
            }
            "*" => {
                overflow = signed_overflow(sa.checked_mul(sb));
                arith(a.wrapping_mul(b))
            }
            "/" | "%" if b == 0 => {
                if live {
                    let what = if op == "/" { "division" } else { "remainder" };
                    return Err(Error::new(
                        at,
                        format!("{what} by zero in preprocessor expression"),
                    ));
                }
                arith(0)
            }
            "/" if unsigned => arith(a / b),
            "%" if unsigned => arith(a % b),
            "/" => {
                overflow = sa == i64::MIN && sb == -1;
                arith(sa.wrapping_div(sb) as u64)
            }
            "%" => arith(sa.wrapping_rem(sb) as u64),
            _ => unreachable!("binary() only passes the operators above"),
        };
        if overflow && live {
            self.overflow(at);
        }
        Ok(v)
    }
}

/// `a << b` in 64 bits, as clang's `APInt::ushl_ov` and `sshl_ov` do it: a
/// shift by 64 or more is 0, and overflows; a shift that drops a set bit
/// overflows (for a signed value, one that changes its sign too).
fn shift_left(a: u64, b: u64, unsigned: bool) -> (u64, bool) {
    if b >= 64 {
        return (0, true);
    }
    let n = b as u32;
    let overflow = if unsigned {
        n > a.leading_zeros()
    } else if (a as i64) >= 0 {
        n >= a.leading_zeros()
    } else {
        n >= a.leading_ones()
    };
    (a << n, overflow)
}

/// Whether a preprocessing number is a floating constant: it has a `.` or
/// an exponent (`e` in decimal, `p` in hex).
fn floating(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let hex = lower.starts_with("0x");
    lower.contains('.') || (!hex && lower.contains('e')) || (hex && lower.contains('p'))
}

/// The low 64 bits of an integer literal too large for them, whose digits
/// [`lexer::integer`] has checked.
fn wrapped(text: &str) -> u64 {
    let lower = text.to_ascii_lowercase();
    let (radix, digits) = match lower.strip_prefix("0x") {
        Some(hex) => (16, hex),
        None if lower.starts_with('0') => (8, lower.as_str()),
        None => (10, lower.as_str()),
    };
    digits
        .chars()
        .map_while(|c| c.to_digit(radix))
        .fold(0u64, |v, d| {
            v.wrapping_mul(u64::from(radix)).wrapping_add(u64::from(d))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Preprocesses `main.c` with these other files around it.
    fn run(files: &[(&str, &str)], src: &str, opts: &Options) -> Result<Output, Error> {
        let files: HashMap<PathBuf, String> = files
            .iter()
            .map(|(name, text)| (normalize(Path::new(name)), text.to_string()))
            .collect();
        let load = move |p: &Path| files.get(p).cloned();
        preprocess("main.c", src, opts, &load).map_err(|mut e| e.pop().expect("the error"))
    }

    /// The tokens, each followed by one space: layout aside, what the
    /// parser gets.
    fn words(toks: &[PpToken]) -> String {
        toks.iter().map(|t| format!("{} ", t.text)).collect()
    }

    /// `expected` split into tokens the same way, to compare with [`words`].
    fn same(expected: &str) -> String {
        words(&lexer::lex_pp(expected, 0).unwrap().0)
    }

    fn pp(src: &str) -> String {
        words(
            &run(&[], src, &Options::default())
                .unwrap_or_else(|e| panic!("{e}"))
                .tokens,
        )
    }

    fn pp_with(files: &[(&str, &str)], src: &str) -> String {
        words(
            &run(files, src, &Options::default())
                .unwrap_or_else(|e| panic!("{e}"))
                .tokens,
        )
    }

    fn err(src: &str) -> String {
        err_with(&[], src)
    }

    fn err_with(files: &[(&str, &str)], src: &str) -> String {
        run(files, src, &Options::default()).unwrap_err().render("main.c")
    }

    /// `main.c` preprocessed going on after errors, as for the parser: its
    /// tokens and diagnostics (the messages are the table's, in
    /// `tests/diagnostics/preprocessor.rs`).
    fn recovered(src: &str) -> Output {
        preprocess_recovering("main.c", src, &Options::default(), &|_| None).unwrap()
    }

    /// The tokens that come out after errors, as [`words`] has them.
    fn pp_recovered(src: &str) -> String {
        words(&recovered(src).tokens)
    }

    #[test]
    fn object_like_macros() {
        assert_eq!(pp("#define N 10\nint a[N];"), same("int a[10];"));
        assert_eq!(pp("#define EMPTY\nEMPTY int EMPTY x;"), same("int x;"));
        // Rescanned: N expands to M, which expands too.
        assert_eq!(pp("#define N M + 1\n#define M 2\nN"), same("2 + 1"));
        // Never inside its own expansion, however indirectly.
        assert_eq!(pp("#define x x + 1\nx"), same("x + 1"));
        assert_eq!(pp("#define a b\n#define b a\na b"), same("a b"));
        // A keyword is a name like any other here.
        assert_eq!(pp("#define int long\nint x;"), same("long x;"));
        // #undef, and redefining the same way.
        assert_eq!(pp("#define N 1\n#undef N\nN"), same("N"));
        assert_eq!(pp("#define N (1 + 2)\n#define N (1 + 2)\nN"), same("(1 + 2)"));
        assert_eq!(pp("#undef NEVER_DEFINED\n1"), same("1"));
    }

    #[test]
    fn function_like_macros() {
        assert_eq!(
            pp("#define sq(x) ((x) * (x))\nsq(a + 1)"),
            same("((a + 1) * (a + 1))")
        );
        assert_eq!(
            pp("#define max(a, b) ((a) > (b) ? (a) : (b))\nmax(f(1, 2), (3, 4))"),
            same("((f(1, 2)) > ((3, 4)) ? (f(1, 2)) : ((3, 4)))")
        );
        // No `(`: just the name. The `(` may be on the next line.
        assert_eq!(pp("#define f(x) [x]\nf + f\n(1)"), same("f + [1]"));
        assert_eq!(pp("#define f() 7\nf() f( )"), same("7 7"));
        // `F (x)` in a definition is an object-like macro.
        assert_eq!(pp("#define F (x)\nF(1)"), same("(x)(1)"));
        // Arguments are expanded before they are substituted...
        assert_eq!(pp("#define N 4\n#define id(x) x\nid(N)"), same("4"));
        // ...and an argument may be empty.
        assert_eq!(
            pp("#define pair(a, b) a|b\npair(,) pair(1,) pair(,2)"),
            same("| 1| |2")
        );
        // C99 6.10.3.4's example: the result is rescanned with what follows.
        assert_eq!(pp("#define f(a) a*g\n#define g(a) f(a)\nf(2)(9)"), same("2*9*g"));
    }

    /// C99 6.10.3.5, EXAMPLE 3: rescanning and the hide sets.
    #[test]
    fn standard_example_3() {
        let src = "#define x 3
#define f(a) f(x * (a))
#undef x
#define x 2
#define g f
#define z z[0]
#define h g(~
#define m(a) a(w)
#define w 0,1
#define t(a) a
#define p() int
#define q(x) x
#define r(x,y) x ## y
#define str(x) # x
f(y+1) + f(f(z)) % t(t(g)(0) + t)(1);
g(x+(3,4)-w) | h 5) & m
(f)^m(m);
p() i[q()] = { q(1), r(2,3), r(4,), r(,5), r(,) };
char c[2][6] = { str(hello), str() };";
        let want = "f(2 * (y+1)) + f(2 * (f(2 * (z[0])))) % f(2 * (0)) + t(1);
f(2 * (2+(3,4)-0,1)) | f(2 * (~ 5)) & f(2 * (0,1))^m(0,1);
int i[] = { 1, 23, 4, 5, };
char c[2][6] = { \"hello\", \"\" };";
        assert_eq!(pp(src), same(want));
    }

    /// C99 6.10.3.5, EXAMPLE 4: `#` and `##`.
    #[test]
    fn standard_example_4() {
        let src = r#"#define str(s) # s
#define xstr(s) str(s)
#define debug(s, t) printf("x" # s "= %d, x" # t "= %s", \
 x ## s, x ## t)
#define INCFILE(n) vers ## n
#define glue(a, b) a ## b
#define xglue(a, b) glue(a, b)
#define HIGHLOW "hello"
#define LOW LOW ", world"
debug(1, 2);
fputs(str(strncmp("abc\0d", "abc", '\4') // this goes away
 == 0) str(: @\n), s);
xstr(INCFILE(2).h)
glue(HIGH, LOW);
xglue(HIGH, LOW)"#;
        let want = r#"printf("x" "1" "= %d, x" "2" "= %s", x1, x2);
fputs("strncmp(\"abc\\0d\", \"abc\", '\\4') == 0" ": @\n", s);
"vers2.h"
"hello";
"hello" ", world""#;
        assert_eq!(pp(src), same(want));
    }

    /// C99 6.10.3.5, EXAMPLE 5: placemarkers.
    #[test]
    fn standard_example_5() {
        let src = "#define t(x,y,z) x ## y ## z
int j[] = { t(1,2,3), t(,4,5), t(6,,7), t(8,9,),
 t(10,,), t(,11,), t(,,12), t(,,) };";
        assert_eq!(pp(src), same("int j[] = { 123, 45, 67, 89, 10, 11, 12, };"));
    }

    /// C99 6.10.3.5, EXAMPLE 7: variadic macros.
    #[test]
    fn standard_example_7() {
        let src = r#"#define debug(...) fprintf(stderr, __VA_ARGS__)
#define showlist(...) puts(#__VA_ARGS__)
#define report(test, ...) ((test)?puts(#test):\
 printf(__VA_ARGS__))
debug("Flag");
debug("X = %d\n", x);
showlist(The first, second, and third items.);
report(x>y, "x is %d but y is %d", x, y);"#;
        let want = r#"fprintf(stderr, "Flag" );
fprintf(stderr, "X = %d\n", x );
puts( "The first, second, and third items." );
((x>y)?puts("x>y"): printf("x is %d but y is %d", x, y));"#;
        assert_eq!(pp(src), same(want));
        // The variadic part may be left out.
        assert_eq!(
            pp("#define v(a, ...) a: __VA_ARGS__ ;\nv(1) v(1,) v(1, 2, 3)"),
            same("1: ; 1: ; 1: 2, 3;")
        );
    }

    #[test]
    fn stringizing_spaces_and_quotes() {
        // Whitespace between tokens becomes one space; none at the ends.
        assert_eq!(pp("#define s(x) #x\ns(  a   +\n  b  )"), same(r#""a + b""#));
        assert_eq!(
            pp("#define s(x) #x\ns(\"a\\n\" '\\'')"),
            same(r#""\"a\\n\" '\\''""#)
        );
        assert_eq!(pp("#define s(x) #x\n#define N 1\ns(N)"), same(r#""N""#));
        // A backslash outside a literal stays as it is: one at the end would
        // escape the closing quote, which C leaves undefined, and clang
        // leaves out (with a warning). Two are fine.
        assert_eq!(pp("#define s(x) #x\ns(\\) s(a \\)"), same(r#""" "a ""#));
        assert_eq!(pp("#define s(x) #x\ns(\\\\)"), same(r#""\\""#));
    }

    #[test]
    fn pasting() {
        assert_eq!(
            pp("#define cat(a, b) a ## b\ncat(x, 1) cat(+, =) cat(<<, =) cat(1, .5e+3)"),
            same("x1 += <<= 1.5e+3")
        );
        // The pasted token is rescanned.
        assert_eq!(
            pp("#define cat(a, b) a ## b\n#define xy 42\ncat(x, y)"),
            same("42")
        );
        // Two tokens that make none are an error, and stay two, the left
        // one where the macro was used, as in clang.
        let out = recovered("#define cat(a, b) a ## b\nint x = 1 cat(+, -) 1;");
        let at: Vec<(&str, Pos)> = out.tokens.iter().map(|t| (t.text.as_str(), t.pos)).collect();
        assert_eq!(at[4..6], [("+", Pos::new(2, 11)), ("-", Pos::new(2, 18))]);
        // Object-like macros paste too.
        assert_eq!(pp("#define X a ## b\nX"), same("ab"));
    }

    #[test]
    fn positions() {
        let out = run(
            &[],
            "#define F(x) (x + 1)\nint a = F(\n  b);",
            &Options::default(),
        )
        .unwrap();
        let at: Vec<(&str, Pos)> = out.tokens.iter().map(|t| (t.text.as_str(), t.pos)).collect();
        // The body's tokens are where the macro was used; the argument's
        // tokens where they were written.
        assert_eq!(
            at,
            [
                ("int", Pos::new(2, 1)),
                ("a", Pos::new(2, 5)),
                ("=", Pos::new(2, 7)),
                ("(", Pos::new(2, 9)),
                ("b", Pos::new(3, 3)),
                ("+", Pos::new(2, 9)),
                ("1", Pos::new(2, 9)),
                (")", Pos::new(2, 9)),
                (";", Pos::new(3, 5)),
            ]
        );
        assert_eq!(out.end, Pos::new(3, 6));
    }

    #[test]
    fn file_and_line() {
        assert_eq!(pp("__LINE__\n\n__LINE__ __FILE__"), same(r#"1 3 "main.c""#));
        // In a macro: where the macro was used.
        assert_eq!(
            pp("#define WHERE __FILE__:__LINE__\n\nWHERE"),
            same(r#""main.c":3"#)
        );
        assert_eq!(
            pp_with(&[("h.h", "__FILE__ __LINE__")], "#include \"h.h\""),
            same(r#""./h.h" 1"#)
        );
        // #line renumbers, and renames.
        assert_eq!(
            pp("#line 100\n__LINE__\n#line 7 \"x.c\"\n__LINE__ __FILE__"),
            same(r#"100 7 "x.c""#)
        );
        // As clang takes them: line 0, past 2^31 - 1, a leading 0 (decimal);
        // a bad one is dropped.
        assert_eq!(
            pp("#line 0\n__LINE__\n#line 2147483648\n__LINE__\n#line 010\n__LINE__"),
            same("0 2147483648 10")
        );
        assert_eq!(
            pp_recovered("#line x\n__LINE__\n#line 4294967296\n__LINE__"),
            same("2 4")
        );
        // GNU's line markers, which `-E` writes, with their flags.
        assert_eq!(
            pp("# 33 \"x.c\"\n__LINE__ __FILE__\n# 7 \"h.h\" 1 3 4\n# 9 \"x.c\" 2\n__LINE__ __FILE__"),
            same(r#"33 "x.c" 9 "x.c""#)
        );
        assert_eq!(
            pp_recovered("# 5 \"x.c\" 2\n__LINE__ __FILE__"),
            same(r#"2 "main.c""#)
        );
        // A backslash in a file name is escaped.
        let out = preprocess("dir\\a.c", "__FILE__", &Options::default(), &|_| None).unwrap();
        assert_eq!(out.tokens[0].text, r#""dir\\a.c""#);
        // `__LINE__` and `__FILE__` may be defined again or undefined (a
        // warning), and the new definition holds.
        assert_eq!(
            pp("#define __LINE__ 1\n#undef __FILE__\n\n__LINE__ __FILE__"),
            same("1 __FILE__")
        );
    }

    #[test]
    fn includes() {
        let files = [
            ("a.h", "int a;\n#include \"sub/b.h\""),
            // Found next to the including file first.
            ("sub/b.h", "int b;\n#include \"c.h\""),
            ("sub/c.h", "int c;"),
            ("inc/d.h", "int d;"),
            ("inc/c.h", "int wrong;"),
        ];
        assert_eq!(
            pp_with(&files, "#include \"a.h\"\nint main;"),
            same("int a; int b; int c; int main;")
        );
        // `<...>` and `"..."` both look in the -I directories, in order.
        let opts = Options {
            include_dirs: vec![PathBuf::from("inc"), PathBuf::from("sub")],
            macros: Vec::new(),
            debug: None,
            lang: Lang::C,
            line_markers: true,
        };
        let src = "#include <d.h>\n#include \"d.h\"\n#include <c.h>\n#define H <b.h>\n#include H";
        let out = run(&files, src, &opts).unwrap();
        assert_eq!(words(&out.tokens), same("int d; int d; int wrong; int b; int c;"));
        // Files are named by where they were found.
        assert_eq!(
            out.files[2..],
            ["inc/d.h", "inc/d.h", "inc/c.h", "sub/b.h", "sub/c.h"]
        );
        // `<...>` does not look next to the including file: what it finds
        // there it includes, with an error, as clang does.
        let files: HashMap<PathBuf, String> = [(PathBuf::from("sub/c.h"), "int c;".to_string())].into();
        let load = move |p: &Path| files.get(p).cloned();
        let out = preprocess_recovering("main.c", "#include <sub/c.h>", &Options::default(), &load).unwrap();
        assert_eq!(words(&out.tokens), same("int c;"));
        assert_eq!(out.diagnostics.len(), 1);
        // A file not found stops everything (clang's fatal error).
        let stopped =
            preprocess_recovering("main.c", "#include \"x.h\"\n#bogus", &Options::default(), &|_| {
                None
            });
        assert_eq!(stopped.unwrap_err().len(), 1);
    }

    #[test]
    fn include_guards_and_once() {
        let files = [
            ("guard.h", "#ifndef GUARD_H\n#define GUARD_H\nint g;\n#endif\n"),
            ("once.h", "#pragma once\nint o;\n"),
            ("twice.h", "int t;\n"),
            ("self.h", "#include \"self.h\"\n"),
        ];
        let src = "#include \"guard.h\"\n#include \"guard.h\"\n#include \"once.h\"\n#include \"./once.h\"\n\
                   #include \"twice.h\"\n#include \"twice.h\"\n";
        assert_eq!(pp_with(&files, src), same("int g; int o; int t; int t;"));
    }

    #[test]
    fn errors_in_headers_name_the_header() {
        let files = [("worse.h", "#error stop  here")];
        assert_eq!(
            err_with(&files, "#include \"worse.h\""),
            "./worse.h:1:2: error: stop  here"
        );
    }

    #[test]
    fn conditionals() {
        let src = "#if 1 + 2 * 3 == 7\nyes\n#else\nno\n#endif";
        assert_eq!(pp(src), same("yes"));
        let src =
            "#define A 2\n#if A == 1\none\n#elif A == 2\ntwo\n#elif A == 2\nagain\n#else\nother\n#endif";
        assert_eq!(pp(src), same("two"));
        let src = "#ifdef A\na\n#endif\n#ifndef A\nnot_a\n#endif\n#define A\n#ifdef A\na\n#endif";
        assert_eq!(pp(src), same("not_a a"));
        // Nested, inside skipped groups too; skipped text need only lex.
        let src =
            "#if 0\n#if 1\nx\n#else\ny\n#endif\ndon't care\n#bogus\n#else\n#if 0\n#elif 1\nz\n#endif\n#endif";
        assert_eq!(pp(src), same("z"));
        // defined, with or without parentheses; unknown names are 0.
        let src = "#define D\n#if defined D && defined(D) && !defined(E) && UNKNOWN == 0\nok\n#endif";
        assert_eq!(pp(src), same("ok"));
    }

    #[test]
    fn if_arithmetic() {
        let truth = |e: &str| pp(&format!("#if {e}\n1\n#else\n0\n#endif")) == same("1");
        // intmax_t and uintmax_t: 64 bits.
        assert!(truth("0x100000000 > 1"));
        assert!(truth("-1 < 0"));
        assert!(truth("-1 > 0u"), "-1 converts to UINTMAX_MAX");
        assert!(truth("0xffffffffffffffff == -1"));
        assert!(truth("(1 << 40) == 0x10000000000"));
        assert!(truth("-8 >> 1 == -4"));
        assert!(truth("7 / 2 == 3 && 7 % 2 == 1 && -7 / 2 == -3"));
        assert!(truth("(2 | 1) == 3 && (6 & 3) == 2 && (6 ^ 3) == 5 && ~0 == -1"));
        assert!(truth("1 ? 2 : 0"));
        assert!(!truth("0 ? 1 : 0"));
        assert!(truth("'a' == 97 && '\\n' == 10"));
        assert!(truth("1 <= 1 && 1 >= 1 && 1 != 2 && !0 && +1 && - -1"));
        // `&&`, `||` and `?:` skip what they need not evaluate.
        assert!(truth("1 || 1 / 0"));
        assert!(!truth("0 && 1 / 0"));
        assert!(truth("1 ? 1 : 1 / 0"));
        assert!(truth("0 ? 1 / 0 : 1"));
        // `?:` groups to the right; `,` is an operator inside parentheses.
        assert!(truth("0 ? 1 : 0 ? 0 : 2"));
        assert!(truth("(0, 2) == 2"));
        // Shifts by the width or more, as clang's: `<<` gives 0, `>>`
        // shifts by 63 (warnings both).
        assert!(!truth("1 << 64") && truth("-1 >> 64 == -1"));
        assert!(truth("18446744073709551615 == 0xffffffffffffffff"));
        // An error makes the condition false, as in clang; but a literal
        // too large for 64 bits, an error too, goes on with its low bits,
        // and so does a line with something after a whole expression.
        let value = |e: &str| pp_recovered(&format!("#if {e}\n1\n#else\n0\n#endif"));
        for wrong in [
            "1 / 0",
            "",
            "1 +",
            "(1",
            "1 2",
            "1 = 2",
            "1.0",
            "defined",
            "defined(X",
            "f(1)",
        ] {
            assert_eq!(value(wrong), same("0"), "#if {wrong}");
        }
        assert_eq!(value("18446744073709551617 == 1"), same("1"));
        assert_eq!(value("1 )"), same("1"));
        // `defined` that a macro produces is evaluated, as clang does it.
        assert_eq!(
            pp("#define D defined(X)\n#define X\n#if D\nyes\n#endif"),
            same("yes")
        );
        assert_eq!(
            pp("#define F(m) defined(m)\n#if F(F) && !F(G)\nyes\n#endif"),
            same("yes")
        );
    }

    #[test]
    fn conditional_errors() {
        // What comes out after the errors, as clang has it: an unterminated
        // group as if it ended with the file, each one reported; a stray
        // `#else` or `#endif` ignored; after a second `#else` nothing is
        // taken; `#ifdef` without a name skipped.
        let errors = |src: &str| {
            let out = recovered(src);
            let lines: Vec<String> = out.diagnostics.iter().map(|d| d.render("main.c")).collect();
            (words(&out.tokens), lines)
        };
        let (toks, lines) = errors("#if 1\n#if 0\nx\n#endif\ny");
        assert_eq!(toks, same("y"));
        assert_eq!(lines, ["main.c:1:2: error: unterminated conditional directive"]);
        let (toks, lines) = errors("#if 0\n#if 1\nx");
        assert_eq!(toks, "");
        assert_eq!(
            lines,
            [
                "main.c:2:2: error: unterminated conditional directive",
                "main.c:1:2: error: unterminated conditional directive"
            ]
        );
        assert_eq!(errors("#else\na\n#endif\nb").0, same("a b"));
        assert_eq!(errors("#if 1\na\n#else\nb\n#else\nc\n#endif").0, same("a"));
        assert_eq!(errors("#if 0\na\n#else\nb\n#elif 1\nc\n#endif").0, same("b"));
        assert_eq!(errors("#ifdef\na\n#else\nb\n#endif").0, same("b"));
        // Extra tokens are a warning.
        assert_eq!(pp("#ifdef X Y\n#else Z\nb\n#endif W"), same("b"));
    }

    #[test]
    fn directives() {
        assert_eq!(pp("#\n# /* nothing */\nx"), same("x"), "null directives");
        // `#pragma once` is carried out; any other pragma, and `#ident`
        // (`#sccs`) with its string, macros replaced, go on as clang passes
        // them on, a line each, which the parser skips.
        assert_eq!(
            pp("#pragma once\n#pragma something  else\nx"),
            "#pragma something else x "
        );
        assert_eq!(
            pp("#define S \"s\"\n#ident S\n#sccs \"t\" junk\nx"),
            "#ident \"s\" #ident \"t\" x "
        );
        let src = "#pragma weird\n#ident \"v1\"\nint main(void) { return 0; }\n";
        assert!(crate::compile_with("main.c", src, &Options::default(), &|_| None).is_ok());
        // `#error` and `#warning` say the rest of the line as written,
        // comments and all, a backslash-newline taken out; clang goes on.
        let out = recovered("#error a /* c */  b  \n#warning x \\\n y\n#line 9\n#error z\nok");
        let lines: Vec<String> = out.diagnostics.iter().map(|d| d.render("main.c")).collect();
        assert_eq!(
            lines,
            [
                "main.c:1:2: error: a /* c */  b  ",
                "main.c:2:2: warning: x  y [-W#warnings]",
                "main.c:9:2: error: z"
            ]
        );
        assert_eq!(words(&out.tokens), same("ok"));
        // `#` starts a directive only at the start of a line.
        assert_eq!(pp("x # define"), same("x # define"));
    }

    #[test]
    fn definition_errors() {
        // A definition with an error is dropped.
        assert_eq!(
            pp_recovered("#define F(a b) a\n#define G(x) #y\n#define H ## x\nF(1) G(1) H"),
            same("F(1) G(1) H")
        );
        // Defining a macro again differently is a warning, and the new
        // definition holds.
        assert_eq!(pp("#define N 1\n#define N 2\nN"), same("2"));
        // GNU's named variadic parameter, and `__VA_ARGS__` where C99 gives
        // it no meaning (a name like any other, to clang).
        assert_eq!(pp("#define F(a, rest...) a: rest\nF(1, 2, 3)"), same("1: 2, 3"));
        assert_eq!(pp("#define F(x) __VA_ARGS__ x\nF(1)"), same("__VA_ARGS__ 1"));
        // `#` in an object-like macro is just a token.
        assert_eq!(pp("#define H # x\nH"), same("# x"));
    }

    #[test]
    fn call_errors() {
        // With the wrong number of arguments the name stays, the arguments
        // go; an unterminated call goes entirely.
        assert_eq!(
            pp_recovered("#define F(a, b) a\nF(1) x F(1, 2, 3) y F()"),
            same("F x F y F")
        );
        assert_eq!(pp_recovered("#define F(a) a\nx F(1"), same("x"));
        // `F()` is one empty argument where one is wanted; the variadic
        // part may be left out.
        assert_eq!(
            pp("#define F(a) [a]\n#define G(a, ...) a\nF() G(1) G()"),
            same("[] 1")
        );
        // Directives in the arguments are carried out, as clang does, but
        // `#include` and `#pragma`, an error there.
        assert_eq!(
            pp("#define F(a) a\nF(1\n#define X 2\n#if 0\n3\n#endif\n) X"),
            same("1 2")
        );
        assert_eq!(
            pp_recovered("#define F(a) #a\nF(1\n#pragma once\n)"),
            same(r#""1""#)
        );
        // A call goes on past the end of a header, not of the file compiled.
        let files = [("open.h", "#define F(a, b) a b\nF(1")];
        assert_eq!(pp_with(&files, "#include \"open.h\"\n, 2) x"), same("1 2 x"));
    }

    #[test]
    fn command_line_macros() {
        let opts = Options {
            include_dirs: Vec::new(),
            macros: vec![
                CmdMacro::Define("ONE".into()),
                CmdMacro::Define("TWO=2".into()),
                CmdMacro::Define("F(x)=(x+1)".into()),
                CmdMacro::Define("GONE=1".into()),
                CmdMacro::Undef("GONE".into()),
                CmdMacro::Define("TWO=22".into()),
                CmdMacro::Undef("__merlin__".into()),
            ],
            debug: None,
            lang: Lang::C,
            line_markers: true,
        };
        let out = run(&[], "ONE TWO F(3) GONE __merlin__ __STDC_VERSION__", &opts).unwrap();
        assert_eq!(words(&out.tokens), same("1 22 (3+1) GONE __merlin__ 199901L"));
        // Defining `TWO` again is clang's warning, there too.
        let warned: Vec<String> = out.diagnostics.iter().map(|d| d.render("main.c")).collect();
        assert_eq!(
            warned,
            [
                "<command line>:6:9: warning: 'TWO' macro redefined [-Wmacro-redefined]\n\
                 <command line>:2:9: note: previous definition is here"
            ]
        );
        let opts = Options {
            include_dirs: Vec::new(),
            macros: vec![CmdMacro::Define("1X".into())],
            debug: None,
            lang: Lang::C,
            line_markers: true,
        };
        assert_eq!(
            run(&[], "", &opts).unwrap_err().render("main.c"),
            "<command line>:1:9: error: macro name must be an identifier"
        );
    }

    #[test]
    fn predefined_macros() {
        assert_eq!(
            pp("__STDC__ __STDC_HOSTED__ __merlin__ __riscv __riscv_xlen __LP64__ __CHAR_UNSIGNED__"),
            same("1 0 1 1 64 1 1")
        );
        // LP64, as clang's `--target=riscv64 -mabi=lp64` says.
        let names =
            "__riscv_xlen _ILP32 __ILP32__ _LP64 __LP64__ __SIZEOF_INT__ __SIZEOF_LONG__ __SIZEOF_POINTER__";
        assert_eq!(pp(names), same("64 _ILP32 __ILP32__ 1 1 4 8 8"));
        // The ISA, the code model and the object format, as clang says them
        // for -march=rv64ima_zicsr_zifencei -mcmodel=medany; assembly is
        // C17 and hosted to clang, and has `__ASSEMBLER__`.
        let names = "__riscv_m __riscv_a __riscv_zicsr __riscv_muldiv __riscv_atomic \
                     __riscv_cmodel_medany __riscv_cmodel_medlow __ELF__ __STDC_VERSION__ __STDC_HOSTED__ \
                     __ASSEMBLER__";
        let out = run(&[], names, &Options::default()).unwrap();
        assert_eq!(
            words(&out.tokens),
            same("2000000 2001000 2000000 1 1 1 __riscv_cmodel_medlow 1 199901L 0 __ASSEMBLER__")
        );
        assert_eq!(
            asm(&[], names),
            "# 1 \"main.S\"\n2000000 2001000 2000000 1 1 1 __riscv_cmodel_medlow 1 201710L 1 1\n"
        );
    }

    #[test]
    fn predefined_types_and_limits() {
        // What clang's `-E -dM` says (the ignored harness test
        // predefined_macros_match_clang checks every one against clang).
        let names = "__SIZE_TYPE__, __PTRDIFF_TYPE__, __INTPTR_TYPE__, __UINTPTR_TYPE__, \
                     __INT64_TYPE__, __UINT64_TYPE__, __SIZEOF_SIZE_T__, __SIZEOF_PTRDIFF_T__, \
                     __SIZEOF_SHORT__, __CHAR_BIT__, __INT_MAX__, __LONG_MAX__, __LONG_LONG_MAX__, \
                     __SIZE_MAX__, __UINT64_MAX__, __INT8_TYPE__, __UINT16_TYPE__";
        assert_eq!(
            spell(&run(&[], names, &Options::default()).unwrap().tokens),
            concat!(
                "long unsigned int, long int, long int, long unsigned int, ",
                "long int, long unsigned int, 8, 8, ",
                "2, 8, 2147483647, 9223372036854775807L, 9223372036854775807LL, ",
                "18446744073709551615UL, 18446744073709551615UL, signed char, unsigned short\n",
            )
        );
    }

    #[test]
    fn spelling_keeps_lines_and_spaces() {
        let out = run(
            &[],
            "#define F(x) x  +  1\nint a=F(2);\n\n  b ;",
            &Options::default(),
        )
        .unwrap();
        assert_eq!(spell(&out.tokens), "int a=2 + 1;\nb ;\n");
        // Tokens that meet only after expansion get a space where they
        // would otherwise read as other tokens, so the output lexes back
        // the same.
        let src = "#define M -\n#define E\n#define f(a,b) a ## b\n#define g(x) x\n#define D /\n\
                   int a = -M 1; int b = 1 -E- 1; int c = f(1, 2)3; int d = g(-)-1; int e = 1 D/ 2;\n\
                   int h = g(x)+g(+)y;";
        let out = run(&[], src, &Options::default()).unwrap();
        let text = spell(&out.tokens);
        assert_eq!(
            text,
            "int a = - - 1; int b = 1 - - 1; int c = 12 3; int d = - -1; int e = 1 / / 2;\n\
             int h = x+ +y;\n"
        );
        assert_eq!(words(&lexer::lex_pp(&text, 0).unwrap().0), words(&out.tokens));
    }

    /// `main.S` with these other files around it, preprocessed as assembly
    /// with `-I i2`, as text.
    fn asm(files: &[(&str, &str)], src: &str) -> String {
        asm_with(files, src, true)
    }

    /// The same, with line markers or without (`-P`).
    fn asm_with(files: &[(&str, &str)], src: &str, line_markers: bool) -> String {
        let files: HashMap<PathBuf, String> = files
            .iter()
            .map(|(name, text)| (normalize(Path::new(name)), text.to_string()))
            .collect();
        let load = move |p: &Path| files.get(p).cloned();
        let opts = Options {
            lang: Lang::AssemblerWithCpp,
            include_dirs: vec![PathBuf::from("i2")],
            ..Options::default()
        };
        preprocess("main.S", src, &opts, &load)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .print_asm(line_markers)
    }

    /// What clang 23's `-E -x assembler-with-cpp` prints for the same file
    /// (but its markers for `<built-in>` and `<command line>`): comments
    /// pass through, macros replaced in them (but between apostrophes:
    /// `'t R '` is a character constant), C comments go, spaces as clang
    /// puts them, `##` that forms no token leaves two.
    #[test]
    fn assembly_is_written_as_clang_writes_it() {
        let src = "#define R lw\n#define E\n#define F(x) [x]\n#define P(a, b) a ## b\n\
                   # A comment: don't R 'a' R, R\n  # 2^64 - 1, R\n#\n\
                   \tli\ta0, 'a'\t\t# it's R\n  E li a0, 1\n R E R\n\
                   \x20 x = -R -1 R- 1+R R+1 F( a ) F(1)+1 F(-)-1\n\
                   \x20 P(., R) P(1, 2) P(x, +) .P(L, 1): ?? =??=\n\
                   \x20 la a0, $x@ \\rs1 /* a C comment */ // to the end\n\
                   \x20 F(a\nb) F(c\n  d) z\n";
        assert_eq!(
            asm(&[], src),
            "# 1 \"main.S\"\n\n\n\n\n # A comment: don't R 'a' R, R\n  # 2^64 - 1, lw\n\n \
             li a0, 'a' # it's R\n    li a0, 1\n lw lw\n  x = -lw -1 lw- 1+lw lw+1 [a] [1]+1 [-]-1\n  \
             .lw 12 x+ .L1: ?? =??=\n  la a0, $x@ \\rs1\n  [a b] [c\n   d] z\n"
        );
        // A `##` that forms no token: a token of the body keeps the space
        // it had before it, an argument's first token has none.
        let src = "#define C1(f) call f ## @plt\n#define C2(f) call f ##@plt\n\
                   #define C3(f, g) call f ## g\n#define C4(f) call f ## %lo\n#define C5(f) f ## :\n\
                   \x20 C1(foo)\n  C2(foo)\n  C3(foo, @plt)\n  C3(foo, @ plt)\n  C4(foo)\n  C5(lbl) nop\n";
        assert_eq!(
            asm(&[], src),
            "# 1 \"main.S\"\n\n\n\n\n\n  call foo @plt\n  call foo@plt\n  call foo@plt\n  call foo@ plt\n  \
             call foo %lo\n  lbl : nop\n"
        );
        // `__ASSEMBLER__`, and the width.
        assert_eq!(asm(&[], "__ASSEMBLER__ __riscv_xlen"), "# 1 \"main.S\"\n1 64\n");
    }

    /// Blank lines up to 8, a marker past that; the markers of `#include`
    /// (a guarded file is not read again) and `#line`. clang names a header
    /// next to a main file given without a directory `./i2/g.h`.
    #[test]
    fn assembly_line_markers() {
        let files = [
            (
                "i2/g.h",
                "/* a guard */\n#ifndef G_H\n#define G_H\n#define SZ 8\n#endif\n",
            ),
            ("i2/t.h", "  .word SZ\n\n  .word 2\n"),
            ("i2/h1.h", "  E x\n"),
        ];
        let src = "a\n#include \"i2/g.h\"\n#include <g.h>\n  .word SZ\n#include \"i2/t.h\"\nb\n\
                   #define E\n#include \"i2/h1.h\"\n#line 20\n  E y\n#line 30 \"other.S\"\n\
                   #define Q\n  y2\n\n\n\n\n\n\n\n\n\nc\n";
        let text = asm(&files, src);
        assert_eq!(
            text,
            "# 1 \"main.S\"\na\n# 1 \"./i2/g.h\" 1\n# 3 \"main.S\" 2\n\n  .word 8\n# 1 \"./i2/t.h\" 1\n  \
             .word 8\n\n  .word 2\n# 6 \"main.S\" 2\nb\n\n# 1 \"./i2/h1.h\" 1\n    x\n# 9 \"main.S\" 2\n\
             # 20 \"main.S\"\n y\n# 30 \"other.S\"\n\n  y2\n# 41 \"other.S\"\nc\n"
        );
        // Each line back to where it came from.
        assert_eq!(source_line(&text, 1), None);
        assert_eq!(source_line(&text, 2), Some(("main.S".into(), 1)));
        assert_eq!(source_line(&text, 6), Some(("main.S".into(), 4)));
        assert_eq!(source_line(&text, 8), Some(("./i2/t.h".into(), 1)));
        assert_eq!(source_line(&text, 10), Some(("./i2/t.h".into(), 3)));
        assert_eq!(source_line(&text, 12), Some(("main.S".into(), 6)));
        assert_eq!(source_line(&text, 18), Some(("main.S".into(), 20)));
        assert_eq!(source_line(&text, 19), None);
        assert_eq!(source_line(&text, 21), Some(("other.S".into(), 31)));
        assert_eq!(source_line(&text, 23), Some(("other.S".into(), 41)));
        assert_eq!(source_line(&text, 24), None);
    }

    /// `-P`, as clang 23's `-E -P` writes the same sources: no markers, and
    /// no blank lines but where a token is on the line right after one
    /// that had none (a directive's, or a skipped one's); a line's first
    /// token indented only where the output moved to a new line for it or
    /// at the top of a file (`   first`, but ` y` after `#endif`).
    #[test]
    fn assembly_without_line_markers() {
        let files = [
            (
                "i2/g.h",
                "/* a guard */\n#ifndef G_H\n#define G_H\n#define SZ 8\n#endif\n",
            ),
            ("i2/t.h", "  .word SZ\n\n  .word 2\n"),
            ("i2/h1.h", "  E x\n"),
            ("i2/h3.h", "\n\n   late\n"),
        ];
        let src = "a\n#include \"i2/g.h\"\n#include <g.h>\n  .word SZ\n#include \"i2/t.h\"\nb\n\
                   #define E\n#include \"i2/h1.h\"\n#line 20\n  E y\n#line 30 \"other.S\"\n\
                   #define Q\n  y2\n\n\n\n\n\n\n\n\n\nc\n";
        assert_eq!(
            asm_with(&files, src, false),
            "a\n\n  .word 8\n  .word 8\n  .word 2\nb\n    x\n y\n\n  y2\nc\n"
        );
        let src = "   first\n\n\n   third  x\n#define X\n   X fifth\n  sixth\n\n\n\n\n\n\n\n\n\n\n\n   far\n\
                   # comment\n  .text : { *(.text) } > RAM\n#if 1\n  . = ALIGN(4096);\n#endif\n";
        assert_eq!(
            asm_with(&files, src, false),
            "   first\n   third x\n     fifth\n  sixth\n   far\n # comment\n  .text : { *(.text) } > RAM\n  \
             . = ALIGN(4096);\n"
        );
        let src = "#if 0\nx\n#endif\n   y\n#define F(a) a\nF(\n z\n)\n  k\n";
        assert_eq!(asm_with(&files, src, false), " y\nz\n  k\n");
        assert_eq!(
            asm_with(&files, "#define E\n#include \"i2/h3.h\"\n  k\n", false),
            "\n late\n k\n"
        );
    }

    /// Which `#` lines are directives in assembly, and errors.
    #[test]
    fn assembly_directives() {
        // Only a directive's name makes one; `#if` in a comment is one.
        assert_eq!(
            asm(&[], "#if 0\n# include nothing\n#endif\n# 12 \"x\"\n#warn\n"),
            "# 1 \"main.S\"\n\n\n\n # 12 \"x\"\n #warn\n"
        );
        // Pragmas but `once`, and `#ident`, pass on in column 1, as clang 23
        // writes them: `#pragma`, then its tokens unreplaced, a space where
        // the source had any; `#ident` and the string, macros replaced.
        let src = "#define M 1\n  #  pragma weird(a,b)  M   x\n#pragma\n\t#pragma  x /* c */ y\n\
                   #pragma once\n  nop\n#define S \"s\"\n# ident S\n#sccs \"q\" junk\n#define E\nE\n\
                   #pragma after_empty\nend\n";
        assert_eq!(
            asm(&[], src),
            "# 1 \"main.S\"\n\n#pragma weird(a,b) M x\n#pragma\n#pragma x y\n\n  nop\n\n#ident \"s\"\n\
             #ident \"q\"\n\n\n#pragma after_empty\nend\n"
        );
        let opts = Options {
            lang: Lang::AssemblerWithCpp,
            ..Options::default()
        };
        let e = preprocess("main.S", "# if the disk is there\n", &opts, &crate::no_files).unwrap_err();
        assert_eq!(
            e[0].render("main.S"),
            "main.S:1:10: error: token is not a valid binary operator in a preprocessor subexpression"
        );
        // `#` in a function-like macro may stand before anything.
        assert_eq!(
            asm(&[], "#define F(x) # y x\nF(1)\n"),
            "# 1 \"main.S\"\n\n# y 1\n"
        );
        // C keeps its errors: an unknown directive, a bad paste.
        assert!(err("#warn\n").contains("invalid preprocessing directive"));
        assert!(err("#define P(a, b) a ## b\nP(., x)").contains("pasting formed '.x'"));
        // The guard of a file is its first `#ifndef` only if it closes last.
        let toks = |s: &str| lexer::lex_pp(s, 0).unwrap().0;
        assert_eq!(guard(&toks("#ifndef A\n#endif\n")), Some("A".into()));
        assert_eq!(guard(&toks("#if !defined(A)\nx\n#endif")), Some("A".into()));
        assert_eq!(guard(&toks("#if !defined A\n#endif")), Some("A".into()));
        assert_eq!(guard(&toks("#ifndef A\n#endif\nx")), None);
        assert_eq!(guard(&toks("x\n#ifndef A\n#endif")), None);
        assert_eq!(guard(&toks("#ifndef A\n#else\n#endif")), None);
        assert_eq!(
            guard(&toks("#ifndef A\n#if 1\n#else\n#endif\n#endif")),
            Some("A".into())
        );
    }
}
