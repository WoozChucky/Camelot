//! Debug information (`-g`): where each instruction comes from in the source,
//! and what the program's variables and types are, for debuggers (merlin-dbg,
//! lldb) and `merlin-objdump -l`. The spec's section 10 ("Debug information")
//! says what is described and how.
//!
//! merlin writes it as assembly, the way clang does, and leaves the encoding of
//! the line table to the assembler:
//!
//! * `.file N "dir" "name"` numbers the source files: 0 is the file being
//!   compiled, with the *compilation directory* (`-fdebug-compilation-dir`, the
//!   directory the names are relative to); then each header that has code or
//!   declares a described entry, numbered as LLVM numbers them (see
//!   [`Lines::renumber`]).
//! * `.loc FILE LINE COLUMN` goes before the code of every statement (see
//!   [`crate::codegen`]): a function's first line (column 0) before its
//!   prologue, each statement's start, a loop's condition and step each time
//!   they run, and the closing brace before the epilogue. The first `.loc`
//!   after the prologue says `prologue_end`, where a debugger stops when it
//!   steps into the function. merlin-as turns them into the `.debug_line`
//!   line table.
//!
//! The rest is DWARF 5 debugging information entries (DIEs), in
//! `.debug_info`, with their abbreviations in `.debug_abbrev`. merlin builds
//! the tree of entries ([`Die`]), lays it out to know where each entry starts
//! (a type is referred to by its offset in the unit, `DW_FORM_ref4`), and
//! writes every byte with `.byte`, `.half`, `.word`, `.quad` and `.asciz`:
//!
//! * a *compile unit* says what the line table belongs to: the file, the
//!   compilation directory, the address range of the code and the offset of
//!   the line table (`DW_AT_stmt_list`). Debuggers find line tables through
//!   compile units: without one, lldb, llvm-objdump and merlin-objdump ignore
//!   `.debug_line`;
//! * under it, a `DW_TAG_subprogram` per function: its name, address range,
//!   line, type, and its *frame base*, `s0` (`DW_OP_reg8`), the canonical frame
//!   address once the prologue has run. Every parameter and local lives in the
//!   frame at a fixed offset from `s0` (see [`crate::codegen`], "Stack
//!   frame"), so its location is `DW_OP_fbreg OFFSET`. Its parameters are
//!   `DW_TAG_formal_parameter`s, its locals `DW_TAG_variable`s, and each block
//!   that declares variables in the frame a `DW_TAG_lexical_block` with its
//!   own address range, nested as the blocks are. A `static` local is at its
//!   symbol (`DW_OP_addrx`), and is the function's whatever block declares
//!   it, after the frame's variables and the blocks, as clang has them;
//! * a `DW_TAG_variable` per global variable the file defines, at its symbol,
//!   and one without a name per string literal; a `DW_TAG_label` per `goto`
//!   label, in its block;
//! * the types they use, each described once: base types (`int`, `unsigned
//!   char`, ...), pointers, arrays (one `DW_TAG_subrange_type` per dimension,
//!   as clang does), structs and unions with their members and offsets (an
//!   anonymous struct, union or enum a member is of inside the record, and
//!   a type a function defines inside the function's entry),
//!   enums with their constants, typedefs, `const` and `volatile`, and
//!   function types (for function pointers); and, as clang keeps them,
//!   the types of the explicit casts its code generator visits and the
//!   enums whose constants the code uses.
//!
//! The entries are in clang's order, which [`order`] works out from what the
//! code generator saw ([`Event`]s), in the order it generated the functions
//! and globals (clang's front end's) and the order of their code: the order
//! LLVM makes them in.
//!
//! The forms are clang's for DWARF 5 (see [`Val`]): a string is an index in
//! `.debug_str_offsets`, whose entries point into `.debug_str`
//! (`DW_FORM_strx1` and up), an address an index in `.debug_addr`
//! (`DW_FORM_addrx`, `DW_OP_addrx`), each string and address there once, and
//! a constant the smallest `data` form that holds it. [`Die::pool`] gives them
//! their indices before the abbreviations are made.
//!
//! Addresses in `.debug_addr` are the target's: 4 bytes written `.word` on
//! RV32, 8 bytes written `.quad` on RV64, as clang writes them, and the
//! unit's header and the table's say which (the address size). merlin-as
//! turns each into a relocation (`R_RISCV_32` or `R_RISCV_64`), and merlin-ld
//! applies it.
//!
//! None of it changes the code: labels and directives assemble to no bytes
//! in `.text`, and the debug sections are not loaded.

mod order;

use crate::types::{ptr_size, IntKind, Quals, Record, Spelled, Spelling, Type, Xlen};
use crate::Pos;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

/// `-g`: emit debug information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugInfo {
    /// The directory the file names are relative to, as clang's
    /// `-fdebug-compilation-dir` (by default the directory merlin runs in).
    pub comp_dir: String,
}

/// `DW_TAG_*`, `DW_AT_*`, `DW_FORM_*`, `DW_ATE_*`, `DW_OP_*` (DWARF 5, section 7).
const TAG_ARRAY_TYPE: u8 = 0x01;
const TAG_ENUMERATION_TYPE: u8 = 0x04;
const TAG_FORMAL_PARAMETER: u8 = 0x05;
const TAG_LABEL: u8 = 0x0a;
const TAG_LEXICAL_BLOCK: u8 = 0x0b;
const TAG_MEMBER: u8 = 0x0d;
const TAG_POINTER_TYPE: u8 = 0x0f;
const TAG_COMPILE_UNIT: u8 = 0x11;
const TAG_STRUCTURE_TYPE: u8 = 0x13;
const TAG_SUBROUTINE_TYPE: u8 = 0x15;
const TAG_TYPEDEF: u8 = 0x16;
const TAG_UNION_TYPE: u8 = 0x17;
const TAG_UNSPECIFIED_PARAMETERS: u8 = 0x18;
const TAG_SUBRANGE_TYPE: u8 = 0x21;
const TAG_BASE_TYPE: u8 = 0x24;
const TAG_CONST_TYPE: u8 = 0x26;
const TAG_ENUMERATOR: u8 = 0x28;
const TAG_SUBPROGRAM: u8 = 0x2e;
const TAG_VARIABLE: u8 = 0x34;
const TAG_VOLATILE_TYPE: u8 = 0x35;

const AT_LOCATION: u8 = 0x02;
const AT_NAME: u8 = 0x03;
const AT_BYTE_SIZE: u8 = 0x0b;
const AT_STMT_LIST: u8 = 0x10;
const AT_LOW_PC: u8 = 0x11;
const AT_HIGH_PC: u8 = 0x12;
const AT_LANGUAGE: u8 = 0x13;
const AT_COMP_DIR: u8 = 0x1b;
const AT_CONST_VALUE: u8 = 0x1c;
const AT_PRODUCER: u8 = 0x25;
const AT_PROTOTYPED: u8 = 0x27;
const AT_COUNT: u8 = 0x37;
const AT_DATA_MEMBER_LOCATION: u8 = 0x38;
const AT_DECL_COLUMN: u8 = 0x39;
const AT_DECL_FILE: u8 = 0x3a;
const AT_DECL_LINE: u8 = 0x3b;
const AT_DECLARATION: u8 = 0x3c;
const AT_ENCODING: u8 = 0x3e;
const AT_EXTERNAL: u8 = 0x3f;
const AT_FRAME_BASE: u8 = 0x40;
const AT_TYPE: u8 = 0x49;
const AT_STR_OFFSETS_BASE: u8 = 0x72;
const AT_ADDR_BASE: u8 = 0x73;

const FORM_DATA2: u8 = 0x05;
const FORM_DATA4: u8 = 0x06;
const FORM_DATA8: u8 = 0x07;
const FORM_DATA1: u8 = 0x0b;
const FORM_SDATA: u8 = 0x0d;
const FORM_UDATA: u8 = 0x0f;
const FORM_REF4: u8 = 0x13;
const FORM_SEC_OFFSET: u8 = 0x17;
const FORM_EXPRLOC: u8 = 0x18;
const FORM_FLAG_PRESENT: u8 = 0x19;
const FORM_ADDRX: u8 = 0x1b;
const FORM_STRX1: u8 = 0x25;
const FORM_STRX2: u8 = 0x26;
const FORM_STRX3: u8 = 0x27;
const FORM_STRX4: u8 = 0x28;

const ATE_BOOLEAN: u8 = 0x02;
const ATE_SIGNED: u8 = 0x05;
const ATE_SIGNED_CHAR: u8 = 0x06;
const ATE_UNSIGNED: u8 = 0x07;
const ATE_UNSIGNED_CHAR: u8 = 0x08;

