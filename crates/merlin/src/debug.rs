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
//!   declares a described type, in the order they come.
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
//! * a `DW_TAG_variable` per global variable the file defines, at its symbol;
//! * the types they use, each described once: base types (`int`, `unsigned
//!   char`, ...), pointers, arrays (one `DW_TAG_subrange_type` per dimension,
//!   as clang does), structs and unions with their members and offsets (an
//!   anonymous struct, union or enum a member is of inside the record),
//!   enums with their constants, typedefs, `const` and `volatile`, and
//!   function types (for function pointers).
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

use crate::types::{ptr_size, IntKind, Record, Spelled, Spelling, Type, Xlen};
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
    blocks: Vec<Scope>,
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
        if inner.vars.is_empty() {
            self.blocks.extend(inner.blocks);
        } else {
            self.blocks.push(inner);
        }
    }
}

/// A function, for its `DW_TAG_subprogram`.
struct Function {
    name: String,
    /// The label just past its code.
    end: String,
    /// Its `.file` number and line.
    file: u32,
    line: u32,
    external: bool,
    /// Declared with a prototype (not `()`).
    prototyped: bool,
    ret: Option<usize>,
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
    /// A typedef name: what it stands for, and its `.file` number and line.
    Typedef(String, Option<usize>, u32, u32),
    /// An enum: its tag, its integer type, its constants and its `.file`
    /// number and line.
    Enum(Option<String>, usize, Vec<(String, i128)>, u32, u32),
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
    /// `.file` directives for files numbered outside the code (a header
    /// that only declares a type), by number: written before the file's
    /// first `.loc` if it gets one, else with the debug sections.
    pending_files: Vec<(u32, String)>,
    /// How many blocks have labels.
    blocks: u32,
    functions: Vec<Function>,
    globals: Vec<Global>,
    types: Vec<TypeNode>,
    type_ids: HashMap<TypeKey, usize>,
    /// The records, typedefs and enums in the table, kept alive so that
    /// their addresses stay theirs.
    keep: Vec<Arc<dyn std::any::Any>>,
    /// The target, whose addresses are 4 or 8 bytes.
    xlen: Xlen,
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
            pending_files: Vec::new(),
            blocks: 0,
            functions: Vec::new(),
            globals: Vec::new(),
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

    /// `.file 0`, the first line of the output.
    pub fn file0(&self) -> String {
        format!(
            ".file 0 {} {}",
            quoted(&self.info.comp_dir),
            quoted(&self.name(0))
        )
    }

    /// The `.file` number of `file`, and the `.file` directive to write first
    /// if it has none yet.
    pub fn number(&mut self, file: u32) -> (u32, Option<String>) {
        if file == 0 {
            return (0, None);
        }
        if let Some(&n) = self.numbers.get(&file) {
            return (n, None);
        }
        let name = self.name(file);
        if let Some(&n) = self.by_name.get(&name) {
            self.numbers.insert(file, n);
            return (n, None);
        }
        let n = self.by_name.len() as u32 + 1;
        self.by_name.insert(name.clone(), n);
        self.numbers.insert(file, n);
        let directive = format!(".file {n} {} {}", quoted(&self.info.comp_dir), quoted(&name));
        (n, Some(directive))
    }

    /// The `.file` number of a declaration's file, outside the code: a new
    /// number's directive goes with the debug sections.
    fn decl_file(&mut self, pos: Pos) -> u32 {
        let (n, directive) = self.number(pos.file);
        self.pending_files.extend(directive.map(|d| (n, d)));
        n
    }

    /// The labels at the start and the end of a new block's code.
    pub fn block_labels(&mut self) -> (String, String) {
        self.blocks += 1;
        let n = self.blocks;
        (format!(".Ldbg.block.{n}"), format!(".Ldbg.block_end.{n}"))
    }

