//! Debug information (`-g`): where each instruction comes from in the source,
//! for debuggers (merlin-dbg, lldb) and `merlin-objdump -l`.
//!
//! merlin writes it as assembly, the way clang does, and leaves the encoding to
//! the assembler:
//!
//! * `.file N "dir" "name"` numbers the source files: 0 is the file being
//!   compiled, with the *compilation directory* (`-fdebug-compilation-dir`, the
//!   directory the names are relative to); then each header that has code, in
//!   the order its code comes.
//! * `.loc FILE LINE COLUMN` goes before the code of every statement (see
//!   [`crate::codegen`]): a function's first line (column 0) before its
//!   prologue, each statement's start, a loop's condition and step each time
//!   they run, and the closing brace before the epilogue. The first `.loc`
//!   after the prologue says `prologue_end`, where a debugger stops when it
//!   steps into the function. merlin-as turns them into the `.debug_line`
//!   line table.
//! * A *compile unit* in `.debug_info` (with its abbreviations in
//!   `.debug_abbrev`) says what the line table belongs to: the file, the
//!   compilation directory, the address range of the code and the offset of
//!   the line table (`DW_AT_stmt_list`), then one `DW_TAG_subprogram` per
//!   function (its name, address range and line). Debuggers find line tables
//!   through compile units: without one, lldb, llvm-objdump and
//!   merlin-objdump ignore `.debug_line`.
//!
//! Addresses (`DW_FORM_addr`) are the target's: 4 bytes written `.word` on
//! RV32, 8 bytes written `.quad` on RV64, as clang writes them, and the
//! unit's header says which (its address size).
//!
//! None of it changes the code: labels and directives assemble to no bytes
//! in `.text`, and the debug sections are not loaded.

use crate::types::{ptr_size, Xlen};
use crate::Pos;
use std::collections::HashMap;
use std::fmt::Write;

/// `-g`: emit debug information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugInfo {
    /// The directory the file names are relative to, as clang's
    /// `-fdebug-compilation-dir` (by default the directory merlin runs in).
    pub comp_dir: String,
}

/// `DW_TAG_*`, `DW_AT_*`, `DW_FORM_*` and the other constants the compile unit uses.
const TAG_COMPILE_UNIT: u8 = 0x11;
const TAG_SUBPROGRAM: u8 = 0x2e;
const AT_NAME: u8 = 0x03;
const AT_STMT_LIST: u8 = 0x10;
const AT_LOW_PC: u8 = 0x11;
const AT_HIGH_PC: u8 = 0x12;
const AT_LANGUAGE: u8 = 0x13;
const AT_COMP_DIR: u8 = 0x1b;
const AT_PRODUCER: u8 = 0x25;
const AT_DECL_FILE: u8 = 0x3a;
const AT_DECL_LINE: u8 = 0x3b;
const AT_EXTERNAL: u8 = 0x3f;
const FORM_ADDR: u8 = 0x01;
const FORM_DATA2: u8 = 0x05;
const FORM_DATA4: u8 = 0x06;
const FORM_STRING: u8 = 0x08;
const FORM_FLAG: u8 = 0x0c;
const FORM_SEC_OFFSET: u8 = 0x17;
const LANG_C99: u16 = 0x0c;
const UT_COMPILE: u8 = 1;

/// A function, for its `DW_TAG_subprogram`.
struct Function {
    name: String,
    /// The label just past its code.
    end: String,
    /// Its `.file` number and line.
    file: u32,
    line: u32,
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
    functions: Vec<Function>,
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
            functions: Vec::new(),
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

    /// The directives that say the next instruction comes from `pos`: a
    /// `.file` if its file has no number yet, then the `.loc`. `column`
    /// overrides the position's (0 for a function's first line).
    pub fn loc(&mut self, pos: Pos, column: Option<u32>, prologue_end: bool) -> Vec<String> {
        let (n, file) = self.number(pos.file);
        let flags = if prologue_end { " prologue_end" } else { "" };
        let col = column.unwrap_or(pos.col);
        file.into_iter()
            .chain([format!(".loc {n} {} {col}{flags}", pos.line)])
            .collect()
    }

    /// Records function `name`, whose code ends at label `end`, for its
    /// `DW_TAG_subprogram`.
    pub fn function(&mut self, name: &str, end: &str, pos: Pos, external: bool) {
        let (file, _) = self.number(pos.file);
        self.functions.push(Function {
            name: name.to_string(),
            end: end.to_string(),
            file,
            line: pos.line,
            external,
        });
    }