const OP_ADDRX: u8 = 0xa1;
const OP_REG8: u8 = 0x58; // DW_OP_reg0 + 8: s0, the frame pointer
const OP_FBREG: u8 = 0x91;

const LANG_C99: u16 = 0x0c;
const UT_COMPILE: u8 = 1;

/// `v` in unsigned LEB128.
fn uleb(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

/// `v` in signed LEB128.
fn sleb(mut v: i64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

/// Where a variable lives.
#[derive(Clone, Debug)]
pub(crate) enum Location {
    /// In the frame, at this offset from `s0`.
    Frame(i32),
    /// At a symbol: a global or a `static` local.
    Symbol(String),
}

/// A parameter or a variable: its name, where it is declared, its type
/// (an index in the type table, none for `void`) and where it lives.
#[derive(Clone, Debug)]
pub(crate) struct Var {
    name: String,
    file: u32,
    line: u32,
    ty: Option<usize>,
    location: Location,
}

/// A block's variables in the frame and the blocks inside it that declare
/// some, its `static` variables and those of the blocks inside it, and the
/// labels around its code.
#[derive(Clone, Debug, Default)]
pub(crate) struct Scope {
    pub begin: String,
    pub end: String,
    vars: Vec<Var>,
    /// The `goto` labels in it, after its variables, as clang has them.
    labels: Vec<Label>,
    blocks: Vec<Scope>,
    /// No lexical block of its own: a block without variables in the frame
    /// whose labels still are, in the block around it, where it is.
    transparent: bool,
    /// clang describes every `static` local of a function in the function's
    /// entry, after its variables in the frame and its blocks, in the order they
    /// are declared, whatever block declares them: a block's statics go up to
    /// the block around it as the block ends, and from there to the function.
    statics: Vec<Var>,
}

impl Scope {
    pub fn new(begin: String, end: String) -> Scope {
        Scope {
            begin,
            end,
            ..Scope::default()
        }
    }

    /// The files of the scope's `goto` labels and its blocks', in order.
    fn label_files(&self, out: &mut Vec<u32>) {
        out.extend(self.labels.iter().map(|l| l.file));
        for b in &self.blocks {
            b.label_files(out);
        }
    }

    /// Adds a `goto` label of the scope.
    pub fn push_label(&mut self, l: Label) {
        self.labels.push(l);
    }

    /// Adds a variable declared in the scope.
    pub fn push(&mut self, v: Var) {
        match v.location {
            Location::Frame(_) => self.vars.push(v),
            Location::Symbol(_) => self.statics.push(v),
        }
    }

    /// Adds `inner`, the scope of a block in this one, once its code is
    /// generated: as a lexical block if it declares variables in the frame,
    /// else its own blocks go into this one (an entry with nothing in it says
    /// nothing); its statics come to this scope.
    pub fn add_block(&mut self, mut inner: Scope) {
        self.statics.append(&mut inner.statics);
        if inner.vars.is_empty() && inner.labels.is_empty() {
            self.blocks.extend(inner.blocks);
        } else {
            inner.transparent = inner.vars.is_empty();
            self.blocks.push(inner);
        }
    }
}

/// A `goto` label: its name, where it is and its code's label.
#[derive(Clone, Debug)]
pub(crate) struct Label {
    name: String,
    file: u32,
    line: u32,
    col: u32,
    sym: String,
}

/// A function, for its `DW_TAG_subprogram`.
struct Function {
    name: String,
    /// The label just past its code.
    end: String,
    /// Its `.file` number and line.
    file: u32,
    line: u32,
    /// Where it starts (its name) and its body ends.
    span: (Pos, Pos),
    external: bool,
    /// Declared with a prototype (not `()`).
    prototyped: bool,
    ret: Option<usize>,
    /// What its code did that clang's front end does too, in order: see
    /// [`Event`].
    events: Vec<Event>,
    params: Vec<Var>,
    variadic: bool,
    /// The variables of its body, and its blocks.
    body: Scope,
}

/// A type in the table: what its entry says. Each is described once, and
/// variables, members and other types refer to it by its index.
enum TypeNode {
    Base(IntKind),
    /// The type `size_t`-like counts of array subranges have, as clang names
    /// it.
    ArraySize,
    /// A pointer to the type (none: to `void`).
    Ptr(Option<usize>),
    /// An array of the element type, with each dimension's count (none: of
    /// unknown size), outermost first.
    Array(usize, Vec<Option<u32>>),
    /// A struct or union, its members (filled in after the entry is in the
    /// table, as a member may point back to the record: each one's name,
    /// type, offset, and `.file` number and line), and its own `.file`
    /// number and line.
    Record(Arc<Record>, Vec<(String, Option<usize>, u32, u32, u32)>, u32, u32),
    /// `const` (true) or `volatile` (false) of the type (none: `void`).
    Qual(bool, Option<usize>),
    /// A function type: what it returns, what it takes, and whether `...`
    /// follows.
    Func(Option<usize>, Vec<Option<usize>>, bool),
    /// A typedef name: what it stands for, and its `.file` number and
    /// position.
    Typedef(String, Option<usize>, u32, Pos),
    /// An enum: its tag, its integer type, its constants and its `.file`
    /// number and line.
    Enum(Option<String>, usize, Vec<(String, i128)>, u32, Pos),
}

/// What makes two types the same, to describe each once. Records, typedefs
/// and enums are themselves (their address), the rest their parts.
#[derive(Clone, PartialEq, Eq, Hash)]
enum TypeKey {
    Base(IntKind),
    ArraySize,
    Ptr(Option<usize>),
    Array(usize, Vec<Option<u32>>),
    Record(usize),
    Qual(bool, Option<usize>),
    Func(Option<usize>, Vec<Option<usize>>, bool),
    Spelling(usize),
}

/// A global variable the file defines.
struct Global {
    var: Var,
    external: bool,
    /// What its initializer did: the string literals it holds and the
    /// symbols it refers to.
    events: Vec<Event>,
}

/// A string literal, which clang describes as a variable without a name: its
/// label, its type (`char [N]`, its NUL counted) and where it is.
struct Literal {
    label: String,
    ty: Option<usize>,
    file: u32,
    line: u32,
}

/// Something the code of a function or the initializer of a global does that
/// clang's front end does too as it emits them, in the order it does it, for
/// [`order`] to work out the order of the unit's entries as clang's.
#[derive(Clone, Debug)]
pub(crate) enum Event {
    /// A local variable is described: its type, and the `.file` number
    /// of its declaration.
    Local(Option<usize>, u32),
    /// A `static` local, at its symbol, of its type.
    Static(String, Option<usize>),
    /// An explicit cast to this type, which clang keeps in the unit
    /// (`DIBuilder::retainType`).
    Cast(Option<usize>),
    /// String literal `i` of [`Lines`]'s.
    Literal(usize),
    /// An enumeration constant of this enum, which clang describes as its
    /// code uses the constant.
    Enum(usize),
}

/// The debug information of one translation unit as it is generated.
pub(crate) struct Lines {
    info: DebugInfo,
    /// The preprocessor's file names, by [`Pos::file`].
    names: Vec<String>,
    /// [`Pos::file`] → `.file` number, for the files given one so far.
    numbers: HashMap<u32, u32>,
    /// A name already numbered (a header read twice is one file).
    by_name: HashMap<String, u32>,
    /// The names of the files numbered so far, file `n` at `n - 1`.
    file_names: Vec<String>,
    /// The final `.file` numbers of the numbers given as the code was
    /// generated ([`Lines::renumber`]).
    renumbered: HashMap<u32, u32>,
    /// The `.file` directives of the files only the entries LLVM makes as
    /// the module ends name, written with the debug sections.
    late_files: Vec<String>,
    /// The order of the entries ([`order`]), worked out by
    /// [`Lines::renumber`], which needs it to number the files: with the
    /// anonymous types records describe and the types functions define.
    order: Option<(order::Order, Nested)>,
    /// How many blocks have labels.
    blocks: u32,
    functions: Vec<Function>,
    globals: Vec<Global>,
    /// The string literals, in the order they come.
    literals: Vec<Literal>,
    /// The functions and the globals, in the order clang's front end emits
    /// them, which code generation follows: indices in `functions` (true) or
    /// `globals` (false).
    items: Vec<(bool, usize)>,
    /// The functions in the order of their code, which LLVM describes them
    /// in (indices in `functions`).
    code: Vec<usize>,
    /// The events of the function or global being generated.
    events: Vec<Event>,
    types: Vec<TypeNode>,
    type_ids: HashMap<TypeKey, usize>,
    /// The records, typedefs and enums in the table, kept alive so that
    /// their addresses stay theirs.
    keep: Vec<Arc<dyn std::any::Any>>,
    /// The target, whose addresses are 4 or 8 bytes.
    xlen: Xlen,
}

/// The anonymous types records describe: each one's record and member
/// ([`Lines::nested_types`]).
type Nested = Vec<(usize, (usize, usize))>;

/// A `.file` or `.loc` directive `line` of file `from`, of file `to`.
fn renumbered_line(line: &str, from: u32, to: u32) -> String {
    let (head, tail) = line.split_at(line.find(['.']).expect("a directive"));
    let (name, rest) = tail.split_once(' ').expect("a directive's number");
    let rest = rest
        .strip_prefix(&from.to_string())
        .expect("the directive's number");
    format!("{head}{name} {to}{rest}")
}

/// `s` as an assembly string literal.
fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Lines {
    pub fn new(info: &DebugInfo, names: &[String], xlen: Xlen) -> Self {
        Lines {
            info: info.clone(),
            names: names.to_vec(),
            numbers: HashMap::new(),
            by_name: HashMap::new(),
            file_names: Vec::new(),
            renumbered: HashMap::new(),
            late_files: Vec::new(),
            order: None,
            blocks: 0,
            functions: Vec::new(),
            globals: Vec::new(),
            literals: Vec::new(),
            items: Vec::new(),
            code: Vec::new(),
            events: Vec::new(),
            types: Vec::new(),
            type_ids: HashMap::new(),
            keep: Vec::new(),
            xlen,
        }
    }

    /// The file's name with `/` separators.
    fn name(&self, file: u32) -> String {
        self.names
            .get(file as usize)
            .map_or_else(|| "<unknown>".to_string(), |n| n.replace('\\', "/"))
    }

    /// `.file 0`, the first line of the output until [`Lines::renumber`]
    /// puts it where clang has it.
    pub fn file0(&self) -> String {
        format!(
            ".file 0 {} {}",
            quoted(&self.info.comp_dir),
            quoted(&self.name(0))
        )
    }

    /// The `.file` number of `file` as the code is generated, given the
    /// first time it is asked for: [`Lines::renumber`] gives the final ones.
    fn number(&mut self, file: u32) -> u32 {
        if file == 0 {
            return 0;
        }
        if let Some(&n) = self.numbers.get(&file) {
            return n;
        }
        let name = self.name(file);
        if let Some(&n) = self.by_name.get(&name) {
            self.numbers.insert(file, n);
            return n;
        }
        let n = self.by_name.len() as u32 + 1;
        self.by_name.insert(name.clone(), n);
        self.file_names.push(name);
        self.numbers.insert(file, n);
        n
    }

    /// The `.file` number of a declaration's file.
    fn decl_file(&mut self, pos: Pos) -> u32 {
        self.number(pos.file)
    }

    /// The `.file` directive of file `n` (as the code was generated) under
    /// its final number `to`.
    fn file_directive(&self, n: u32, to: u32) -> String {
        let name = &self.file_names[n as usize - 1];
        format!(".file {to} {} {}", quoted(&self.info.comp_dir), quoted(name))
    }

    /// Numbers the files as LLVM does, and returns the output `text` with
    /// its `.loc` directives renumbered and the `.file` directives where
    /// clang writes them. Call it once the code is generated and its order
    /// known ([`Lines::code_order`]).
    ///
    /// LLVM gives a file its number the first time it needs one
    /// (`DwarfUnit::getOrCreateSourceID`, for a `.loc` or an entry's
    /// `DW_AT_decl_file`), and so in three steps:
    ///
    /// 1. as the module begins, the files of the enums and the types of
    ///    casts it describes then (phase 1 of [`order`]), a type's after
    ///    those of the types it refers to, as LLVM gives a type's entry its
    ///    file once it has made what it refers to: their `.file`s come right
    ///    after `.file 0`;
    /// 2. then each file the code comes from, at its first `.loc` in the
    ///    output, which is in the order of the module's symbols, not the
    ///    order the code was generated in (see "Order" in
    ///    [`crate::codegen`]): its `.file` right before that `.loc`;
    /// 3. as the module ends, the files of the other entries, in the order
    ///    LLVM makes them (phases 3 to 6 of [`order`]): their `.file`s go
    ///    with the debug sections.
    ///
    /// LLVM makes the unit as the module begins if it describes something
    /// then or has globals (string literals among them): `.file 0` is then
    /// the first line of the output. Otherwise it makes the unit for the
    /// first function's code, and `.file 0` comes right before the first
    /// `.loc`.
    pub fn renumber(&mut self, text: &str) -> String {
        // Arrays count their elements in the type clang names
        // __ARRAY_SIZE_TYPE__.
        if self.types.iter().any(|t| matches!(t, TypeNode::Array(..))) {
            self.intern(TypeKey::ArraySize, || TypeNode::ArraySize);
        }
        let nested = self.nested_types();
        let local: Vec<(usize, usize)> = self
            .local_types()
            .into_iter()
            .filter(|(t, _)| !nested.iter().any(|(n, _)| n == t))
            .collect();
        let order = self.order(&nested, &local);

        // The final numbers, from 1 as LLVM needs them.
        let mut map: HashMap<u32, u32> = HashMap::from([(0, 0)]);
        let give = |map: &mut HashMap<u32, u32>, n: u32| -> Option<u32> {
            if map.contains_key(&n) {
                return None;
            }
            let next = map.len() as u32;
            map.insert(n, next);
            Some(next)
        };
        // 1. The files of the entries made as the module begins.
        let mut first = String::new();
        for &n in &order.begin_files {
            if let Some(to) = give(&mut map, n) {
                let _ = writeln!(first, "    {}", self.file_directive(n, to));
            }
        }
        let unit_first = order.begins || !self.globals.is_empty() || !self.literals.is_empty();
        // 2. The files of the code, as the output needs them. The only
        // `.file` in `text` is `.file 0`.
        let directive = |l: &str, name: &str| -> Option<u32> {
            let rest = l.trim_start().strip_prefix(name)?;
            rest.split_whitespace().next()?.parse().ok()
        };
        let mut file0 = None;
        let mut out = String::with_capacity(text.len());
        for l in text.split_inclusive('\n') {
            if directive(l, ".file ").is_some() {
                if unit_first {
                    out.push_str(l);
                    out.push_str(&first);
                } else {
                    file0 = Some(l);
                }
                continue;
            }
            let Some(n) = directive(l, ".loc ") else {
                out.push_str(l);
                continue;
            };
            if let Some(f) = file0.take() {
                out.push_str(f);
            }
            if let Some(to) = give(&mut map, n) {
                let _ = writeln!(out, "    {}", self.file_directive(n, to));
            }
            out.push_str(&renumbered_line(l, n, map[&n]));
        }
        if let Some(f) = file0 {
            out.insert_str(0, f);
        }
        // 3. The files of the entries made as the module ends.
        for &n in &order.end_files {
            if let Some(to) = give(&mut map, n) {
                let d = self.file_directive(n, to);
                self.late_files.push(d);
            }
        }
        self.renumbered = map;
        self.order = Some((order, nested));
        out
    }

    /// The final number of file `n`.
    fn final_file(&self, n: u32) -> u32 {
        self.renumbered.get(&n).copied().unwrap_or(n)
    }

    /// A `DW_AT_decl_file` of file `n`.
    fn file_val(&self, n: u32) -> Val {
        Val::Const(self.final_file(n).into())
    }

    /// The labels at the start and the end of a new block's code.
    pub fn block_labels(&mut self) -> (String, String) {
        self.blocks += 1;
        let n = self.blocks;
        (format!(".Ldbg.block.{n}"), format!(".Ldbg.block_end.{n}"))
    }

    /// The `.loc` that says the next instruction comes from `pos` (its
    /// file's `.file` comes with [`Lines::renumber`]). `column` overrides
    /// the position's (0 for a function's first line).
    pub fn loc(&mut self, pos: Pos, column: Option<u32>, prologue_end: bool) -> String {
        let n = self.number(pos.file);
        let flags = if prologue_end { " prologue_end" } else { "" };
        let col = column.unwrap_or(pos.col);
        format!(".loc {n} {} {col}{flags}", pos.line)
    }

    // ---- types ----

    /// The index in the type table of `ty`, as `spelled` spelled its base
    /// (none for `void`). The spelled type is found again inside `ty`: where
    /// what is left of `ty` is the spelled type, the typedef or the enum
    /// stands for it (`Node *p` is a pointer to the typedef `Node`).
    fn type_of(&mut self, ty: &Type, spelled: &Spelled) -> Option<usize> {
        if let Some(s) = &spelled.0 {
            if ty == s.ty() {
                return Some(self.spelling(s));
            }
            // A typedef of an array, qualified (`const row`): merlin's type
            // has the qualifiers on the elements, clang's describes the
            // typedef qualified, `const` of `row`.
            if let Some(q) = s.ty().is_array().then(|| ty.quals_added_to(s.ty())).flatten() {
                let id = Some(self.spelling(s));
                return self.qualify(id, q);
            }
        }
        match ty {
            Type::Void => None,
            Type::Int(k) => Some(self.intern(TypeKey::Base(*k), || TypeNode::Base(*k))),
            Type::Ptr(to) => {
                let to = self.type_of(to, spelled);
                Some(self.intern(TypeKey::Ptr(to), || TypeNode::Ptr(to)))
            }
            Type::Array(..) => {
                // `int m[2][3]` is one array type with two subranges, as clang
                // describes it, unless an inner array is a typedef's.
                let mut dims = Vec::new();
                let mut t = ty;
                while let Type::Array(elem, n) = t {
                    // From an array typedef on, qualified or not.
                    if spelled
                        .0
                        .as_ref()
                        .is_some_and(|s| s.ty().is_array() && t.quals_added_to(s.ty()).is_some())
                    {
                        break;
                    }
                    dims.push(*n);
                    t = elem;
                }
                let elem = self.type_of(t, spelled)?;
                let key = TypeKey::Array(elem, dims.clone());
                Some(self.intern(key, || TypeNode::Array(elem, dims)))
            }
            Type::Record(r) => Some(self.record(r)),
            Type::Qual(t, q) => {
                let id = self.type_of(t, spelled);
                self.qualify(id, *q)
            }
            Type::Func(f) => {
                let ret = self.type_of(&f.ret, spelled);
                // Each parameter as its declaration has it (`int
                // (*read)(const pagetable_t)`), as clang describes them.
                let none = Spelled::default();
                let params: Vec<Option<usize>> = f
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| match f.spelled.0.get(i) {
                        Some((declared, spelled)) => self.type_of(declared, spelled),
                        None => self.type_of(p, &none),
                    })
                    .collect();
                let key = TypeKey::Func(ret, params.clone(), f.variadic);
                Some(self.intern(key, || TypeNode::Func(ret, params, f.variadic)))
            }
        }
    }

    /// Type `id` with qualifiers `q`: `const volatile T` is const of
    /// volatile of T, as clang has it.
    fn qualify(&mut self, mut id: Option<usize>, q: Quals) -> Option<usize> {
        if q.volatile {
            id = Some(self.intern(TypeKey::Qual(false, id), || TypeNode::Qual(false, id)));
        }
        if q.konst {
            id = Some(self.intern(TypeKey::Qual(true, id), || TypeNode::Qual(true, id)));
        }
        id
    }

    /// The index of a type, added to the table if it is new.
    fn intern(&mut self, key: TypeKey, node: impl FnOnce() -> TypeNode) -> usize {
        if let Some(&id) = self.type_ids.get(&key) {
            return id;
        }
        self.types.push(node());
        self.type_ids.insert(key, self.types.len() - 1);
        self.types.len() - 1
    }

    /// A struct or union: in the table before its members are, so that a
    /// member that points back to it finds it.
    fn record(&mut self, r: &Arc<Record>) -> usize {
        let key = TypeKey::Record(Arc::as_ptr(r) as usize);
        if let Some(&id) = self.type_ids.get(&key) {
            return id;
        }
        // Where it is defined, as clang has it, which may come after where
        // it was first declared (`struct s;` in a header).
        let pos = r.body().map_or(r.pos, |b| b.pos);
        let file = self.decl_file(pos);
        let id = self.intern(key, || TypeNode::Record(r.clone(), Vec::new(), file, pos.line));
        self.keep.push(r.clone());
        let mut members = Vec::new();
        if let Some(body) = r.body() {
            for m in &body.members {
                let ty = self.type_of(&m.ty, &m.spelled);
                let file = self.decl_file(m.pos);
                members.push((m.name.clone(), ty, m.offset, file, m.pos.line));
            }
        }
        if let TypeNode::Record(_, ms, _, _) = &mut self.types[id] {
            *ms = members;
        }
        id
    }

    /// A typedef name or an enum.
    fn spelling(&mut self, s: &Spelling) -> usize {
        let key = match s {
            Spelling::Typedef(t) => TypeKey::Spelling(Arc::as_ptr(t) as *const u8 as usize),
            Spelling::Enum(e) => TypeKey::Spelling(Arc::as_ptr(e) as *const u8 as usize),
        };
        if let Some(&id) = self.type_ids.get(&key) {
            return id;
        }
        match s {
            Spelling::Typedef(t) => {
                self.keep.push(t.clone());
                let of = self.type_of(&t.ty, &t.spelled);
                let file = self.decl_file(t.pos);
                let node = TypeNode::Typedef(t.name.clone(), of, file, t.pos);
                self.intern(key, || node)
            }
            Spelling::Enum(e) => {
                self.keep.push(e.clone());
                let k = e.ty.int_kind().expect("an enum is an integer type");
                let of = self.intern(TypeKey::Base(k), || TypeNode::Base(k));
                let file = self.decl_file(e.pos);
                let node = TypeNode::Enum(e.tag.clone(), of, e.constants.clone(), file, e.pos);
                self.intern(key, || node)
            }
        }
    }

    // ---- variables and functions ----

    /// A parameter or a local variable named `name` of type `ty` (spelled
    /// `spelled`), declared at `pos`, living at `location`.
    pub fn var(&mut self, name: &str, ty: &Type, spelled: &Spelled, pos: Pos, location: Location) -> Var {
        let ty = self.type_of(ty, spelled);
        let file = self.decl_file(pos);
        Var {
            name: name.to_string(),
            file,
            line: pos.line,
            ty,
            location,
        }
    }

    /// A global variable the file defines, at symbol `sym`; the events
    /// since the last function or global are its initializer's.
    pub fn global(&mut self, name: &str, sym: &str, ty: (&Type, &Spelled), pos: Pos, external: bool) {
        let var = self.var(name, ty.0, ty.1, pos, Location::Symbol(sym.to_string()));
        let events = std::mem::take(&mut self.events);
        self.items.push((false, self.globals.len()));
        self.globals.push(Global {
            var,
            external,
            events,
        });
    }

    /// A variable declared in the function being generated, as clang's front
    /// end describes it there ([`Event::Local`], [`Event::Static`]).
    pub fn declared(&mut self, v: &Var) {
        let e = match &v.location {
            Location::Frame(_) => Event::Local(v.ty, v.file),
            Location::Symbol(sym) => Event::Static(sym.clone(), v.ty),
        };
        self.events.push(e);
    }

    /// An explicit cast to `ty` (spelled `spelled`) in the code generated.
    pub fn cast(&mut self, ty: &Type, spelled: &Spelled) {
        let t = self.type_of(ty, spelled);
        // A cast to void has no type to keep.
        if t.is_some() {
            self.events.push(Event::Cast(t));
        }
    }

    /// An enumeration constant of the enum `of` spells, in the code
    /// generated.
    pub fn enumerator(&mut self, of: &Spelled) {
        if let Some(s) = &of.0 {
            let t = self.spelling(s);
            self.events.push(Event::Enum(t));
        }
    }

    /// `goto` label `name`, at `pos`, whose code starts at label `sym`.
    pub fn label(&mut self, name: &str, pos: Pos, sym: &str) -> Label {
        let file = self.decl_file(pos);
        Label {
            name: name.to_string(),
            file,
            line: pos.line,
            col: pos.col,
            sym: sym.to_string(),
        }
    }

    /// String literal `label`, a `char` array of `len` bytes (its NUL
    /// included, if it has one), at `pos`.
    pub fn literal(&mut self, label: &str, len: usize, pos: Pos) {
        let array = Type::Array(Box::new(Type::Int(IntKind::Char)), Some(len as u32));
        let ty = self.type_of(&array, &Spelled::default());
        let file = self.decl_file(pos);
        self.events.push(Event::Literal(self.literals.len()));
        self.literals.push(Literal {
            label: label.to_string(),
            ty,
            file,
            line: pos.line,
        });
    }

    /// The functions, by name, in the order of their code.
    pub fn code_order(&mut self, names: &[&str]) {
        self.code = names
            .iter()
            .filter_map(|n| self.functions.iter().position(|f| f.name == *n))
            .collect();
    }

    /// Records function `name`, whose code ends at label `end`, for its
    /// `DW_TAG_subprogram`: whether it is external and declared with a
    /// prototype, what it returns, its parameters and its body's variables and
    /// blocks.
    #[allow(clippy::too_many_arguments)]
    pub fn function(
        &mut self,
        name: &str,
        end: &str,
        (pos, body_end): (Pos, Pos),
        (external, prototyped): (bool, bool),
        ret: (&Type, &Spelled),
        params: Vec<Var>,
        variadic: bool,
        body: Scope,
    ) {
        let file = self.number(pos.file);
        let ret = self.type_of(ret.0, ret.1);
        let events = std::mem::take(&mut self.events);
        self.items.push((true, self.functions.len()));
        self.functions.push(Function {
            name: name.to_string(),
            end: end.to_string(),
            file,
            line: pos.line,
            span: (pos, body_end),
            external,
            prototyped,
            ret,
            events,
            params,
            variadic,
            body,
        });
    }

    // ---- the entries ----

    /// The entry of a parameter or variable.
    fn var_die(&self, tag: u8, v: &Var) -> Die {
        let location = match &v.location {
            Location::Frame(off) => {
                let mut b = vec![OP_FBREG];
                b.extend(sleb(i64::from(*off)));
                vec![Expr::Bytes(b)]
            }
            Location::Symbol(sym) => vec![Expr::Addr(sym.clone())],
        };
        let mut d = Die::new(tag, &v.name);
        d.attr(AT_LOCATION, Val::Expr(location));
        d.attr(AT_NAME, Val::Str(v.name.clone()));
        d.attr(AT_DECL_FILE, self.file_val(v.file));
        d.attr(AT_DECL_LINE, Val::Const(v.line.into()));
        if let Some(t) = v.ty {
            d.attr(AT_TYPE, Val::Ref(t));
        }
        d
    }

    /// The entry of a `static` local: a global's attributes, in the order clang
    /// writes them (name, type, file, line, location).
    fn static_die(&self, v: &Var) -> Die {
        self.global_die(&Global {
            var: v.clone(),
            external: false,
            events: Vec::new(),
        })
    }

    /// The entries of a scope's variables in the frame, its labels and its
    /// blocks.
    fn scope_children(&self, s: &Scope, out: &mut Vec<Die>) {
        for v in &s.vars {
            out.push(self.var_die(TAG_VARIABLE, v));
        }
        for l in &s.labels {
            let mut d = Die::new(TAG_LABEL, &l.name);
            d.attr(AT_NAME, Val::Str(l.name.clone()));
            d.attr(AT_DECL_FILE, self.file_val(l.file));
            d.attr(AT_DECL_LINE, Val::Const(l.line.into()));
            d.attr(AT_DECL_COLUMN, Val::Const(l.col.into()));
            d.attr(AT_LOW_PC, Val::Addr(l.sym.clone()));
            out.push(d);
        }
        for b in &s.blocks {
            if b.transparent {
                self.scope_children(b, out);
                continue;
            }
            let mut d = Die::new(TAG_LEXICAL_BLOCK, "a block");
            d.attr(AT_LOW_PC, Val::Addr(b.begin.clone()));
            d.attr(AT_HIGH_PC, Val::Len(b.end.clone(), b.begin.clone()));
            self.scope_children(b, &mut d.children);
            out.push(d);
        }
    }

    /// The entry of function `i`, with its static locals and the types it
    /// defines in the order `local` has them, after the types it defines
    /// that LLVM makes as the module begins (`first`).
    fn function_die(
        &self,
        i: usize,
        first: &[usize],
        local: &[order::Local],
        nested: &[(usize, (usize, usize))],
    ) -> Die {
        let f = &self.functions[i];
        let mut d = Die::new(TAG_SUBPROGRAM, &f.name);
        d.attr(AT_LOW_PC, Val::Addr(f.name.clone()));
        d.attr(AT_HIGH_PC, Val::Len(f.end.clone(), f.name.clone()));
        d.attr(AT_FRAME_BASE, Val::Expr(vec![Expr::Bytes(vec![OP_REG8])]));
        d.attr(AT_NAME, Val::Str(f.name.clone()));
        d.attr(AT_DECL_FILE, self.file_val(f.file));
        d.attr(AT_DECL_LINE, Val::Const(f.line.into()));
        // `int f()` has no prototype; clang says so by leaving this out.
        if f.prototyped {
            d.attr(AT_PROTOTYPED, Val::Flag);
        }
        if let Some(t) = f.ret {
            d.attr(AT_TYPE, Val::Ref(t));
        }
        if f.external {
            d.attr(AT_EXTERNAL, Val::Flag);
        }
        for &t in first {
            d.children.push(self.type_die(t, nested));
        }
        for p in &f.params {
            d.children.push(self.var_die(TAG_FORMAL_PARAMETER, p));
        }
        // The variables in the frame, the labels and the blocks, then `...`,
        // then every static local of the function and the types it defines,
        // whatever block declares them, as clang has them.
        self.scope_children(&f.body, &mut d.children);
        if f.variadic {
            d.children
                .push(Die::new(TAG_UNSPECIFIED_PARAMETERS, "the variadic arguments"));
        }
        for l in local {
            match l {
                order::Local::Type(t) => d.children.push(self.type_die(*t, nested)),
                order::Local::Static(sym) => {
                    let v = f
                        .body
                        .statics
                        .iter()
                        .find(|v| matches!(&v.location, Location::Symbol(s) if s == sym));
                    d.children.extend(v.map(|v| self.static_die(v)));
                }
            }
        }
        d
    }

    /// The entry of a string literal: a variable without a name, as clang
    /// describes one.
    fn literal_die(&self, l: &Literal) -> Die {
        let mut d = Die::new(TAG_VARIABLE, &format!("the string literal {}", l.label));
        if let Some(t) = l.ty {
            d.attr(AT_TYPE, Val::Ref(t));
        }
        d.attr(AT_DECL_FILE, self.file_val(l.file));
        d.attr(AT_DECL_LINE, Val::Const(l.line.into()));
        d.attr(AT_LOCATION, Val::Expr(vec![Expr::Addr(l.label.clone())]));
        d
    }

    /// The structs, unions, enums and typedefs defined inside a function's
    /// body (where they are declared lies inside its code's), and the
    /// function: clang describes them in its entry.
    fn local_types(&self) -> Vec<(usize, usize)> {
        let inside = |p: Pos, (start, end): (Pos, Pos)| {
            p.file == start.file
                && (start.line, start.col) <= (p.line, p.col)
                && (p.line, p.col) <= (end.line, end.col)
        };
        let mut local = Vec::new();
        for (id, t) in self.types.iter().enumerate() {
            let pos = match t {
                TypeNode::Record(r, ..) => r.body().map_or(r.pos, |b| b.pos),
                TypeNode::Typedef(_, _, _, pos) | TypeNode::Enum(_, _, _, _, pos) => *pos,
                _ => continue,
            };
            if let Some(f) = self.functions.iter().position(|f| inside(pos, f.span)) {
                local.push((id, f));
            }
        }
        local
    }

    fn global_die(&self, g: &Global) -> Die {
        let v = &g.var;
        let mut d = Die::new(TAG_VARIABLE, &v.name);
        d.attr(AT_NAME, Val::Str(v.name.clone()));
        if let Some(t) = v.ty {
            d.attr(AT_TYPE, Val::Ref(t));
        }
        if g.external {
            d.attr(AT_EXTERNAL, Val::Flag);
        }
        d.attr(AT_DECL_FILE, self.file_val(v.file));
        d.attr(AT_DECL_LINE, Val::Const(v.line.into()));
        let Location::Symbol(sym) = &v.location else {
            unreachable!("a global is at its symbol")
        };
        d.attr(AT_LOCATION, Val::Expr(vec![Expr::Addr(sym.clone())]));
        d
    }

    /// The anonymous structs, unions and enums that a record's member is of,
    /// or points to or is an array of: each one's record and member (the
    /// first that reaches it), which clang describes it in, after the member.
    /// A C type with no name can only be used where it is defined.
    fn nested_types(&self) -> Nested {
        let mut nested: Vec<(usize, (usize, usize))> = Vec::new();
        for (id, t) in self.types.iter().enumerate() {
            let TypeNode::Record(_, members, ..) = t else {
                continue;
            };
            for (i, (_, ty, ..)) in members.iter().enumerate() {
                let mut t = *ty;
                // Through pointers, arrays and qualifiers (typedefs have names).
                while let Some(b) = t {
                    t = match &self.types[b] {
                        TypeNode::Ptr(to) | TypeNode::Qual(_, to) => *to,
                        TypeNode::Array(elem, _) => Some(*elem),
                        _ => break,
                    };
                }
                let Some(b) = t else { continue };
                let anonymous = match &self.types[b] {
                    TypeNode::Record(r, ..) => r.tag.is_none(),
                    TypeNode::Enum(tag, ..) => tag.is_none(),
                    _ => false,
                };
                if anonymous && b != id && !nested.iter().any(|(n, _)| *n == b) {
                    nested.push((b, (id, i)));
                }
            }
        }
        nested
    }

    /// The entry of type `id`, with the anonymous types `nested` puts in it.
    fn type_die(&self, id: usize, nested: &[(usize, (usize, usize))]) -> Die {
        let typed = |d: &mut Die, t: Option<usize>| {
            if let Some(t) = t {
                d.attr(AT_TYPE, Val::Ref(t));
            }
        };
        let mut d = match &self.types[id] {
            TypeNode::Base(k) => {
                let mut d = Die::new(TAG_BASE_TYPE, k.name());
                let encoding = match k {
                    IntKind::Bool => ATE_BOOLEAN,
                    // Plain char is unsigned on RISC-V.
                    IntKind::Char | IntKind::UChar => ATE_UNSIGNED_CHAR,
                    IntKind::SChar => ATE_SIGNED_CHAR,
                    k if k.signed() => ATE_SIGNED,
                    _ => ATE_UNSIGNED,
                };
                d.attr(AT_NAME, Val::Str(k.name().to_string()));
                d.attr(AT_ENCODING, Val::Data1(encoding));
                d.attr(AT_BYTE_SIZE, Val::Const(k.size(self.xlen).into()));
                d
            }
            TypeNode::ArraySize => {
                let mut d = Die::new(TAG_BASE_TYPE, "__ARRAY_SIZE_TYPE__");
                d.attr(AT_NAME, Val::Str("__ARRAY_SIZE_TYPE__".into()));
                d.attr(AT_BYTE_SIZE, Val::Const(8));
                d.attr(AT_ENCODING, Val::Data1(ATE_UNSIGNED));
                d
            }
            TypeNode::Ptr(to) => {
                let mut d = Die::new(TAG_POINTER_TYPE, "a pointer");
                typed(&mut d, *to);
                d
            }
            TypeNode::Array(elem, dims) => {
                let mut d = Die::new(TAG_ARRAY_TYPE, "an array");
                d.attr(AT_TYPE, Val::Ref(*elem));
                let size_type = self.type_ids[&TypeKey::ArraySize];
                for n in dims {
                    let mut s = Die::new(TAG_SUBRANGE_TYPE, "a dimension");
                    s.attr(AT_TYPE, Val::Ref(size_type));
                    if let Some(n) = n {
                        s.attr(AT_COUNT, Val::Const((*n).into()));
                    }
                    d.children.push(s);
                }
                d
            }
            TypeNode::Record(r, members, file, line) => {
                let tag = if r.union {
                    TAG_UNION_TYPE
                } else {
                    TAG_STRUCTURE_TYPE
                };
                let mut d = Die::new(tag, &r.name());
                if let Some(t) = &r.tag {
                    d.attr(AT_NAME, Val::Str(t.clone()));
                }
                // One only declared has no file and line (clang's
                // `isForwardDecl`).
                match r.body() {
                    Some(body) => {
                        d.attr(AT_BYTE_SIZE, Val::Const(body.size.into()));
                        d.attr(AT_DECL_FILE, self.file_val(*file));
                        d.attr(AT_DECL_LINE, Val::Const((*line).into()));
                    }
                    None => d.attr(AT_DECLARATION, Val::Flag),
                }
                for (i, (name, ty, offset, file, line)) in members.iter().enumerate() {
                    let mut m = Die::new(TAG_MEMBER, name);
                    m.attr(AT_NAME, Val::Str(name.clone()));
                    typed(&mut m, *ty);
                    m.attr(AT_DECL_FILE, self.file_val(*file));
                    m.attr(AT_DECL_LINE, Val::Const((*line).into()));
                    // A union's members too, at 0, as clang writes them.
                    m.attr(AT_DATA_MEMBER_LOCATION, Val::Const((*offset).into()));
                    d.children.push(m);
                    // The anonymous types this member's type is made of, defined
                    // here: inside the record, after the member, as clang has
                    // them.
                    for &(t, _) in nested.iter().filter(|(_, at)| *at == (id, i)) {
                        d.children.push(self.type_die(t, nested));
                    }
                }
                d
            }
            TypeNode::Qual(konst, of) => {
                let (tag, what) = if *konst {
                    (TAG_CONST_TYPE, "const")
                } else {
                    (TAG_VOLATILE_TYPE, "volatile")
                };
                let mut d = Die::new(tag, what);
                typed(&mut d, *of);
                d
            }
            TypeNode::Func(ret, params, variadic) => {
                let mut d = Die::new(TAG_SUBROUTINE_TYPE, "a function type");
                // The return type first, then the flag, as LLVM's
                // `constructTypeDIE` adds them: the abbreviation lists the
                // attributes in this order.
                typed(&mut d, *ret);
                d.attr(AT_PROTOTYPED, Val::Flag);
                for p in params {
                    let mut pd = Die::new(TAG_FORMAL_PARAMETER, "a parameter");
                    typed(&mut pd, *p);
                    d.children.push(pd);
                }
                if *variadic {
                    d.children
                        .push(Die::new(TAG_UNSPECIFIED_PARAMETERS, "the variadic arguments"));
                }
                d
            }
            TypeNode::Typedef(name, of, file, pos) => {
                let mut d = Die::new(TAG_TYPEDEF, name);
                typed(&mut d, *of);
                d.attr(AT_NAME, Val::Str(name.clone()));
                // A builtin one (`__builtin_va_list`) is nowhere in the file.
                if pos.line != 0 {
                    d.attr(AT_DECL_FILE, self.file_val(*file));
                    d.attr(AT_DECL_LINE, Val::Const(pos.line.into()));
                }
                d
            }
            TypeNode::Enum(tag, of, constants, file, pos) => {
                let line = &pos.line;
                let what = format!("enum {}", tag.as_deref().unwrap_or("(anonymous)"));
                let mut d = Die::new(TAG_ENUMERATION_TYPE, &what);
                d.attr(AT_TYPE, Val::Ref(*of));
                if let Some(t) = tag {
                    d.attr(AT_NAME, Val::Str(t.clone()));
                }
                d.attr(AT_BYTE_SIZE, Val::Const(4));
                d.attr(AT_DECL_FILE, self.file_val(*file));
                d.attr(AT_DECL_LINE, Val::Const((*line).into()));
                let signed = matches!(self.types[*of], TypeNode::Base(k) if k.signed());
                for (name, value) in constants {
                    let mut e = Die::new(TAG_ENUMERATOR, name);
                    e.attr(AT_NAME, Val::Str(name.clone()));
                    // The constant's form says how to read it: signed for
                    // an enum of int, unsigned for one of unsigned int, as
                    // clang writes them.
                    let v = if signed {
                        Val::Sdata(*value as i64)
                    } else {
                        Val::Udata(*value as u64)
                    };
                    e.attr(AT_CONST_VALUE, v);
                    d.children.push(e);
                }
                d
            }
        };
        d.type_id = Some(id);
        d
    }

    /// The `.debug_abbrev` and `.debug_info` sections, and the label at the
    /// start of `.debug_line` that the compile unit points at. Nothing if the
    /// file has no functions (no code to describe).
    pub fn sections(&mut self) -> String {
        if self.functions.is_empty() {
            return String::new();
        }
        let (order, nested) = self.order.take().expect("the files are numbered first");
        let first = &self.functions[self.code[0]].name;
        let last_end = &self.functions[self.code[self.code.len() - 1]].end;

        // The attributes in clang's order.
        let mut cu = Die::new(TAG_COMPILE_UNIT, "the compile unit");
        cu.attr(AT_PRODUCER, Val::Str("merlin".into()));
        cu.attr(AT_LANGUAGE, Val::Data2(LANG_C99));
        cu.attr(AT_NAME, Val::Str(self.name(0)));
        cu.attr(AT_STR_OFFSETS_BASE, Val::SecOffset(".Lstr_offsets_base0".into()));
        cu.attr(AT_STMT_LIST, Val::SecOffset(".Lline_table_start0".into()));
        cu.attr(AT_COMP_DIR, Val::Str(self.info.comp_dir.clone()));
        cu.attr(AT_LOW_PC, Val::Addr(first.clone()));
        cu.attr(AT_HIGH_PC, Val::Len(last_end.clone(), first.clone()));
        cu.attr(AT_ADDR_BASE, Val::SecOffset(".Laddr_table_base0".into()));
        // The entries in clang's order (see [`order`]).
        for child in &order.unit {
            let die = match *child {
                order::Child::Function(i) => self.function_die(i, &order.first[i], &order.locals[i], &nested),
                order::Child::Global(i) => self.global_die(&self.globals[i]),
                order::Child::Literal(i) => self.literal_die(&self.literals[i]),
                order::Child::Type(t) => self.type_die(t, &nested),
            };
            cu.children.push(die);
        }

        // The strings and the addresses, each once, by their index in
        // `.debug_str_offsets` and `.debug_addr`, in the order they come.
        let (mut strings, mut addrs) = (Vec::new(), Vec::new());
        cu.pool(&mut strings, &mut addrs);

        // The abbreviations, one per shape of entry (tag, children, the
        // attributes' forms), numbered from 1 in the order they come.
        let mut abbrevs: Vec<Abbrev> = Vec::new();
        cu.number(&mut abbrevs);

        // Where each entry starts, from the unit's start: its header is 12
        // bytes (length, version, unit type, address size, abbreviations).
        let addr = ptr_size(self.xlen);
        let mut type_offsets = vec![0u32; self.types.len()];
        let end = cu.layout(12, &mut type_offsets);

        let mut s = String::new();
        for f in &self.late_files {
            let _ = writeln!(s, "    {f}");
        }
        let _ = writeln!(s, "\n    .section .debug_abbrev,\"\",@progbits");
        let _ = writeln!(s, ".Ldebug_abbrev0:");
        for (i, (tag, children, attrs)) in abbrevs.iter().enumerate() {
            let mut b = uleb(i as u64 + 1);
            b.push(*tag);
            b.push(u8::from(*children));
            let _ = writeln!(s, "    {}    # {}: {}", bytes(&b), i + 1, tag_name(*tag));
            for &(at, form) in attrs {
                let _ = writeln!(s, "    {}", bytes(&[at, form]));
            }
            let _ = writeln!(s, "    .byte 0, 0");
        }
        let _ = writeln!(s, "    .byte 0    # the end of the abbreviations");

        let _ = writeln!(s, "\n    .section .debug_info,\"\",@progbits");
        let _ = writeln!(s, "    .word {}    # unit length", end - 4);
        let _ = writeln!(s, "    .half 5    # DWARF version");
        let _ = writeln!(s, "    .byte {UT_COMPILE}    # a compile unit");
        let _ = writeln!(s, "    .byte {addr}    # address size");
        let _ = writeln!(s, "    .word .Ldebug_abbrev0");
        let addr_directive = match self.xlen {
            Xlen::Rv32 => ".word",
            Xlen::Rv64 => ".quad",
        };
        cu.emit(&mut s, &type_offsets, 0);

        // The strings: their offsets in `.debug_str` (a header of 8 bytes,
        // then a word each, as clang writes them), then the strings.
        let _ = writeln!(s, "\n    .section .debug_str_offsets,\"\",@progbits");
        let _ = writeln!(s, "    .word {}    # length", 4 + 4 * strings.len());
        let _ = writeln!(s, "    .half 5    # DWARF version");
        let _ = writeln!(s, "    .half 0    # padding");
        let _ = writeln!(s, ".Lstr_offsets_base0:");
        for i in 0..strings.len() {
            let _ = writeln!(s, "    .word .Linfo_string{i}");
        }
        let _ = writeln!(s, "\n    .section .debug_str,\"MS\",@progbits,1");
        for (i, t) in strings.iter().enumerate() {
            let _ = writeln!(s, ".Linfo_string{i}:");
            let _ = writeln!(s, "    .asciz {}", quoted(t));
        }
        // The addresses: a header of 8 bytes, then one each.
        let _ = writeln!(s, "\n    .section .debug_addr,\"\",@progbits");
        let _ = writeln!(s, "    .word {}    # length", 4 + addr as usize * addrs.len());
        let _ = writeln!(s, "    .half 5    # DWARF version");
        let _ = writeln!(s, "    .byte {addr}    # address size");
        let _ = writeln!(s, "    .byte 0    # segment selector size");
        let _ = writeln!(s, ".Laddr_table_base0:");
        for a in &addrs {
            let _ = writeln!(s, "    {addr_directive} {a}");
        }

        let _ = writeln!(s, "\n    .section .debug_line,\"\",@progbits");
        let _ = writeln!(s, ".Lline_table_start0:");
        s
    }
}

/// `.byte` of some bytes.
fn bytes(b: &[u8]) -> String {
    let list: Vec<String> = b.iter().map(|b| format!("{b:#04x}")).collect();
    format!(".byte {}", list.join(", "))
}

/// A tag's name, for the comments.
fn tag_name(tag: u8) -> &'static str {
    match tag {
        TAG_ARRAY_TYPE => "DW_TAG_array_type",
        TAG_ENUMERATION_TYPE => "DW_TAG_enumeration_type",
        TAG_FORMAL_PARAMETER => "DW_TAG_formal_parameter",
        TAG_LABEL => "DW_TAG_label",
        TAG_LEXICAL_BLOCK => "DW_TAG_lexical_block",
        TAG_MEMBER => "DW_TAG_member",
        TAG_POINTER_TYPE => "DW_TAG_pointer_type",
        TAG_COMPILE_UNIT => "DW_TAG_compile_unit",
        TAG_STRUCTURE_TYPE => "DW_TAG_structure_type",
        TAG_SUBROUTINE_TYPE => "DW_TAG_subroutine_type",
        TAG_TYPEDEF => "DW_TAG_typedef",
        TAG_UNION_TYPE => "DW_TAG_union_type",
        TAG_UNSPECIFIED_PARAMETERS => "DW_TAG_unspecified_parameters",
        TAG_SUBRANGE_TYPE => "DW_TAG_subrange_type",
        TAG_BASE_TYPE => "DW_TAG_base_type",
        TAG_CONST_TYPE => "DW_TAG_const_type",
        TAG_ENUMERATOR => "DW_TAG_enumerator",
        TAG_SUBPROGRAM => "DW_TAG_subprogram",
        TAG_VARIABLE => "DW_TAG_variable",
        TAG_VOLATILE_TYPE => "DW_TAG_volatile_type",
        _ => "?",
    }
}

