//! # The preprocessor (C99 6.10)
//!
//! Works on the lexer's preprocessing tokens ([`PpToken`]) and hands the
//! parser the tokens of the whole translation unit, every directive carried
//! out and every macro replaced. It does what C99 asks, with two limits
//! (both errors, never a different meaning): a directive may not appear
//! inside the arguments of a macro call (C leaves that undefined; GCC and
//! clang accept some), and `defined` may not come out of a macro expansion
//! in `#if` (also undefined).
//!
//! * `#include "file"` looks in the including file's directory, then in
//!   each `-I` directory in order; `#include <file>` only in the `-I`
//!   directories (merlin has no system headers). The name may also come
//!   from a macro (`#include HEADER`). Includes nest up to 200 deep.
//!   `#pragma once` makes a file include only once; include guards
//!   (`#ifndef X` / `#define X` ... `#endif`) work like in any compiler.
//!   Other pragmas are ignored, as C says.
//! * `#define NAME tokens` and `#define F(a, b) tokens` (with `...` and
//!   `__VA_ARGS__`), `#undef`. Defining a macro again is an error unless the
//!   definitions are the same (C99 6.10.3p2), except from the command line
//!   (`-D`), where the last one wins.
//! * `#if`, `#elif` with `defined NAME` / `defined(NAME)` and integer
//!   constant expressions in 64 bits (C's `intmax_t`, `uintmax_t`), where any
//!   identifier left after macro replacement is 0; `#ifdef`, `#ifndef`,
//!   `#else`, `#endif`. Groups that are skipped are only scanned for the
//!   directives that nest, so they may hold anything that lexes (an
//!   apostrophe in a sentence too).
//! * `#error` stops with its text; `#line N "name"` renumbers the lines
//!   that follow; `#` alone does nothing. Any other directive is an error.
//! * `__FILE__` and `__LINE__`, and a few predefined macros ([`PREDEFINED`]).
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

use crate::lexer::{self, PpKind, PpToken};
use crate::{Error, Pos};
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

/// What the command line tells the preprocessor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `-I`: where `#include` looks, in order (after the including file's
    /// own directory, for `#include "..."`).
    pub include_dirs: Vec<PathBuf>,
    /// `-D` and `-U`, in command-line order.
    pub macros: Vec<CmdMacro>,
}

/// Reads the file at a path for `#include`, or `None` if there is none.
pub type Loader<'a> = &'a dyn Fn(&Path) -> Option<String>;

/// Macros every translation unit starts with. merlin compiles freestanding
/// C99 for RISC-V ILP32, with plain `char` unsigned, so it says what clang
/// says for `--target=riscv32 -ffreestanding -std=c99` about those, and
/// `__merlin__` says who is compiling.
pub const PREDEFINED: &[(&str, &str)] = &[
    ("__STDC__", "1"),
    ("__STDC_VERSION__", "199901L"),
    ("__STDC_HOSTED__", "0"),
    ("__merlin__", "1"),
    ("__riscv", "1"),
    ("__riscv_xlen", "32"),
    ("__ILP32__", "1"),
    ("__CHAR_BIT__", "8"),
    ("__CHAR_UNSIGNED__", "1"),
    ("__SIZEOF_INT__", "4"),
    ("__SIZEOF_LONG__", "4"),
    ("__SIZEOF_POINTER__", "4"),
];

/// How deeply `#include`s may nest (clang's limit too).
const MAX_INCLUDE_DEPTH: usize = 200;

/// The preprocessed translation unit.
#[derive(Debug)]
pub struct Output {
    /// Every token, macros replaced, directives gone.
    pub tokens: Vec<PpToken>,
    /// The end of the file being compiled (where end of file is reported).
    pub end: Pos,
    /// The name of each file, indexed by [`Pos::file`]: 0 is the file being
    /// compiled, then headers (named by the directory they were found in
    /// and the name in the `#include`) and the predefined and command-line
    /// macros (`<built-in>`, `<command line>`).
    pub files: Vec<String>,
}

impl Output {
    /// Names the file of every error not in the file being compiled.
    pub fn name_files(&self, mut errors: Vec<Error>) -> Vec<Error> {
        name_files(&self.files, &mut errors);
        errors
    }
}

fn name_files(files: &[String], errors: &mut [Error]) {
    for e in errors {
        if e.pos.file != 0 && e.file.is_none() {
            e.file = files.get(e.pos.file as usize).cloned();
        }
    }
}