    /// The `.debug_abbrev` and `.debug_info` sections, and the label at the
    /// start of `.debug_line` that the compile unit points at. Nothing if the
    /// file has no functions (no code to describe).
    pub fn sections(&self) -> String {
        let (Some(first), Some(last)) = (self.functions.first(), self.functions.last()) else {
            return String::new();
        };
        let mut s = String::new();
        let line = |s: &mut String, text: String| {
            let _ = writeln!(s, "    {text}");
        };
        let bytes = |b: &[u8]| {
            let list: Vec<String> = b.iter().map(|b| format!("{b:#04x}")).collect();
            format!(".byte {}", list.join(", "))
        };

        let _ = writeln!(s, "\n    .section .debug_abbrev,\"\",@progbits");
        let _ = writeln!(s, ".Ldebug_abbrev0:");
        line(
            &mut s,
            format!(
                "{}    # 1: the compile unit, with children",
                bytes(&[1, TAG_COMPILE_UNIT, 1])
            ),
        );
        for (at, form) in [
            (AT_PRODUCER, FORM_STRING),
            (AT_LANGUAGE, FORM_DATA2),
            (AT_NAME, FORM_STRING),
            (AT_COMP_DIR, FORM_STRING),
            (AT_STMT_LIST, FORM_SEC_OFFSET),
            (AT_LOW_PC, FORM_ADDR),
            (AT_HIGH_PC, FORM_DATA4),
            (0, 0),
        ] {
            line(&mut s, bytes(&[at, form]));
        }
        line(
            &mut s,
            format!("{}    # 2: a function", bytes(&[2, TAG_SUBPROGRAM, 0])),
        );
        for (at, form) in [
            (AT_LOW_PC, FORM_ADDR),
            (AT_HIGH_PC, FORM_DATA4),
            (AT_NAME, FORM_STRING),
            (AT_DECL_FILE, FORM_DATA2),
            (AT_DECL_LINE, FORM_DATA4),
            (AT_EXTERNAL, FORM_FLAG),
            (0, 0),
        ] {
            line(&mut s, bytes(&[at, form]));
        }
        line(&mut s, ".byte 0    # the end of the abbreviations".into());

        // The unit's length counts what follows its own 4 bytes.
        let asciz = |t: &str| t.len() as u32 + 1;
        let producer = "merlin";
        let name = self.name(0);
        let addr = ptr_size(self.xlen);
        let addr_directive = match self.xlen {
            Xlen::Rv32 => ".word",
            Xlen::Rv64 => ".quad",
        };
        let mut length = 2 + 1 + 1 + 4; // version, unit type, address size, abbreviations
        length += 1 + asciz(producer) + 2 + asciz(&name) + asciz(&self.info.comp_dir) + 4 + addr + 4;
        for f in &self.functions {
            length += 1 + addr + 4 + asciz(&f.name) + 2 + 4 + 1;
        }
        length += 1; // the end of the unit's children

        let _ = writeln!(s, "\n    .section .debug_info,\"\",@progbits");
        line(&mut s, format!(".word {length}    # unit length"));
        line(&mut s, ".half 5    # DWARF version".into());
        line(&mut s, format!(".byte {UT_COMPILE}    # a compile unit"));
        line(&mut s, format!(".byte {addr}    # address size"));
        line(&mut s, ".word .Ldebug_abbrev0".into());
        line(&mut s, ".byte 1    # the compile unit".into());
        line(&mut s, format!(".asciz {}", quoted(producer)));
        line(&mut s, format!(".half {LANG_C99:#x}    # C99"));
        line(&mut s, format!(".asciz {}", quoted(&name)));
        line(&mut s, format!(".asciz {}", quoted(&self.info.comp_dir)));
        line(&mut s, ".word .Lline_table_start0".into());
        line(&mut s, format!("{addr_directive} {}", first.name));
        line(&mut s, format!(".word {} - {}", last.end, first.name));
        for f in &self.functions {
            line(&mut s, format!(".byte 2    # {}", f.name));
            line(&mut s, format!("{addr_directive} {}", f.name));
            line(&mut s, format!(".word {} - {}", f.end, f.name));
            line(&mut s, format!(".asciz {}", quoted(&f.name)));
            line(&mut s, format!(".half {}", f.file));
            line(&mut s, format!(".word {}", f.line));
            line(&mut s, format!(".byte {}", u8::from(f.external)));
        }
        line(
            &mut s,
            ".byte 0    # the end of the compile unit's children".into(),
        );

        let _ = writeln!(s, "\n    .section .debug_line,\"\",@progbits");
        let _ = writeln!(s, ".Lline_table_start0:");
        s
    }
}