/// Part of a location expression.
#[derive(Clone, Debug)]
enum Expr {
    Bytes(Vec<u8>),
    /// A symbol's address: `DW_OP_addrx` and its index in `.debug_addr`, once
    /// [`Die::pool`] has given it one (the bytes then).
    Addr(String),
}

/// An attribute's value, which decides its form. Constants take the smallest
/// `data` form that holds them, strings and addresses are indices (in
/// `.debug_str_offsets` and `.debug_addr`), as clang writes them for DWARF 5.
#[derive(Clone, Debug)]
enum Val {
    /// A string, `DW_FORM_strx1` to `strx4` once [`Die::pool`] has made it a
    /// [`Val::Strx`].
    Str(String),
    /// `DW_FORM_strx1`, `strx2`, `strx3` or `strx4`, as the index needs.
    Strx(u32),
    /// `DW_FORM_data1`, whatever the value (an encoding).
    Data1(u8),
    /// `DW_FORM_data2`, whatever the value (a language).
    Data2(u16),
    /// A constant: `DW_FORM_data1`, `data2`, `data4` or `data8`, the first
    /// that holds it (LLVM's `DIEInteger::BestForm`).
    Const(u64),
    Sdata(i64),
    Udata(u64),
    /// `DW_FORM_flag_present`: the attribute is there, and takes no bytes.
    Flag,
    /// A label's address, `DW_FORM_addrx` once [`Die::pool`] has made it a
    /// [`Val::Addrx`].
    Addr(String),
    /// `DW_FORM_addrx`: an index in `.debug_addr`.
    Addrx(u32),
    /// `DW_FORM_data4`: the distance between two labels (high pc).
    Len(String, String),
    /// `DW_FORM_ref4`: a type's entry, by its index in the table.
    Ref(usize),
    /// `DW_FORM_exprloc`.
    Expr(Vec<Expr>),
    /// `DW_FORM_sec_offset`: a label in another section.
    SecOffset(String),
}