    /// The directives that say the next instruction comes from `pos`: a
    /// `.file` if its file has no number yet, then the `.loc`. `column`
    /// overrides the position's (0 for a function's first line).
    pub fn loc(&mut self, pos: Pos, column: Option<u32>, prologue_end: bool) -> Vec<String> {
        let (n, mut file) = self.number(pos.file);
        if let Some(i) = self.pending_files.iter().position(|(m, _)| *m == n) {
            file = Some(self.pending_files.remove(i).1);
        }
        let flags = if prologue_end { " prologue_end" } else { "" };
        let col = column.unwrap_or(pos.col);
        file.into_iter()
            .chain([format!(".loc {n} {} {col}{flags}", pos.line)])
            .collect()
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
                    if spelled.0.as_ref().is_some_and(|s| t == s.ty()) {
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
            // `const volatile T` is const of volatile of T, as clang has it.
            Type::Qual(t, q) => {
                let mut id = self.type_of(t, spelled);
                if q.volatile {
                    id = Some(self.intern(TypeKey::Qual(false, id), || TypeNode::Qual(false, id)));
                }
                if q.konst {
                    id = Some(self.intern(TypeKey::Qual(true, id), || TypeNode::Qual(true, id)));
                }
                id
            }
            Type::Func(f) => {
                let ret = self.type_of(&f.ret, spelled);
                let params: Vec<Option<usize>> = f
                    .params
                    .iter()
                    .map(|p| self.type_of(p, &Spelled::default()))
                    .collect();
                let key = TypeKey::Func(ret, params.clone(), f.variadic);
                Some(self.intern(key, || TypeNode::Func(ret, params, f.variadic)))
            }
        }
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
        let file = self.decl_file(r.pos);
        let id = self.intern(key, || TypeNode::Record(r.clone(), Vec::new(), file, r.pos.line));
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
                let node = TypeNode::Typedef(t.name.clone(), of, file, t.pos.line);
                self.intern(key, || node)
            }
            Spelling::Enum(e) => {
                self.keep.push(e.clone());
                let k = e.ty.int_kind().expect("an enum is an integer type");
                let of = self.intern(TypeKey::Base(k), || TypeNode::Base(k));
                let file = self.decl_file(e.pos);
                let node = TypeNode::Enum(e.tag.clone(), of, e.constants.clone(), file, e.pos.line);
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

    /// A global variable the file defines, at symbol `sym`.
    #[allow(clippy::too_many_arguments)]
    pub fn global(&mut self, name: &str, sym: &str, ty: &Type, spelled: &Spelled, pos: Pos, external: bool) {
        let var = self.var(name, ty, spelled, pos, Location::Symbol(sym.to_string()));
        self.globals.push(Global { var, external });
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
        pos: Pos,
        (external, prototyped): (bool, bool),
        ret: (&Type, &Spelled),
        params: Vec<Var>,
        variadic: bool,
        body: Scope,
    ) {
        let (file, _) = self.number(pos.file);
        let ret = self.type_of(ret.0, ret.1);
        self.functions.push(Function {
            name: name.to_string(),
            end: end.to_string(),
            file,
            line: pos.line,
            external,
            prototyped,
            ret,
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
        d.attr(AT_DECL_FILE, Val::Const(v.file.into()));
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
        })
    }

    /// The entries of a scope's variables in the frame and of its blocks.
    fn scope_children(&self, s: &Scope, out: &mut Vec<Die>) {
        for v in &s.vars {
            out.push(self.var_die(TAG_VARIABLE, v));
        }
        for b in &s.blocks {
            let mut d = Die::new(TAG_LEXICAL_BLOCK, "a block");
            d.attr(AT_LOW_PC, Val::Addr(b.begin.clone()));
            d.attr(AT_HIGH_PC, Val::Len(b.end.clone(), b.begin.clone()));
            self.scope_children(b, &mut d.children);
            out.push(d);
        }
    }

    fn function_die(&self, f: &Function) -> Die {
        let mut d = Die::new(TAG_SUBPROGRAM, &f.name);
        d.attr(AT_LOW_PC, Val::Addr(f.name.clone()));
        d.attr(AT_HIGH_PC, Val::Len(f.end.clone(), f.name.clone()));
        d.attr(AT_FRAME_BASE, Val::Expr(vec![Expr::Bytes(vec![OP_REG8])]));
        d.attr(AT_NAME, Val::Str(f.name.clone()));
        d.attr(AT_DECL_FILE, Val::Const(f.file.into()));
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
        for p in &f.params {
            d.children.push(self.var_die(TAG_FORMAL_PARAMETER, p));
        }
        if f.variadic {
            d.children
                .push(Die::new(TAG_UNSPECIFIED_PARAMETERS, "the variadic arguments"));
        }
        // The variables in the frame and the blocks, then every static local
        // of the function, as clang has them.
        self.scope_children(&f.body, &mut d.children);
        for v in &f.body.statics {
            d.children.push(self.static_die(v));
        }
        d
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
        d.attr(AT_DECL_FILE, Val::Const(v.file.into()));
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
    fn nested_types(&self) -> Vec<(usize, (usize, usize))> {
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
                match r.body() {
                    Some(body) => d.attr(AT_BYTE_SIZE, Val::Const(body.size.into())),
                    None => d.attr(AT_DECLARATION, Val::Flag),
                }
                d.attr(AT_DECL_FILE, Val::Const((*file).into()));
                d.attr(AT_DECL_LINE, Val::Const((*line).into()));
                for (i, (name, ty, offset, file, line)) in members.iter().enumerate() {
                    let mut m = Die::new(TAG_MEMBER, name);
                    m.attr(AT_NAME, Val::Str(name.clone()));
                    typed(&mut m, *ty);
                    m.attr(AT_DECL_FILE, Val::Const((*file).into()));
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
                d.attr(AT_PROTOTYPED, Val::Flag);
                typed(&mut d, *ret);
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
            TypeNode::Typedef(name, of, file, line) => {
                let mut d = Die::new(TAG_TYPEDEF, name);
                typed(&mut d, *of);
                d.attr(AT_NAME, Val::Str(name.clone()));
                d.attr(AT_DECL_FILE, Val::Const((*file).into()));
                d.attr(AT_DECL_LINE, Val::Const((*line).into()));
                d
            }
            TypeNode::Enum(tag, of, constants, file, line) => {
                let what = format!("enum {}", tag.as_deref().unwrap_or("(anonymous)"));
                let mut d = Die::new(TAG_ENUMERATION_TYPE, &what);
                d.attr(AT_TYPE, Val::Ref(*of));
                if let Some(t) = tag {
                    d.attr(AT_NAME, Val::Str(t.clone()));
                }
                d.attr(AT_BYTE_SIZE, Val::Const(4));
                d.attr(AT_DECL_FILE, Val::Const((*file).into()));
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
        // Arrays count their elements in the type clang names
        // __ARRAY_SIZE_TYPE__.
        if self.types.iter().any(|t| matches!(t, TypeNode::Array(..))) {
            self.intern(TypeKey::ArraySize, || TypeNode::ArraySize);
        }
        let first = &self.functions[0].name;
        let last_end = &self.functions[self.functions.len() - 1].end;

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
        for f in &self.functions {
            cu.children.push(self.function_die(f));
        }
        for g in &self.globals {
            cu.children.push(self.global_die(g));
        }
        let nested = self.nested_types();
        for id in 0..self.types.len() {
            if !nested.iter().any(|(n, _)| *n == id) {
                cu.children.push(self.type_die(id, &nested));
            }
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
        for (_, f) in &self.pending_files {
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