/// Preprocesses the source `src` of the file `name` (its name for
/// `__FILE__` and errors; `#include "..."` looks next to it). An error in a
/// header has its [`Error::file`] set.
pub fn preprocess(name: &str, src: &str, opts: &Options, load: Loader) -> Result<Output, Error> {
    let mut pp = Pp {
        opts,
        load,
        files: vec![File {
            name: name.to_string(),
            path: normalize(Path::new(name)),
        }],
        macros: HashMap::new(),
        once: HashSet::new(),
        stack: Vec::new(),
        pending: VecDeque::new(),
        isolated: false,
        conds: Vec::new(),
        command_line: true,
    };
    let result = pp.run(src);
    let files: Vec<String> = pp.files.into_iter().map(|f| f.name).collect();
    match result {
        Ok((tokens, end)) => Ok(Output { tokens, end, files }),
        Err(mut e) => {
            name_files(&files, std::slice::from_mut(&mut e));
            Err(e)
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

/// A token and its hide set.
#[derive(Clone, Debug)]
struct PTok {
    t: PpToken,
    hide: HideSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MacroKind {
    Object,
    /// The parameter names, and whether `...` ends the list (its argument
    /// is `__VA_ARGS__`, after the named ones).
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

    /// The index of the parameter a body token names, if it names one
    /// (`__VA_ARGS__` is the one after the named parameters).
    fn param(&self, t: &PpToken) -> Option<usize> {
        let MacroKind::Function { params, variadic } = &self.kind else {
            return None;
        };
        let name = t.ident()?;
        if *variadic && name == "__VA_ARGS__" {
            return Some(params.len());
        }
        params.iter().position(|p| p == name)
    }
}

struct File {
    /// For `__FILE__` and errors.
    name: String,
    /// For `#pragma once` and for finding the files it includes.
    path: PathBuf,
}

/// A file being read.
struct Cursor {
    toks: Vec<PpToken>,
    i: usize,
    /// Its entry in [`Pp::files`] (`#line "name"` makes another).
    file: u32,
    /// Added to every line number (`#line`).
    line_delta: i64,
    /// How many conditionals were open when it was entered: the ones it
    /// opens must be closed in it.
    conds: usize,
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
    /// Reading the predefined and command-line macros, which may be defined
    /// again.
    command_line: bool,
}

impl Pp<'_> {
    fn run(&mut self, src: &str) -> Result<(Vec<PpToken>, Pos), Error> {
        // The predefined macros, then -D and -U, as directives of their own
        // files.
        let mut builtin: String = PREDEFINED
            .iter()
            .map(|(name, value)| format!("#define {name} {value}\n"))
            .collect();
        builtin.push_str("#define __FILE__\n#define __LINE__\n");
        self.read_directives("<built-in>", &builtin)?;
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
        self.command_line = false;

        let (toks, end) = lexer::lex_pp(src, 0)?;
        self.enter(toks, 0);
        let mut out = Vec::new();
        while let Some(t) = self.next()? {
            out.push(t.t);
        }
        Ok((out, end))
    }

    /// Reads a made-up file holding only directives.
    fn read_directives(&mut self, name: &str, text: &str) -> Result<(), Error> {
        let id = self.new_file(name.to_string(), PathBuf::from(name));
        let (toks, _) = lexer::lex_pp(text, id)?;
        self.enter(toks, id);
        match self.next()? {
            Some(t) => Err(Error::new(t.t.pos, "expected a directive")),
            None => Ok(()),
        }
    }

    fn new_file(&mut self, name: String, path: PathBuf) -> u32 {
        self.files.push(File { name, path });
        (self.files.len() - 1) as u32
    }

    fn enter(&mut self, toks: Vec<PpToken>, file: u32) {
        self.stack.push(Cursor {
            toks,
            i: 0,
            file,
            line_delta: 0,
            conds: self.conds.len(),
        });
    }

    // ---- reading tokens ----

    /// The next token of the innermost file, if it has one, without taking it.
    fn peek_file(&self) -> Option<&PpToken> {
        let cur = self.stack.last()?;
        cur.toks.get(cur.i)
    }

    /// Takes the next token of the innermost file (there must be one), with
    /// its position as `#line` has it.
    fn take_file(&mut self) -> PpToken {
        let cur = self.stack.last_mut().expect("a file");
        let mut t = cur.toks[cur.i].clone();
        cur.i += 1;
        for p in [&mut t.pos, &mut t.end] {
            p.file = cur.file;
            p.line = (i64::from(p.line) + cur.line_delta) as u32;
        }
        t
    }

    /// Whether the innermost file's next token starts a directive.
    fn at_directive(&self) -> bool {
        self.peek_file().is_some_and(|t| t.bol && t.is_punct("#"))
    }

    /// The tokens of the innermost file up to the end of the line.
    fn rest_of_line(&mut self) -> Vec<PpToken> {
        let mut line = Vec::new();
        while self.peek_file().is_some_and(|t| !t.bol) {
            line.push(self.take_file());
        }
        line
    }

    /// Leaves the innermost file, which must have closed its conditionals.
    fn leave(&mut self) -> Result<(), Error> {
        let cur = self.stack.pop().expect("a file");
        if self.conds.len() > cur.conds {
            return Err(Error::new(
                self.conds.last().expect("checked").pos,
                "unterminated conditional directive",
            ));
        }
        Ok(())
    }

    /// The next token before macro replacement: from an expansion waiting
    /// to be rescanned, or else from the files, carrying out directives on
    /// the way. Inside a macro call's arguments (`in_args`) a directive is
    /// an error and the end of a file ends the input.
    fn next_raw(&mut self, in_args: bool) -> Result<Option<PTok>, Error> {
        loop {
            if let Some(t) = self.pending.pop_front() {
                return Ok(Some(t));
            }
            if self.isolated || self.stack.is_empty() {
                return Ok(None);
            }
            if self.peek_file().is_none() {
                if in_args {
                    return Ok(None);
                }
                self.leave()?;
                continue;
            }
            if self.at_directive() {
                if in_args {
                    let pos = self.take_file().pos;
                    return Err(Error::new(
                        pos,
                        "directives inside the arguments of a macro call are not supported",
                    ));
                }
                self.directive()?;
                continue;
            }
            let t = self.take_file();
            return Ok(Some(PTok {
                t,
                hide: HideSet::default(),
            }));
        }
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
            let Some(t) = self.next_raw(false)? else {
                return Ok(None);
            };
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
                    self.substitute(&m, &[], &hide, &t.t, t.t.end)?
                }
                MacroKind::Function { params, variadic } => {
                    if !self.lparen_follows() {
                        return Ok(Some(t));
                    }
                    let (args, rparen) = self.arguments(&t.t, params.len(), *variadic)?;
                    let hide = hide_union(&hide_intersection(&t.hide, &rparen.hide), &Rc::new(vec![name]));
                    self.substitute(&m, &args, &hide, &t.t, rparen.t.end)?
                }
            };
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
                bol: false,
                space: at.t.space,
            },
            hide: at.hide.clone(),
        }
    }

    /// Replaces a list of tokens on its own, as if it were the rest of the
    /// file: a macro argument, or an `#if` line.
    fn expand_alone(&mut self, toks: Vec<PTok>) -> Result<Vec<PTok>, Error> {
        let saved = std::mem::replace(&mut self.pending, toks.into());
        let was_isolated = std::mem::replace(&mut self.isolated, true);
        let mut out = Vec::new();
        let result = loop {
            match self.next() {
                Ok(Some(t)) => out.push(t),
                Ok(None) => break Ok(out),
                Err(e) => break Err(e),
            }
        };
        self.pending = saved;
        self.isolated = was_isolated;
        result
    }

    /// After a function-like macro's name, reads `( arguments )`: each
    /// argument's tokens (split at commas outside parentheses; the commas of
    /// the variadic part stay in `__VA_ARGS__`) and the `)`.
    fn arguments(
        &mut self,
        name: &PpToken,
        nparams: usize,
        variadic: bool,
    ) -> Result<(Vec<Vec<PTok>>, PTok), Error> {
        let unterminated = || {
            Error::new(
                name.pos,
                format!("unterminated call of the function-like macro '{}'", name.text),
            )
        };
        self.next_raw(true)?.ok_or_else(unterminated)?; // (
        let mut args: Vec<Vec<PTok>> = vec![Vec::new()];
        let mut depth = 0;
        let rparen = loop {
            let t = self.next_raw(true)?.ok_or_else(unterminated)?;
            if t.t.is_punct("(") {
                depth += 1;
            } else if t.t.is_punct(")") {
                if depth == 0 {
                    break t;
                }
                depth -= 1;
            } else if t.t.is_punct(",") && depth == 0 && !(variadic && args.len() == nparams + 1) {
                args.push(Vec::new());
                continue;
            }
            args.last_mut().expect("one at least").push(t);
        };
        // `F()` passes one empty argument, which is no argument for a macro
        // without parameters.
        if nparams == 0 && !variadic && args.len() == 1 && args[0].is_empty() {
            args.clear();
        }
        // The variadic part may be left out entirely.
        if variadic && args.len() == nparams {
            args.push(Vec::new());
        }
        let want = nparams + usize::from(variadic);
        if args.len() != want {
            let which = if args.len() > want { "many" } else { "few" };
            return Err(Error::new(
                name.pos,
                format!(
                    "too {which} arguments provided to function-like macro invocation of '{}' \
                     (expected {nparams}{}, have {})",
                    name.text,
                    if variadic { " or more" } else { "" },
                    args.len()
                ),
            ));
        }
        Ok((args, rparen))
    }

    /// The expansion of macro `m` used at `at` (ending at `end`), with these
    /// arguments, every token's hide set extended by `hide`.
    fn substitute(
        &mut self,
        m: &Macro,
        args: &[Vec<PTok>],
        hide: &HideSet,
        at: &PpToken,
        end: Pos,
    ) -> Result<Vec<PTok>, Error> {
        // A body token, where the macro was used.
        let here = |t: &PpToken| PTok {
            t: PpToken {
                pos: at.pos,
                end,
                bol: false,
                ..t.clone()
            },
            hide: HideSet::default(),
        };
        let placemarker = || PTok {
            t: PpToken {
                kind: PpKind::Placemarker,
                text: String::new(),
                pos: at.pos,
                end,
                bol: false,
                space: false,
            },
            hide: HideSet::default(),
        };
        let mut out: Vec<PTok> = Vec::new();
        // A `##` was just seen: the next tokens' first is pasted to the last one out.
        let mut paste = false;
        let body = &m.body;
        let mut i = 0;
        while i < body.len() {
            let t = &body[i];
            let next_is_paste = body.get(i + 1).is_some_and(|n| n.is_punct("##"));
            // In a function-like macro, `#` is always before a parameter
            // (`define` checks).
            let stringized = body
                .get(i + 1)
                .and_then(|n| m.param(n))
                .filter(|_| t.is_punct("#"));
            let piece: Vec<PTok> = if t.is_punct("##") {
                paste = true;
                i += 1;
                continue;
            } else if let Some(p) = stringized {
                // `#param`: the argument's spelling, as a string literal.
                let arg = &args[p];
                i += 1;
                let mut s = here(t);
                s.t.kind = PpKind::Str;
                s.t.text = stringize(arg).ok_or_else(|| {
                    Error::new(
                        at.pos,
                        "'#' produced an invalid string literal: it ends in a lone '\\'",
                    )
                })?;
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
                    self.expand_alone(arg.clone())?
                }
            } else {
                vec![here(t)]
            };
            i += 1;
            let mut piece = piece.into_iter();
            if paste {
                paste = false;
                let left = out.pop().expect("'##' is never first in a body");
                match piece.next() {
                    Some(right) => out.push(glue(left, right)?),
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
        }
        if let Some(first) = out.first_mut() {
            first.t.space = at.space;
        }
        Ok(out)
    }

    // ---- directives ----

    /// Carries out the directive starting at the innermost file's next
    /// token, a `#` at the start of a line.
    fn directive(&mut self) -> Result<(), Error> {
        let hash = self.take_file();
        let line = self.rest_of_line();
        let Some(first) = line.first() else {
            return Ok(()); // `#` alone: the null directive
        };
        let args = &line[1..];
        let name = first.ident().unwrap_or("");
        match name {
            "define" => self.define(first, args),
            "undef" => {
                let name = self.macro_name(first, args)?;
                no_more(&args[1..], "#undef")?;
                if matches!(
                    self.macros.get(&name).map(|m| &m.kind),
                    Some(MacroKind::File | MacroKind::Line)
                ) {
                    return Err(Error::new(
                        args[0].pos,
                        format!("cannot undefine the builtin macro '{name}'"),
                    ));
                }
                self.macros.remove(&name);
                Ok(())
            }
            "include" => self.include(first, args),
            "if" => {
                let value = self.condition(first, args)?;
                self.open_cond(first.pos, value)
            }
            "ifdef" | "ifndef" => {
                let name = self.macro_name(first, args)?;
                no_more(&args[1..], &format!("#{}", first.text))?;
                let defined = self.macros.contains_key(&name);
                self.open_cond(first.pos, defined == (first.text == "ifdef"))
            }
            "elif" | "else" => {
                let cond = self.innermost_cond(first)?;
                if cond.seen_else {
                    return Err(Error::new(first.pos, format!("#{} after #else", first.text)));
                }
                if first.text == "else" {
                    no_more(args, "#else")?;
                    cond.seen_else = true;
                }
                // The group that ends here was taken: skip the others.
                self.skip_group()
            }
            "endif" => {
                no_more(args, "#endif")?;
                self.innermost_cond(first)?;
                self.conds.pop();
                Ok(())
            }
            "error" => Err(Error::new(first.pos, format!("#error {}", spelling(args)))),
            "pragma" => {
                if matches!(args, [t] if t.ident() == Some("once")) {
                    let file = self.stack.last().expect("a file").file;
                    self.once.insert(self.files[file as usize].path.clone());
                }
                // Other pragmas mean nothing to merlin, and C says to ignore them.
                Ok(())
            }
            "line" => self.line(first, args),
            _ => {
                let what = if name.is_empty() { &first.text } else { name };
                Err(Error::new(
                    if name.is_empty() { hash.pos } else { first.pos },
                    format!("invalid preprocessing directive '#{what}'"),
                ))
            }
        }
    }

    /// The name a `#define`, `#undef`, `#ifdef` or `#ifndef` is about.
    fn macro_name(&self, directive: &PpToken, args: &[PpToken]) -> Result<String, Error> {
        let Some(t) = args.first() else {
            return Err(Error::new(directive.end, "macro name missing"));
        };
        match t.ident() {
            Some("defined") => Err(Error::new(t.pos, "'defined' cannot be used as a macro name")),
            Some(name) => Ok(name.to_string()),
            None => Err(Error::new(t.pos, "macro name must be an identifier")),
        }
    }

    fn define(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let name = self.macro_name(directive, args)?;
        let name_pos = args[0].pos;
        let rest = &args[1..];
        // `F(` with no space before the `(` makes a function-like macro.
        let (kind, body) = if rest.first().is_some_and(|t| t.is_punct("(") && !t.space) {
            let mut params: Vec<String> = Vec::new();
            let mut variadic = false;
            let mut i = 1;
            let close = loop {
                let Some(t) = rest.get(i) else {
                    return Err(Error::new(directive.pos, "missing ')' in macro parameter list"));
                };
                i += 1;
                if t.is_punct(")") && params.is_empty() && !variadic {
                    break i;
                }
                if t.is_punct("...") {
                    variadic = true;
                } else if let Some(p) = t.ident() {
                    if p == "__VA_ARGS__" {
                        return Err(Error::new(
                            t.pos,
                            "'__VA_ARGS__' cannot be a macro parameter name",
                        ));
                    }
                    if params.iter().any(|q| q == p) {
                        return Err(Error::new(t.pos, format!("duplicate macro parameter name '{p}'")));
                    }
                    params.push(p.to_string());
                } else {
                    return Err(Error::new(t.pos, "invalid token in macro parameter list"));
                }
                match rest.get(i) {
                    Some(t) if t.is_punct(")") => break i + 1,
                    Some(t) if t.is_punct(",") && !variadic => i += 1,
                    Some(t) => return Err(Error::new(t.pos, "expected ',' or ')' in macro parameter list")),
                    None => return Err(Error::new(directive.pos, "missing ')' in macro parameter list")),
                }
            };
            (MacroKind::Function { params, variadic }, rest[close..].to_vec())
        } else {
            (MacroKind::Object, rest.to_vec())
        };
        let m = Macro {
            kind,
            body,
            pos: name_pos,
        };
        // What the body may hold: `##` between two tokens, `#` before a
        // parameter, `__VA_ARGS__` only in a variadic macro.
        if let Some(t) = m
            .body
            .first()
            .filter(|t| t.is_punct("##"))
            .or(m.body.last().filter(|t| t.is_punct("##")))
        {
            return Err(Error::new(
                t.pos,
                "'##' cannot appear at either end of a macro expansion",
            ));
        }
        for (i, t) in m.body.iter().enumerate() {
            let function = matches!(m.kind, MacroKind::Function { .. });
            if function && t.is_punct("#") && !m.body.get(i + 1).is_some_and(|n| m.param(n).is_some()) {
                return Err(Error::new(t.pos, "'#' is not followed by a macro parameter"));
            }
            if t.ident() == Some("__VA_ARGS__") && m.param(t).is_none() {
                return Err(Error::new(
                    t.pos,
                    "__VA_ARGS__ can only appear in the expansion of a variadic macro",
                ));
            }
        }
        if let Some(prev) = self.macros.get(&name) {
            if matches!(prev.kind, MacroKind::File | MacroKind::Line) {
                return Err(Error::new(
                    name_pos,
                    format!("cannot redefine the builtin macro '{name}'"),
                ));
            }
            if !self.command_line && !prev.same(&m) {
                let at = match self.files.get(prev.pos.file as usize) {
                    Some(f) if prev.pos.file != name_pos.file => format!("{}:{}", f.name, prev.pos),
                    _ => prev.pos.to_string(),
                };
                return Err(Error::new(
                    name_pos,
                    format!("'{name}' macro redefined (previous definition at {at})"),
                ));
            }
        }
        self.macros.insert(name, m);
        Ok(())
    }

    fn include(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let (header, angled, pos) = match header_name(args)? {
            Some(h) => h,
            None => {
                // `#include MACRO`: replace, then it must be one of the two forms.
                let toks = args
                    .iter()
                    .map(|t| PTok {
                        t: t.clone(),
                        hide: HideSet::default(),
                    })
                    .collect();
                let expanded: Vec<PpToken> = self.expand_alone(toks)?.into_iter().map(|t| t.t).collect();
                header_name(&expanded)?.ok_or_else(|| {
                    Error::new(
                        args.first().map_or(directive.end, |t| t.pos),
                        "expected \"FILENAME\" or <FILENAME>",
                    )
                })?
            }
        };
        if header.is_empty() {
            return Err(Error::new(pos, "empty filename"));
        }
        // `"file"`: next to the including file first, then the -I directories.
        let cur = self.stack.last().expect("a file").file as usize;
        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        if !angled {
            let dir = self.files[cur]
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let shown = Path::new(&self.files[cur].name)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            dirs.push((shown, dir));
        }
        for d in &self.opts.include_dirs {
            dirs.push((d.to_string_lossy().into_owned(), d.clone()));
        }
        for (shown, dir) in dirs {
            let path = normalize(&dir.join(&header));
            let Some(text) = (self.load)(&path) else {
                continue;
            };
            if self.once.contains(&path) {
                return Ok(());
            }
            if self.stack.len() >= MAX_INCLUDE_DEPTH {
                return Err(Error::new(pos, "#include nested too deeply"));
            }
            let name = if shown.is_empty() {
                header.clone()
            } else {
                format!("{}/{header}", shown.trim_end_matches(['/', '\\']))
            };
            let id = self.new_file(name, path);
            let (toks, _) = lexer::lex_pp(&text, id)?;
            self.enter(toks, id);
            return Ok(());
        }
        Err(Error::new(pos, format!("'{header}' file not found")))
    }

    /// `#line N` or `#line N "name"` (after macro replacement): the next
    /// line is line N (of a file called `name`).
    fn line(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<(), Error> {
        let toks = args
            .iter()
            .map(|t| PTok {
                t: t.clone(),
                hide: HideSet::default(),
            })
            .collect();
        let toks: Vec<PpToken> = self.expand_alone(toks)?.into_iter().map(|t| t.t).collect();
        let bad = || {
            Error::new(
                directive.pos,
                "#line takes a line number and an optional file name",
            )
        };
        let n: u32 = match toks.first() {
            Some(t) if t.kind == PpKind::Number && t.text.bytes().all(|b| b.is_ascii_digit()) => {
                t.text.parse().map_err(|_| bad())?
            }
            _ => return Err(bad()),
        };
        if n == 0 || n > 2_147_483_647 {
            return Err(Error::new(toks[0].pos, "#line number out of range"));
        }
        let rename = match &toks[1..] {
            [] => None,
            [t] if t.kind == PpKind::Str => Some(lexer::decode(&t.text, t.pos)?),
            _ => return Err(bad()),
        };
        // The directive's own line, as the lexer numbered it.
        let cur = self.stack.last().expect("a file");
        let written = i64::from(directive.pos.line) - cur.line_delta;
        let path = self.files[cur.file as usize].path.clone();
        let renamed = rename.map(|bytes| self.new_file(String::from_utf8_lossy(&bytes).into_owned(), path));
        let cur = self.stack.last_mut().expect("a file");
        cur.line_delta = i64::from(n) - (written + 1);
        if let Some(id) = renamed {
            cur.file = id;
        }
        Ok(())
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

    fn open_cond(&mut self, pos: Pos, value: bool) -> Result<(), Error> {
        self.conds.push(Cond {
            pos,
            taken: value,
            seen_else: false,
        });
        if value {
            Ok(())
        } else {
            self.skip_group()
        }
    }

    /// Skips a group of the innermost conditional, and any that follow it,
    /// until one is taken (an `#elif` whose condition holds, or `#else`, if
    /// none was taken before) or its `#endif` closes it. Only directives
    /// are looked at, to follow the nesting of conditionals.
    fn skip_group(&mut self) -> Result<(), Error> {
        let mut depth = 0;
        loop {
            match self.peek_file() {
                None => {
                    return Err(Error::new(
                        self.conds.last().expect("open").pos,
                        "unterminated conditional directive",
                    ))
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
                Some("if" | "ifdef" | "ifndef") => depth += 1,
                Some("endif") if depth > 0 => depth -= 1,
                Some("endif") => {
                    no_more(args, "#endif")?;
                    self.conds.pop();
                    return Ok(());
                }
                Some(d @ ("elif" | "else")) if depth == 0 => {
                    let cond = self.conds.last_mut().expect("open");
                    if cond.seen_else {
                        return Err(Error::new(first.pos, format!("#{d} after #else")));
                    }
                    if d == "else" {
                        no_more(args, "#else")?;
                        cond.seen_else = true;
                        if !cond.taken {
                            cond.taken = true;
                            return Ok(());
                        }
                    } else if !cond.taken && self.condition(first, args)? {
                        self.conds.last_mut().expect("open").taken = true;
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }

    /// The value of an `#if` or `#elif` condition.
    fn condition(&mut self, directive: &PpToken, args: &[PpToken]) -> Result<bool, Error> {
        // `defined NAME` and `defined(NAME)` first, before macros are replaced.
        let mut toks = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let t = &args[i];
            i += 1;
            if t.ident() != Some("defined") {
                toks.push(PTok {
                    t: t.clone(),
                    hide: HideSet::default(),
                });
                continue;
            }
            let paren = args.get(i).is_some_and(|t| t.is_punct("("));
            if paren {
                i += 1;
            }
            let Some(name) = args.get(i).and_then(PpToken::ident) else {
                return Err(Error::new(
                    args.get(i).map_or(t.end, |t| t.pos),
                    "'defined' needs a macro name",
                ));
            };
            i += 1;
            if paren {
                if !args.get(i).is_some_and(|t| t.is_punct(")")) {
                    return Err(Error::new(
                        args.get(i).map_or(args[i - 1].end, |t| t.pos),
                        "expected ')' after the macro name",
                    ));
                }
                i += 1;
            }
            let value = if self.macros.contains_key(name) { "1" } else { "0" };
            toks.push(PTok {
                t: PpToken {
                    kind: PpKind::Number,
                    text: value.to_string(),
                    ..t.clone()
                },
                hide: HideSet::default(),
            });
        }
        let toks: Vec<PpToken> = self.expand_alone(toks)?.into_iter().map(|t| t.t).collect();
        let mut e = Eval {
            toks: &toks,
            i: 0,
            end: args.last().map_or(directive.end, |t| t.end),
        };
        if toks.is_empty() {
            return Err(Error::new(
                directive.end,
                format!("#{} with no expression", directive.text),
            ));
        }
        let v = e.cond(true)?;
        if let Some(t) = toks.get(e.i) {
            return Err(Error::new(
                t.pos,
                format!("unexpected '{}' in preprocessor expression", t.text),
            ));
        }
        Ok(v.bits != 0)
    }
}

/// Errors if a directive has tokens past what it takes.
fn no_more(rest: &[PpToken], directive: &str) -> Result<(), Error> {
    match rest.first() {
        Some(t) => Err(Error::new(
            t.pos,
            format!("extra tokens at end of {directive} directive"),
        )),
        None => Ok(()),
    }
}

/// `"file"` or `<file>` at the start of an `#include`'s tokens, with
/// whether it was `<file>` and where it is; `None` if neither form is there.
fn header_name(toks: &[PpToken]) -> Result<Option<(String, bool, Pos)>, Error> {
    let Some(first) = toks.first() else {
        return Ok(None);
    };
    let (name, angled, used) = if first.kind == PpKind::Str {
        // Not a string literal: the characters between the quotes, as they are.
        (first.text[1..first.text.len() - 1].to_string(), false, 1)
    } else if first.is_punct("<") {
        let Some(close) = toks.iter().position(|t| t.is_punct(">")) else {
            return Err(Error::new(first.pos, "expected '>' after the file name"));
        };
        let mut name = String::new();
        for (k, t) in toks[1..close].iter().enumerate() {
            if k > 0 && t.space {
                name.push(' ');
            }
            name.push_str(&t.text);
        }
        (name, true, close + 1)
    } else {
        return Ok(None);
    };
    no_more(&toks[used..], "#include")?;
    Ok(Some((name, angled, first.pos)))
}

/// Tokens as written, a space where there was whitespace between them:
/// what `#error` prints.
fn spelling(toks: &[PpToken]) -> String {
    let mut s = String::new();
    for (k, t) in toks.iter().enumerate() {
        if k > 0 && t.space {
            s.push(' ');
        }
        s.push_str(&t.text);
    }
    s
}

/// `\` and `"` escaped, to go between double quotes.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `#arg`: the argument's spelling as a string literal, with the `\` and
/// `"` of string literals and character constants in it escaped. Other
/// backslashes stay as they are, so an argument that ends in one (`s(\)`)
/// has no valid literal (C leaves it undefined): `None`.
fn stringize(arg: &[PTok]) -> Option<String> {
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
    if trailing % 2 == 1 {
        return None;
    }
    s.push('"');
    Some(s)
}

/// `left ## right`: the two spellings joined, which must read as one token.
fn glue(left: PTok, right: PTok) -> Result<PTok, Error> {
    if left.t.kind == PpKind::Placemarker {
        return Ok(right);
    }
    if right.t.kind == PpKind::Placemarker {
        return Ok(left);
    }
    let text = format!("{}{}", left.t.text, right.t.text);
    let one = match lexer::lex_pp(&text, left.t.pos.file) {
        Ok((toks, _))
            if toks.len() == 1 && toks[0].text == text && !matches!(toks[0].kind, PpKind::Invalid(_)) =>
        {
            Some(toks[0].kind.clone())
        }
        _ => None,
    };
    let Some(kind) = one else {
        return Err(Error::new(
            left.t.pos,
            format!("pasting formed '{text}', an invalid preprocessing token"),
        ));
    };
    Ok(PTok {
        t: PpToken { kind, text, ..left.t },
        hide: left.hide,
    })
}

/// A value in `#if` arithmetic: 64 bits, signed (`intmax_t`) or not
/// (`uintmax_t`).
#[derive(Clone, Copy)]
struct Val {
    bits: u64,
    unsigned: bool,
}

impl Val {
    fn signed(v: i64) -> Val {
        Val {
            bits: v as u64,
            unsigned: false,
        }
    }

    fn truth(b: bool) -> Val {
        Val::signed(i64::from(b))
    }
}

/// Evaluates an `#if` expression (C99 6.10.1): an integer constant
/// expression over 64-bit values, every operator but assignments, `++`,
/// `--`, `,` and `sizeof`. `&&`, `||` and `?:` evaluate only what they
/// need (`eval` false below them): division by zero there is no error.
struct Eval<'t> {
    toks: &'t [PpToken],
    i: usize,
    /// Where the expression ends, for "expected value" at the end.
    end: Pos,
}

impl Eval<'_> {
    fn peek(&self) -> Option<&PpToken> {
        self.toks.get(self.i)
    }

    fn eat(&mut self, p: &str) -> bool {
        let yes = self.peek().is_some_and(|t| t.is_punct(p));
        if yes {
            self.i += 1;
        }
        yes
    }

    fn here(&self) -> Pos {
        self.peek().map_or(self.end, |t| t.pos)
    }

    fn cond(&mut self, eval: bool) -> Result<Val, Error> {
        let c = self.binary(1, eval)?;
        if !self.eat("?") {
            return Ok(c);
        }
        let yes = c.bits != 0;
        let a = self.cond(eval && yes)?;
        if !self.eat(":") {
            return Err(Error::new(self.here(), "expected ':' in preprocessor expression"));
        }
        let b = self.cond(eval && !yes)?;
        let unsigned = a.unsigned || b.unsigned;
        let v = if yes { a } else { b };
        Ok(Val {
            bits: v.bits,
            unsigned,
        })
    }

    fn binary(&mut self, min: u8, eval: bool) -> Result<Val, Error> {
        let mut l = self.unary(eval)?;
        loop {
            let Some(PpKind::Punct(op)) = self.peek().map(|t| t.kind.clone()) else {
                return Ok(l);
            };
            let prec = match op {
                "||" => 1,
                "&&" => 2,
                "|" => 3,
                "^" => 4,
                "&" => 5,
                "==" | "!=" => 6,
                "<" | ">" | "<=" | ">=" => 7,
                "<<" | ">>" => 8,
                "+" | "-" => 9,
                "*" | "/" | "%" => 10,
                _ => return Ok(l),
            };
            if prec < min {
                return Ok(l);
            }
            let pos = self.here();
            self.i += 1;
            // The right side of `&&` and `||` is evaluated only if it matters.
            let eval_r = match op {
                "&&" => eval && l.bits != 0,
                "||" => eval && l.bits == 0,
                _ => eval,
            };
            let r = self.binary(prec + 1, eval_r)?;
            l = apply(op, l, r, eval, pos)?;
        }
    }

    fn unary(&mut self, eval: bool) -> Result<Val, Error> {
        let pos = self.here();
        let Some(t) = self.peek().cloned() else {
            return Err(Error::new(pos, "expected value in preprocessor expression"));
        };
        self.i += 1;
        match &t.kind {
            PpKind::Punct("(") => {
                let v = self.cond(eval)?;
                if !self.eat(")") {
                    return Err(Error::new(self.here(), "expected ')' in preprocessor expression"));
                }
                Ok(v)
            }
            PpKind::Punct(op @ ("+" | "-" | "~" | "!")) => {
                let v = self.unary(eval)?;
                Ok(match *op {
                    "+" => v,
                    "-" => Val {
                        bits: v.bits.wrapping_neg(),
                        unsigned: v.unsigned,
                    },
                    "~" => Val {
                        bits: !v.bits,
                        unsigned: v.unsigned,
                    },
                    _ => Val::truth(v.bits == 0),
                })
            }
            PpKind::Number => {
                let lit = lexer::integer(&t.text, t.pos)?;
                let Some(bits) = lit.value else {
                    return Err(Error::new(
                        t.pos,
                        "integer literal is too large to be represented in any integer type",
                    ));
                };
                Ok(Val {
                    bits,
                    unsigned: lit.unsigned || bits > i64::MAX as u64,
                })
            }
            PpKind::Char => match lexer::decode(&t.text, t.pos)?[..] {
                [b] => Ok(Val::signed(i64::from(b))),
                _ => Err(Error::new(t.pos, "character constants hold one byte in merlin")),
            },
            PpKind::Ident if t.text == "defined" => Err(Error::new(
                t.pos,
                "'defined' produced by macro expansion is not supported",
            )),
            // An identifier that is no macro: 0.
            PpKind::Ident => Ok(Val::signed(0)),
            _ => Err(Error::new(
                t.pos,
                format!("unexpected '{}' in preprocessor expression", t.text),
            )),
        }
    }
}

/// `l op r` in `#if` arithmetic: unsigned if either side is (except for
/// shifts, which take the left side's type), wrapping.
fn apply(op: &str, l: Val, r: Val, eval: bool, pos: Pos) -> Result<Val, Error> {
    let unsigned = l.unsigned || r.unsigned;
    let (a, b) = (l.bits, r.bits);
    let (sa, sb) = (a as i64, b as i64);
    let arith = |bits: u64| Val { bits, unsigned };
    let cmp = |lt: bool| Val::truth(lt);
    Ok(match op {
        "||" => Val::truth(a != 0 || b != 0),
        "&&" => Val::truth(a != 0 && b != 0),
        "|" => arith(a | b),
        "^" => arith(a ^ b),
        "&" => arith(a & b),
        "==" => Val::truth(a == b),
        "!=" => Val::truth(a != b),
        "<" => cmp(if unsigned { a < b } else { sa < sb }),
        ">" => cmp(if unsigned { a > b } else { sa > sb }),
        "<=" => cmp(if unsigned { a <= b } else { sa <= sb }),
        ">=" => cmp(if unsigned { a >= b } else { sa >= sb }),
        "<<" => Val {
            bits: a.wrapping_shl(b as u32),
            unsigned: l.unsigned,
        },
        ">>" => Val {
            bits: if l.unsigned {
                a.wrapping_shr(b as u32)
            } else {
                sa.wrapping_shr(b as u32) as u64
            },
            unsigned: l.unsigned,
        },
        "+" => arith(a.wrapping_add(b)),
        "-" => arith(a.wrapping_sub(b)),
        "*" => arith(a.wrapping_mul(b)),
        "/" | "%" => {
            if b == 0 {
                if !eval {
                    return Ok(arith(0));
                }
                return Err(Error::new(pos, "division by zero in preprocessor expression"));
            }
            let v = match (op, unsigned) {
                ("/", true) => a / b,
                ("/", false) => sa.wrapping_div(sb) as u64,
                (_, true) => a % b,
                (_, false) => sa.wrapping_rem(sb) as u64,
            };
            arith(v)
        }
        _ => unreachable!("binary() only passes the operators above"),
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
        preprocess("main.c", src, opts, &load)
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
        // escape the closing quote, which C leaves undefined. Two are fine.
        assert_eq!(
            err("#define s(x) #x\nconst char *p = s(\\);"),
            "main.c:2:17: error: '#' produced an invalid string literal: it ends in a lone '\\'"
        );
        assert!(err("#define s(x) #x\ns(a \\)").contains("lone '\\'"));
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
        assert_eq!(
            err("#define cat(a, b) a ## b\ncat(+, -)"),
            "main.c:2:5: error: pasting formed '+-', an invalid preprocessing token"
        );
        assert_eq!(
            err("#define cat(a, b) a ## b\ncat(/, /)"),
            "main.c:2:5: error: pasting formed '//', an invalid preprocessing token"
        );
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
            same(r#""h.h" 1"#)
        );
        // #line renumbers, and renames.
        assert_eq!(
            pp("#line 100\n__LINE__\n#line 7 \"x.c\"\n__LINE__ __FILE__"),
            same(r#"100 7 "x.c""#)
        );
        assert_eq!(err("#line 0"), "main.c:1:7: error: #line number out of range");
        assert_eq!(
            err("#line x"),
            "main.c:1:2: error: #line takes a line number and an optional file name"
        );
        // A backslash in a file name is escaped.
        let out = preprocess("dir\\a.c", "__FILE__", &Options::default(), &|_| None).unwrap();
        assert_eq!(out.tokens[0].text, r#""dir\\a.c""#);
        assert_eq!(
            err("#define __LINE__ 1"),
            "main.c:1:9: error: cannot redefine the builtin macro '__LINE__'"
        );
        assert_eq!(
            err("#undef __FILE__"),
            "main.c:1:8: error: cannot undefine the builtin macro '__FILE__'"
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
        };
        let src = "#include <d.h>\n#include \"d.h\"\n#include <c.h>\n#define H <b.h>\n#include H";
        let out = run(&files, src, &opts).unwrap();
        assert_eq!(words(&out.tokens), same("int d; int d; int wrong; int b; int c;"));
        // Files are named by where they were found.
        assert_eq!(
            out.files[2..],
            ["inc/d.h", "inc/d.h", "inc/c.h", "sub/b.h", "sub/c.h"]
        );
        // `<...>` does not look next to the including file.
        assert_eq!(
            err_with(&files, "#include <a.h>"),
            "main.c:1:10: error: 'a.h' file not found"
        );
        assert_eq!(
            err_with(&files, "#include \"nope.h\""),
            "main.c:1:10: error: 'nope.h' file not found"
        );
        assert_eq!(
            err("#include"),
            "main.c:1:9: error: expected \"FILENAME\" or <FILENAME>"
        );
        assert_eq!(
            err("#include \"a.h\" x"),
            "main.c:1:16: error: extra tokens at end of #include directive"
        );
        assert_eq!(
            err("#include <a.h"),
            "main.c:1:10: error: expected '>' after the file name"
        );
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
        assert_eq!(
            err_with(&files, "#include \"self.h\""),
            "self.h:1:10: error: #include nested too deeply"
        );
    }

    #[test]
    fn errors_in_headers_name_the_header() {
        let files = [("bad.h", "\n#if 1\n"), ("worse.h", "#error stop  here")];
        assert_eq!(
            err_with(&files, "#include \"bad.h\""),
            "bad.h:2:2: error: unterminated conditional directive"
        );
        assert_eq!(
            err_with(&files, "#include \"worse.h\""),
            "worse.h:1:2: error: #error stop here"
        );
        // A conditional must end in the file that opened it.
        let files = [("end.h", "#endif\n")];
        assert_eq!(
            err_with(&files, "#if 1\n#include \"end.h\"\n#endif"),
            "end.h:1:2: error: #endif without #if"
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
        assert_eq!(
            err("#if 1 / 0\n#endif"),
            "main.c:1:7: error: division by zero in preprocessor expression"
        );
        assert_eq!(err("#if\n#endif"), "main.c:1:4: error: #if with no expression");
        assert_eq!(
            err("#if 1 +\n#endif"),
            "main.c:1:8: error: expected value in preprocessor expression"
        );
        assert_eq!(
            err("#if (1\n#endif"),
            "main.c:1:7: error: expected ')' in preprocessor expression"
        );
        assert_eq!(
            err("#if 1 2\n#endif"),
            "main.c:1:7: error: unexpected '2' in preprocessor expression"
        );
        assert_eq!(
            err("#if 1 = 2\n#endif"),
            "main.c:1:7: error: unexpected '=' in preprocessor expression"
        );
        // Past 64 bits a literal has no type at all: an error, not u64::MAX.
        assert!(truth("18446744073709551615 == 0xffffffffffffffff"));
        for big in [
            "18446744073709551616",
            "0x1ffffffffffffffff",
            "99999999999999999999999",
        ] {
            assert_eq!(
                err(&format!("#if {big} == 0\n#endif")),
                "main.c:1:5: error: integer literal is too large to be represented in any integer type"
            );
        }
        assert_eq!(
            err("#if 1.0\n#endif"),
            "main.c:1:5: error: floating-point literals are not supported"
        );
        assert_eq!(
            err("#if defined\n#endif"),
            "main.c:1:12: error: 'defined' needs a macro name"
        );
        assert_eq!(
            err("#if defined(X\n#endif"),
            "main.c:1:14: error: expected ')' after the macro name"
        );
        assert_eq!(
            err("#define D defined(X)\n#if D\n#endif"),
            "main.c:2:5: error: 'defined' produced by macro expansion is not supported"
        );
    }

    #[test]
    fn conditional_errors() {
        assert_eq!(
            err("#if 1\nx"),
            "main.c:1:2: error: unterminated conditional directive"
        );
        assert_eq!(
            err("#if 0\nx"),
            "main.c:1:2: error: unterminated conditional directive"
        );
        assert_eq!(err("#else"), "main.c:1:2: error: #else without #if");
        assert_eq!(err("#elif 1"), "main.c:1:2: error: #elif without #if");
        assert_eq!(err("#endif"), "main.c:1:2: error: #endif without #if");
        assert_eq!(
            err("#if 1\n#else\n#else\n#endif"),
            "main.c:3:2: error: #else after #else"
        );
        assert_eq!(
            err("#if 0\n#else\n#elif 1\n#endif"),
            "main.c:3:2: error: #elif after #else"
        );
        assert_eq!(
            err("#if 1\n#endif X"),
            "main.c:2:8: error: extra tokens at end of #endif directive"
        );
        assert_eq!(err("#ifdef\n#endif"), "main.c:1:7: error: macro name missing");
        assert_eq!(
            err("#ifdef 1\n#endif"),
            "main.c:1:8: error: macro name must be an identifier"
        );
    }

    #[test]
    fn directives() {
        assert_eq!(pp("#\n# /* nothing */\nx"), same("x"), "null directives");
        assert_eq!(pp("#pragma once\n#pragma something else\nx"), same("x"));
        assert_eq!(
            err("#error \"stop\" now"),
            "main.c:1:2: error: #error \"stop\" now"
        );
        assert_eq!(
            err("#bogus"),
            "main.c:1:2: error: invalid preprocessing directive '#bogus'"
        );
        assert_eq!(
            err("# 33 \"x.c\""),
            "main.c:1:1: error: invalid preprocessing directive '#33'"
        );
        // `#` starts a directive only at the start of a line.
        assert_eq!(pp("x # define"), same("x # define"));
    }

    #[test]
    fn definition_errors() {
        assert_eq!(err("#define"), "main.c:1:8: error: macro name missing");
        assert_eq!(
            err("#define 1 2"),
            "main.c:1:9: error: macro name must be an identifier"
        );
        assert_eq!(
            err("#define defined"),
            "main.c:1:9: error: 'defined' cannot be used as a macro name"
        );
        assert_eq!(
            err("#define N 1\n#define N 2"),
            "main.c:2:9: error: 'N' macro redefined (previous definition at 1:9)"
        );
        assert_eq!(
            err("#define F(a) a\n#define F(b) b"),
            "main.c:2:9: error: 'F' macro redefined (previous definition at 1:9)"
        );
        assert_eq!(
            err("#define N 1+1\n#define N 1 + 1"),
            "main.c:2:9: error: 'N' macro redefined (previous definition at 1:9)"
        );
        assert_eq!(
            err("#define F(a"),
            "main.c:1:2: error: missing ')' in macro parameter list"
        );
        assert_eq!(
            err("#define F(a,)"),
            "main.c:1:13: error: invalid token in macro parameter list"
        );
        assert_eq!(
            err("#define F(a b)"),
            "main.c:1:13: error: expected ',' or ')' in macro parameter list"
        );
        assert_eq!(
            err("#define F(a, a)"),
            "main.c:1:14: error: duplicate macro parameter name 'a'"
        );
        assert_eq!(
            err("#define F(..., a)"),
            "main.c:1:14: error: expected ',' or ')' in macro parameter list"
        );
        assert_eq!(
            err("#define F(x) #y"),
            "main.c:1:14: error: '#' is not followed by a macro parameter"
        );
        assert_eq!(
            err("#define F(x) ## x"),
            "main.c:1:14: error: '##' cannot appear at either end of a macro expansion"
        );
        assert_eq!(
            err("#define F x ##"),
            "main.c:1:13: error: '##' cannot appear at either end of a macro expansion"
        );
        assert_eq!(
            err("#define F(x) __VA_ARGS__"),
            "main.c:1:14: error: __VA_ARGS__ can only appear in the expansion of a variadic macro"
        );
        // `#` in an object-like macro is just a token.
        assert_eq!(pp("#define H # x\nH"), same("# x"));
    }

    #[test]
    fn call_errors() {
        assert_eq!(
            err("#define F(a, b) a\nF(1)"),
            "main.c:2:1: error: too few arguments provided to function-like macro invocation of 'F' (expected 2, have 1)"
        );
        assert_eq!(
            err("#define F(a) a\nF(1, 2)"),
            "main.c:2:1: error: too many arguments provided to function-like macro invocation of 'F' (expected 1, have 2)"
        );
        assert_eq!(
            err("#define F() 1\nF(x)"),
            "main.c:2:1: error: too many arguments provided to function-like macro invocation of 'F' (expected 0, have 1)"
        );
        assert_eq!(
            err("#define F(a) a\nF(1"),
            "main.c:2:1: error: unterminated call of the function-like macro 'F'"
        );
        assert_eq!(
            err("#define F(a) a\nF(1\n#define X\n)"),
            "main.c:3:1: error: directives inside the arguments of a macro call are not supported"
        );
        // A call may not run past the end of a header.
        let files = [("open.h", "#define F(a) a\nF(1")];
        assert_eq!(
            err_with(&files, "#include \"open.h\"\n)"),
            "open.h:2:1: error: unterminated call of the function-like macro 'F'"
        );
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
        };
        let out = run(&[], "ONE TWO F(3) GONE __merlin__ __STDC_VERSION__", &opts).unwrap();
        assert_eq!(words(&out.tokens), same("1 22 (3+1) GONE __merlin__ 199901L"));
        let opts = Options {
            include_dirs: Vec::new(),
            macros: vec![CmdMacro::Define("1X".into())],
        };
        assert_eq!(
            run(&[], "", &opts).unwrap_err().render("main.c"),
            "<command line>:1:9: error: macro name must be an identifier"
        );
    }

    #[test]
    fn predefined_macros() {
        assert_eq!(
            pp("__STDC__ __STDC_HOSTED__ __merlin__ __riscv __riscv_xlen __ILP32__ __CHAR_UNSIGNED__"),
            same("1 0 1 1 32 1 1")
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
}