/// How many bytes an index takes as `DW_FORM_strx1` to `strx4`, and the form.
fn strx(i: u32) -> (u32, u8) {
    match i {
        0..=0xff => (1, FORM_STRX1),
        0x100..=0xffff => (2, FORM_STRX2),
        0x1_0000..=0xff_ffff => (3, FORM_STRX3),
        _ => (4, FORM_STRX4),
    }
}

/// How many bytes a constant takes in the smallest `data` form, and the form.
fn data(v: u64) -> (u32, u8) {
    match v {
        0..=0xff => (1, FORM_DATA1),
        0x100..=0xffff => (2, FORM_DATA2),
        0x1_0000..=0xffff_ffff => (4, FORM_DATA4),
        _ => (8, FORM_DATA8),
    }
}

/// The index of `item` in `pool`, added at the end if it is new.
fn index_in(pool: &mut Vec<String>, item: &str) -> u32 {
    let i = match pool.iter().position(|p| p == item) {
        Some(i) => i,
        None => {
            pool.push(item.to_string());
            pool.len() - 1
        }
    };
    i as u32
}

impl Val {
    fn form(&self) -> u8 {
        match self {
            Val::Str(_) | Val::Addr(_) => unreachable!("pooled before the forms are asked"),
            Val::Strx(i) => strx(*i).1,
            Val::Data1(_) => FORM_DATA1,
            Val::Data2(_) => FORM_DATA2,
            Val::Const(v) => data(*v).1,
            Val::Len(..) => FORM_DATA4,
            Val::Sdata(_) => FORM_SDATA,
            Val::Udata(_) => FORM_UDATA,
            Val::Flag => FORM_FLAG_PRESENT,
            Val::Addrx(_) => FORM_ADDRX,
            Val::Ref(_) => FORM_REF4,
            Val::Expr(_) => FORM_EXPRLOC,
            Val::SecOffset(_) => FORM_SEC_OFFSET,
        }
    }

    /// The bytes of a (pooled) expression.
    fn expr_len(e: &[Expr]) -> u32 {
        e.iter()
            .map(|p| match p {
                Expr::Bytes(b) => b.len() as u32,
                Expr::Addr(_) => unreachable!("pooled before the layout"),
            })
            .sum()
    }

    fn size(&self) -> u32 {
        match self {
            Val::Str(_) | Val::Addr(_) => unreachable!("pooled before the layout"),
            Val::Strx(i) => strx(*i).0,
            Val::Data1(_) => 1,
            Val::Data2(_) => 2,
            Val::Const(v) => data(*v).0,
            Val::Len(..) | Val::Ref(_) | Val::SecOffset(_) => 4,
            Val::Sdata(v) => sleb(*v).len() as u32,
            Val::Udata(v) => uleb(*v).len() as u32,
            Val::Flag => 0,
            Val::Addrx(i) => uleb((*i).into()).len() as u32,
            Val::Expr(e) => {
                let n = Val::expr_len(e);
                uleb(n.into()).len() as u32 + n
            }
        }
    }
}

/// An abbreviation: a tag, whether children follow, and each attribute and
/// its form.
type Abbrev = (u8, bool, Vec<(u8, u8)>);

/// A debugging information entry as it is built.
struct Die {
    tag: u8,
    /// What the entry is, for the comment before it.
    what: String,
    attrs: Vec<(u8, Val)>,
    children: Vec<Die>,
    /// Its abbreviation's code, once numbered.
    code: u64,
    /// The type it describes, if it is one.
    type_id: Option<usize>,
}

impl Die {
    fn new(tag: u8, what: &str) -> Die {
        Die {
            tag,
            what: what.to_string(),
            attrs: Vec::new(),
            children: Vec::new(),
            code: 0,
            type_id: None,
        }
    }

    fn attr(&mut self, at: u8, v: Val) {
        self.attrs.push((at, v));
    }

    /// Turns the strings and addresses of this entry and those below it into
    /// indices in `strings` and `addrs`, adding those not seen yet: a string
    /// becomes a [`Val::Strx`], an address a [`Val::Addrx`], and a symbol's
    /// address in an expression `DW_OP_addrx` and its index.
    fn pool(&mut self, strings: &mut Vec<String>, addrs: &mut Vec<String>) {
        for (_, v) in &mut self.attrs {
            match v {
                Val::Str(s) => *v = Val::Strx(index_in(strings, s)),
                Val::Addr(l) => *v = Val::Addrx(index_in(addrs, l)),
                Val::Expr(e) => {
                    for p in e.iter_mut() {
                        if let Expr::Addr(sym) = p {
                            let mut b = vec![OP_ADDRX];
                            b.extend(uleb(index_in(addrs, sym).into()));
                            *p = Expr::Bytes(b);
                        }
                    }
                }
                _ => {}
            }
        }
        for c in &mut self.children {
            c.pool(strings, addrs);
        }
    }

    /// Gives this entry and those below it their abbreviations, adding the
    /// shapes not seen yet.
    fn number(&mut self, abbrevs: &mut Vec<Abbrev>) {
        let shape = (
            self.tag,
            !self.children.is_empty(),
            self.attrs.iter().map(|(a, v)| (*a, v.form())).collect::<Vec<_>>(),
        );
        self.code = match abbrevs.iter().position(|a| *a == shape) {
            Some(i) => i as u64 + 1,
            None => {
                abbrevs.push(shape);
                abbrevs.len() as u64
            }
        };
        for c in &mut self.children {
            c.number(abbrevs);
        }
    }

    /// Lays the entry out at `at` (from the unit's start), noting where each
    /// type's entry is; returns the offset just past it and its children.
    fn layout(&self, at: u32, type_offsets: &mut [u32]) -> u32 {
        if let Some(id) = self.type_id {
            type_offsets[id] = at;
        }
        let mut next = at + uleb(self.code).len() as u32;
        next += self.attrs.iter().map(|(_, v)| v.size()).sum::<u32>();
        if !self.children.is_empty() {
            for c in &self.children {
                next = c.layout(next, type_offsets);
            }
            next += 1; // the end of the children
        }
        next
    }

    /// Writes the entry and its children.
    fn emit(&self, s: &mut String, type_offsets: &[u32], depth: usize) {
        let line = |s: &mut String, text: String| {
            let _ = writeln!(s, "    {text}");
        };
        let _ = writeln!(
            s,
            "    # {}{}: {}",
            "  ".repeat(depth),
            tag_name(self.tag),
            self.what
        );
        line(s, bytes(&uleb(self.code)));
        for (_, v) in &self.attrs {
            match v {
                Val::Str(_) | Val::Addr(_) => unreachable!("pooled before the entries are written"),
                Val::Strx(i) => {
                    let b = i.to_le_bytes();
                    line(s, bytes(&b[..strx(*i).0 as usize]));
                }
                Val::Data1(b) => line(s, format!(".byte {b}")),
                Val::Data2(h) => line(s, format!(".half {h}")),
                Val::Const(v) => match data(*v).0 {
                    1 => line(s, format!(".byte {v}")),
                    2 => line(s, format!(".half {v}")),
                    4 => line(s, format!(".word {v}")),
                    _ => line(s, format!(".quad {v}")),
                },
                Val::Sdata(v) => line(s, bytes(&sleb(*v))),
                Val::Udata(v) => line(s, bytes(&uleb(*v))),
                Val::Addrx(i) => line(s, bytes(&uleb((*i).into()))),
                Val::Flag => {}
                Val::Len(end, start) => line(s, format!(".word {end} - {start}")),
                Val::Ref(t) => line(s, format!(".word {}", type_offsets[*t])),
                Val::SecOffset(l) => line(s, format!(".word {l}")),
                Val::Expr(e) => {
                    let n = Val::expr_len(e);
                    let what = if n == 1 { "byte" } else { "bytes" };
                    line(
                        s,
                        format!("{}    # an expression of {n} {what}", bytes(&uleb(n.into()))),
                    );
                    for p in e {
                        if let Expr::Bytes(b) = p {
                            line(s, bytes(b));
                        }
                    }
                }
            }
        }
        if !self.children.is_empty() {
            for c in &self.children {
                c.emit(s, type_offsets, depth + 1);
            }
            line(s, ".byte 0    # the end of the children".into());
        }
    }
}
