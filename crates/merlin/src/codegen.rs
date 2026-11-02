//! # Code generation: AST to Excalibur assembly
//!
//! Turns a checked [`Program`] into `.s` text for merlin-as. It is a *stack
//! machine* code generator, the approach of Rui Ueyama's chibicc: every
//! expression leaves its value in `a0`, and a value that must survive while
//! another expression is evaluated is pushed on the stack. Nothing is kept in
//! registers across statements and every variable lives in memory, so the
//! output is long but easy to follow, and easy to check by hand.
//!
//! ## Contract
//!
//! * Input: a [`Program`] that passed [`crate::sema`]. Every expression has its
//!   C type, conversions are explicit cast nodes, `sizeof` is folded, every
//!   member access knows its offset, initializer lists are fully braced, and
//!   every variable says where it lives: local `#i` of its function
//!   (parameters first, then the declared locals and the hidden ones calls
//!   need) or a global symbol. Every call has the right number of arguments
//!   (those `...` takes promoted), `va_end` and `va_copy` have become plain
//!   expressions, and every compound literal has its hidden local.
//! * Output: assembly merlin-as accepts (and so does clang): functions in
//!   `.text`, initialized variables in `.data`, zero-initialized ones in
//!   `.bss`, `const` ones and string literals in `.rodata`. Every function and
//!   variable that is not `static` gets `.globl` (except a C99 inline
//!   definition, kept local); declarations without a definition emit nothing
//!   (the linker finds them). Like clang, every function and variable gets
//!   `.type` (`@function` or `@object`) and `.size`, so the symbol table says
//!   what each address belongs to.
//! * Semantics: 32-bit wrapping arithmetic, `/` and `%` truncate toward zero
//!   (`div`/`rem`, or `divu`/`remu` when unsigned), shifts use the low 5 bits
//!   of the amount, comparisons and `! && ||` produce 0 or 1, `&&`/`||`
//!   short-circuit, an assignment evaluates to the stored value. Falling off
//!   the end of a function returns 0.
//! * Memory: every read and write the source asks for is done, once, in
//!   order, and nothing is kept in a register across statements. So
//!   `volatile` needs nothing special: the code generator never caches,
//!   removes, merges or reorders memory accesses.
//!
//! ## Values and types
//!
//! Every value in a register is 32 bits wide. A `char` or `short` is widened
//! when it is loaded (`lbu`/`lb`, `lhu`/`lh`, by its signedness; plain `char`
//! is unsigned) and narrowed when it is stored (`sb`, `sh`), so integer
//! promotion costs nothing. A conversion *to* a narrow type in a register (a
//! cast, or the value of `c = 300` for a `char c`) is done in place:
//!
//! ```text
//!   andi  a0, a0, 255         # to unsigned char (and plain char)
//!   slli  a0, a0, 24          # to signed char: keep the low byte...
//!   srai  a0, a0, 24          # ...and copy its top bit up
//!   slli  a0, a0, 16          # to unsigned short (srai for short)
//!   srli  a0, a0, 16
//! ```
//!
//! The usual arithmetic conversions decide the instruction: `divu`, `remu`,
//! `sltu` and `srl` when the operation is unsigned (an operand is `unsigned
//! int` or `unsigned long`, or a pointer), `div`, `rem`, `slt` and `sra`
//! otherwise.
//!
//! A `_Bool` holds 0 or 1: converting to it is `snez a0, a0` (from a pointer
//! too), and it is loaded with `lbu`.
//!
//! An array is never loaded: an expression of array type leaves its address in
//! `a0`, which is exactly C's array-to-pointer decay. A function's name
//! leaves the function's address (`la a0, f`), and so does `*fp`.
//!
//! ## Structs and unions
//!
//! A struct (or union) is not a value that fits a register, so an expression
//! of struct type leaves the struct's *address* in `a0`, like an array: a
//! variable's slot, `*p`'s `p`, a member's address inside its parent. Then
//!
//! * `s.m` and `p->m` add the member's offset (sema worked it out) to that
//!   address, and load the member if it is a scalar;
//! * `a = b` copies the bytes from `b`'s address to `a`'s, and leaves `a`'s
//!   address (the assignment's value);
//! * a struct a call returns is kept in a hidden local of the caller (sema
//!   adds one per such call), whose address is the call's value: so
//!   `f().m` is a member access like any other.
//!
//! Copies go in units of the struct's alignment, at most a word (`lw`/`sw`,
//! `lhu`/`sh`, `lbu`/`sb`), so no access is misaligned: unrolled for up to 16
//! units, a loop for more. Struct locals get word-aligned slots that are a
//! whole number of words long, so that a struct arriving in registers can be
//! stored with `sw`.
//!
//! ## Calling convention (RISC-V ILP32)
//!
//! * Arguments go in `a0-a7`, left to right, and the words past `a7` on the
//!   stack, from `0(sp)` up at the call; the result comes back in `a0`.
//!   Arguments and results narrower than 32 bits travel widened by their type
//!   (sema's casts do that). The arguments `...` takes travel the same way.
//! * A struct of up to 8 bytes travels in one or two registers, its bytes as
//!   they lie in memory: it is loaded from its address word by word (put
//!   together from smaller aligned pieces if it is not word-aligned). A
//!   bigger one is passed as the address of a copy the caller makes (in a
//!   hidden local), and returned through memory: the caller passes the
//!   address of its result local as a hidden first argument in `a0`, and the
//!   callee copies the result there. [`crate::abi`] has the rules.
//! * `ra` holds the return address (`call f` is `auipc ra` + `jalr ra`; a
//!   call through a pointer loads it into `t1` and does `jalr t1`).
//! * `sp`, `s0-s11` must look untouched to the caller when we return; the
//!   `a` and `t` registers may be clobbered by any call. We only ever use
//!   `s0` (saved in the prologue), the argument registers, and `t0-t4` as
//!   scratch.
//! * `sp` is 16-byte aligned at every call.
//!
//! ## Stack frame
//!
//! `s0` (the frame pointer) holds the value `sp` had on entry, so the caller's
//! frame starts at `s0` and ours lies below it:
//!
//! ```text
//!            caller's frame
//!   s0 ->  +------------------+  (16-byte aligned: sp before the call)
//!          | saved ra         |  -4(s0)
//!          | caller's s0      |  -8(s0)
//!          | local #0         |           parameters first: a0 is stored here,
//!          | local #1         |           a1 next, and so on
//!          | ...              |
//!          | (padding)        |           frame size rounded up to 16
//!   sp ->  +------------------+  (16-byte aligned) after the prologue
//!          | temporaries      |  pushed and popped while evaluating expressions
//! ```
//!
//! The caller's stack arguments, if any, are right above: word 8 of the
//! arguments at `0(s0)`, word 9 at `4(s0)`, and so on.
//!
//! Locals are placed downwards from `-8(s0)` in order, each at the next
//! offset aligned for its type (arrays at least to 4 bytes): an `int` takes the
//! 4 bytes below the previous local, a `char` 1 byte, an `int a[3]` 12 bytes.
//! With only `int`s, local `#i` is at `-(12 + 4*i)(s0)`. Sema gives every
//! declaration in the function its own local (shadowed names included), so a
//! slot never has to be reused and scopes cost nothing here.
//!
//! The prologue saves `ra` and `s0` in a fixed 16 bytes, points `s0` at the top
//! of the frame and then makes room for the rest; the epilogue undoes it from
//! `s0`, so it never needs to know the frame size:
//!
//! ```text
//!   f:  addi  sp, sp, -16          .L.return.f:              # result in a0
//!       sw    ra, 12(sp)               addi  sp, s0, -16
//!       sw    s0, 8(sp)                lw    ra, 12(sp)
//!       addi  s0, sp, 16               lw    s0, 8(sp)
//!       addi  sp, sp, -(frame - 16)    addi  sp, sp, 16
//!       sw    a0, -12(s0)  # params    ret
//! ```
//!
//! A struct parameter that came in registers is stored word by word; one
//! passed by reference is copied from the caller's copy into its slot. A
//! parameter on the stack is loaded from above `s0` (through `t1`) and
//! stored into its slot like the others. A function returning a big struct
//! saves the result address (`a0`) in a hidden local.
//!
//! ## Variadic functions
//!
//! A variadic function cannot know where its `...` arguments are until it
//! reads them, so its prologue stores the argument registers its named
//! parameters leave free (`a<n>` to `a7`) at the very top of its frame, right
//! below `s0`, where the caller's stack words continue them. Every variadic
//! argument then lies in one run of words from there up, as if all had been
//! passed on the stack, and a `va_list` is a plain pointer that walks it (as
//! clang does). `ra` and `s0` go below the saved registers, and the fixed
//! part of the frame grows to a multiple of 16 that holds them all:
//!
//! ```text
//!   int f(int n, ...)                    s0 ->  +-----------+
//!     addi  sp, sp, -48                         | a7 .. a1  |  -4(s0) .. -28(s0)
//!     sw    ra, 16(sp)    # -32(s0)             | saved ra  |  -32(s0)
//!     sw    s0, 12(sp)    # -36(s0)             | caller s0 |  -36(s0)
//!     addi  s0, sp, 48                          | n         |  -40(s0), locals...
//!     sw    a1, -28(s0)   ... sw a7, -4(s0)
//! ```
//!
//! `va_start(ap, n)` points `ap` at the first variadic word (`-28(s0)` here;
//! `4*k(s0)` when the named parameters take `8 + k` words, all registers and
//! some stack). `va_arg(ap, T)` takes the words of a `T` at `ap` and moves
//! `ap` past them: a scalar is loaded, a small struct's value is its address
//! right there, and a big struct's is the word there (the address of the
//! caller's copy). `va_end` does nothing and `va_copy` copies the pointer.
//!
//! ## Expressions
//!
//! `expr(e)` emits code that leaves `e`'s value in `a0`. For `l op r`:
//!
//! ```text
//!   <l>                    # a0 = l
//!   addi  sp, sp, -4       # push a0: <r> may need a0 (and any register)
//!   sw    a0, 0(sp)
//!   <r>                    # a0 = r
//!   lw    a1, 0(sp)        # pop: a1 = l
//!   addi  sp, sp, 4
//!   sub   a0, a1, a0       # a0 = l - r
//! ```
//!
//! Comparisons give 0 or 1 with `slt` (`l < r`), `slt` with the operands
//! swapped (`l > r`), those two followed by `xori 1` (`>=`, `<=`), and `xor`
//! followed by `seqz`/`snez` (`==`, `!=`). `&&` and `||` branch around their
//! right side and turn the result into 0 or 1 with `snez`; `?:` branches like
//! an `if`.
//!
//! Pointer arithmetic counts elements: `p + i` scales `i` by the size of what
//! `p` points to (a shift for powers of two), and `p - q` divides the byte
//! difference by it.
//!
//! ## Places: where assignments store
//!
//! The target of `=`, `op=`, `++` and `--` is a *place*. A scalar local is
//! reached directly at its offset from `s0`; for anything else (a global,
//! `*p`, `a[i]`) the address is computed first and pushed, so that the value
//! can be computed with any registers, then popped into `a1` for the store:
//!
//! ```text
//!   x = 5;   (local)          a[i] = 5;
//!     li    a0, 5               <address of a[i]>
//!     sw    a0, -12(s0)         addi sp, sp, -4    # push it
//!                               sw   a0, 0(sp)
//!                               li   a0, 5
//!                               lw   a1, 0(sp)     # pop it
//!                               addi sp, sp, 4
//!                               sw   a0, 0(a1)
//! ```
//!
//! `x op= v` loads the place's value (peeking at the pushed address, which
//! stays for the store), pushes it, evaluates `v`, applies the operator,
//! converts the result to `x`'s type and stores it, so `x` is evaluated once.
//! `++x` and `x++` add the step (1, or the pointed-to size) in place; `x++`
//! then subtracts it again to give back the old value.
//!
//! Globals are reached through their address: `la a0, g` (`auipc` + `addi`,
//! PC-relative), then a load or store at `0(a0)`.
//!
//! A call evaluates its arguments left to right, pushing all but the last;
//! the last goes from `a0` to `a(n-1)` with `mv`, then the others are popped
//! into `a(n-2)` down to `a0` (a small struct's address is popped into `t1`
//! and its words loaded from there). A call through a pointer evaluates the
//! pointer first and pops it into `t1` last. If temporaries of an enclosing
//! expression are still pushed (as in `x + f(y)`), `sp` is lowered to a
//! multiple of 16 around the `call`, to keep the convention's alignment.
//!
//! When the arguments take more than `a0-a7`, every one of them is pushed;
//! then `sp` goes down far enough for the stack words (keeping it aligned),
//! each stack word is copied from its pushed argument to its place at
//! `0(sp)`, `4(sp)`, ..., and the register words are loaded from theirs. One
//! `addi` after the call frees the words and the pushed arguments together.
//!
//! ## Initializers
//!
//! A local scalar's initializer is an assignment. A local array or struct
//! with an initializer list is first cleared (word by word, or with a loop
//! when it is big), then each element or member given is stored; a struct
//! initialized by another struct is copied. A local without an initializer is
//! left as it is (C: indeterminate). Sema has resolved designators already:
//! a subobject one skips has an empty list, which stores nothing, and a
//! union's list ends with the member it holds.
//!
//! A compound literal `(T){ ... }` is a hidden local initialized the same
//! way each time the expression is evaluated; its value is the local's
//! address (loaded for a scalar `T`).
//!
//! Globals and `static` locals are data: the initializer, a constant
//! expression, is written out with `.byte`, `.half`, `.word` (`.word sym+off`
//! for an address, which the linker fills in), `.ascii` and `.zero` (struct
//! padding too), after a `.balign` for the type. A `static` local's symbol is
//! `function.name`, a local symbol (no `.globl`). Each string literal gets a
//! label `.L.str.<n>` in `.rodata`.
//!
//! ## Control flow
//!
//! Conditions are evaluated into `a0` and tested with `beqz`/`bnez`. Jumps go
//! to local labels named `.L.<what>.<n>`, numbered across the whole file
//! (`.L.else.3`, `.L.break.7`); every function returns through
//! `.L.return.<name>`.
//!
//! A `switch` is a chain of compares, one `li t0, value` + `beq a0, t0` per
//! `case` in source order, then a jump to `default` (or past the body). The
//! body follows unchanged, its `case` labels (`.L.case.<function>.<n>`) where
//! they stand, so control falls through from one to the next, and `break`
//! jumps to the end. A table of addresses would be faster for dense cases; a
//! chain is simpler to read and works for any values. A C label `name` is
//! `.L.label.<function>.<name>`, and `goto` is a `j` to it.
//!
//! A conditional branch reaches only ±4 KiB, and merlin-as reports a branch
//! that cannot reach its label as an error rather than rewriting it. So once a
//! function is generated, [`relax_branches`] estimates every instruction's
//! address (never underestimating a distance) and turns a branch that may be
//! too far into the opposite branch over a `j`, which reaches ±1 MiB:
//!
//! ```text
//!   beqz  a0, .L.end.4      becomes      bnez  a0, 1f
//!                                        j     .L.end.4
//!                                     1:
//! ```
//!
//! (and `beq a0, t0, ...` becomes `bne a0, t0, 1f` over a `j`).

use crate::abi::{self, Pass};
use crate::ast::*;
use crate::consteval::{self, Base, Const};
use crate::sema::arith_type;
use crate::types::{FuncType, IntKind, Type};
use crate::Error;
use std::collections::HashMap;
use std::fmt::Write;

/// Generates assembly for a whole translation unit.
pub fn generate(prog: &Program) -> Result<String, Error> {
    // merlin-as reads a register name as a register, never as a symbol, so a
    // function or global called `sp` or `a0` could not be referenced or even
    // defined.
    for item in &prog.items {
        let names: Vec<(&str, crate::Pos)> = match item {
            Item::Func(f) => vec![(&f.name, f.pos)],
            Item::Decl(ds) => ds.iter().map(|d| (d.name.as_str(), d.pos)).collect(),
        };
        if let Some((name, pos)) = names
            .into_iter()
            .find(|(n, _)| excalibur::reg::parse(n).is_some())
        {
            return Err(Error::new(
                pos,
                format!("'{name}' is a register name, which cannot be used as a symbol name"),
            ));
        }
    }

    let mut file = FileGen {
        out: String::new(),
        section: "",
        labels: 0,
        strings: Vec::new(),
    };
    for item in &prog.items {
        match item {
            Item::Func(f) => {
                let Some(body) = &f.body else {
                    continue; // a prototype: the definition is elsewhere
                };
                // A variadic function keeps the argument registers its named
                // parameters leave free at the top of its frame.
                let Type::Func(ft) = f.ty() else {
                    unreachable!("a function's type")
                };
                let (_, named_words) = abi::arguments(&ft);
                let va_save = if f.variadic {
                    4 * (abi::ARG_REGS - named_words.min(abi::ARG_REGS)) as i32
                } else {
                    0
                };
                let Some((offsets, frame)) = layout(&f.locals, SAVED_REGS + va_save) else {
                    let msg = format!("the locals of '{}' need a stack frame larger than 2 GiB", f.name);
                    return Err(Error::new(f.pos, msg));
                };
                let mut g = FuncGen {
                    func: f,
                    offsets,
                    frame,
                    named_words,
                    va_save,
                    lines: Vec::new(),
                    depth: 0,
                    breaks: Vec::new(),
                    continues: Vec::new(),
                    file: &mut file,
                    ret_label: format!(".L.return.{}", f.name),
                    statics: Vec::new(),
                };
                g.function(body);
                let (lines, statics) = (g.lines, g.statics);
                file.switch(".text");
                file.out.push('\n');
                render(&lines, &relax_branches(&lines), &mut file.out);
                for d in statics {
                    file.variable(d);
                }
            }
            Item::Decl(decls) => {
                for d in decls.iter().filter(|d| d.define) {
                    file.variable(d);
                }
            }
        }
    }
    file.strings();
    Ok(file.out)
}

/// `ra` and the caller's `s0` take the top 8 bytes of every frame (below the
/// saved argument registers of a variadic function).
const SAVED_REGS: i32 = 8;

/// The largest frame: offsets from `s0` and the frame size must stay `i32`s.
const MAX_FRAME: i64 = i32::MAX as i64 & !15;

/// Places every local in the frame, below the `header` bytes at its top
/// (`ra`, `s0` and a variadic function's saved registers): its offset from
/// `s0`, and the frame size (a multiple of 16, the convention's stack
/// alignment). `None` if the frame would be larger than [`MAX_FRAME`].
fn layout(locals: &[Local], header: i32) -> Option<(Vec<i32>, i32)> {
    // In i64: each local is at most `i32::MAX` bytes, but their sum need not be.
    let mut off = -i64::from(header);
    let mut offsets = Vec::with_capacity(locals.len());
    for l in locals {
        let mut size = l.ty.size().expect("sema: locals have complete types");
        // Arrays and structs are word-aligned so that clearing them can use
        // `sw`, and a struct's slot is a whole number of words, so that one
        // arriving in registers can be stored with `sw` (see [`abi`]).
        let align = if l.ty.is_array() || l.ty.is_record() {
            l.ty.align().max(4)
        } else {
            l.ty.align()
        };
        if l.ty.is_record() {
            size = size.next_multiple_of(4);
        }
        // Round down to the alignment (two's complement: works below zero).
        off = (off - i64::from(size)) & !(i64::from(align) - 1);
        if (-off + 15) & !15 > MAX_FRAME {
            return None;
        }
        offsets.push(off as i32);
    }
    Some((offsets, ((-off + 15) & !15) as i32))
}

/// How a call's arguments travel.
struct ArgLayout {
    /// The arguments' types: the parameters', then for `...` the (promoted)
    /// arguments' own.
    types: Vec<Type>,
    /// Each argument's first word and how it is passed (see [`abi`]).
    words: Vec<(usize, Pass)>,
    /// How many words they take in all, with a hidden result address.
    used: usize,
}

impl ArgLayout {
    fn new(ft: &FuncType, args: &[Expr]) -> ArgLayout {
        let mut types = ft.params.clone();
        types.extend(args[ft.params.len()..].iter().map(|a| a.ty.clone()));
        let (words, used) = abi::call_arguments(&ft.ret, &types);
        ArgLayout { types, words, used }
    }
}

/// Where word `word` of a call's arguments is (see [`abi`]): a register, or
/// an offset from the `sp` of the call (the callee's `s0`).
enum ArgWord {
    Reg(String),
    Stack(i32),
}

fn arg_word(word: usize) -> ArgWord {
    if word < abi::ARG_REGS {
        ArgWord::Reg(format!("a{word}"))
    } else {
        ArgWord::Stack(4 * (word - abi::ARG_REGS) as i32)
    }
}

/// Does `v` fit a 12-bit signed immediate (`addi`, `lw`, `sw`)?
fn fits_imm12(v: i32) -> bool {
    (-2048..=2047).contains(&v)
}

/// The load that brings a value of type `ty` into a register, widened.
fn load_op(ty: &Type) -> &'static str {
    match ty.unqual() {
        // `_Bool` holds 0 or 1: `lbu`.
        Type::Int(k) => match (k.size(), k.signed()) {
            (1, true) => "lb",
            (1, false) => "lbu",
            (2, true) => "lh",
            (2, false) => "lhu",
            _ => "lw",
        },
        Type::Ptr(_) => "lw",
        _ => unreachable!("no value of type '{ty}' is loaded"),
    }
}

/// The store that writes a value of type `ty` (its low bytes) to memory.
fn store_op(ty: &Type) -> &'static str {
    match ty.size() {
        Some(1) => "sb",
        Some(2) => "sh",
        Some(4) => "sw",
        _ => unreachable!("no value of type '{ty}' is stored"),
    }
}

/// `log2(n)` if `n` is a power of two.
fn log2(n: u32) -> Option<u32> {
    n.is_power_of_two().then(|| n.trailing_zeros())
}

/// What is shared by the whole file: the output, the label counter and the
/// string literals seen so far.
struct FileGen {
    out: String,
    /// The section the output is in.
    section: &'static str,
    /// Label counter shared by every function in the file, so labels are unique.
    labels: usize,
    /// String literals, emitted at the end; literal `i` is `.L.str.<i>`.
    strings: Vec<Vec<u8>>,
}

impl FileGen {
    fn switch(&mut self, section: &'static str) {
        if self.section != section {
            let directive = if section == ".rodata" {
                ".section .rodata"
            } else {
                section
            };
            // A blank line sets the new section apart from what came before.
            if !self.out.is_empty() {
                self.out.push('\n');
            }
            let _ = writeln!(self.out, "    {directive}");
            self.section = section;
        }
    }

    /// The label of a new string literal.
    fn string(&mut self, bytes: &[u8]) -> String {
        self.strings.push(bytes.to_vec());
        format!(".L.str.{}", self.strings.len() - 1)
    }

    /// Emits a global variable or `static` local: its symbol and its data.
    fn variable(&mut self, d: &Declarator) {
        let Some(VarRef::Global(sym)) = &d.var else {
            unreachable!("sema gives globals and statics a symbol")
        };
        let mut data = Vec::new();
        if let Some(init) = &d.init {
            self.data(&d.ty, init, &mut data);
        }
        // A `const` object never changes: read-only data, as clang does (a
        // `const volatile` one may be changed from outside, so it stays
        // writable).
        let quals = d.ty.quals();
        if quals.konst && !quals.volatile {
            self.switch(".rodata");
        } else if d.init.is_some() {
            self.switch(".data");
        } else {
            self.switch(".bss");
        }
        let size = d.ty.size().expect("sema: definitions have complete types");
        let storage = if d.storage == Storage::Static {
            "static "
        } else {
            ""
        };
        let bytes = if size == 1 { "byte" } else { "bytes" };
        let _ = writeln!(
            self.out,
            "\n    # {storage}{}: {size} {bytes}",
            d.ty.declare(&d.name)
        );
        if d.storage != Storage::Static && !sym.contains('.') {
            let _ = writeln!(self.out, "    .globl {sym}");
        }
        if d.ty.align() > 1 {
            let _ = writeln!(self.out, "    .balign {}", d.ty.align());
        }
        // An object symbol with its size, as for functions: debuggers and
        // tests find what an address belongs to from the symbol table.
        let _ = writeln!(self.out, "    .type {sym}, @object");
        let _ = writeln!(self.out, "    .size {sym}, {size}");
        let _ = writeln!(self.out, "{sym}:");
        if d.init.is_none() {
            data.push(format!(".zero {size}"));
        }
        for line in data {
            let _ = writeln!(self.out, "    {line}");
        }
    }

    /// The directives for an object of type `ty` initialized by `init`.
    fn data(&mut self, ty: &Type, init: &Init, out: &mut Vec<String>) {
        match (ty.unqual(), init) {
            // A subobject a designator skipped: zero.
            (_, Init::List(items, _)) if items.is_empty() => {
                out.push(format!(".zero {}", ty.size().expect("a complete type")));
            }
            (Type::Record(r), Init::List(items, _)) if r.union => {
                // The member the union is initialized with is the last item.
                let body = r.body().expect("sema: definitions have complete types");
                let k = items.len() - 1;
                let m = &body.members[k];
                self.data(&m.ty, &items[k], out);
                let size = m.ty.size().expect("complete member");
                if body.size > size {
                    out.push(format!(".zero {}", body.size - size));
                }
            }
            (Type::Record(r), Init::List(items, _)) => {
                // Members in order (sema's list is fully braced; a union's has
                // at most its first member), with the padding between them and
                // after the last as zeros.
                let body = r.body().expect("sema: definitions have complete types");
                let mut at = 0;
                for (m, item) in body.members.iter().zip(items) {
                    if m.offset > at {
                        out.push(format!(".zero {}", m.offset - at));
                    }
                    self.data(&m.ty, item, out);
                    at = m.offset + m.ty.size().expect("complete member");
                }
                if body.size > at {
                    out.push(format!(".zero {}", body.size - at));
                }
            }
            (Type::Array(elem, Some(n)), Init::List(items, _)) => {
                for item in items {
                    self.data(elem, item, out);
                }
                let rest = (*n as usize - items.len()) as u32 * elem.size().unwrap();
                if rest > 0 {
                    out.push(format!(".zero {rest}"));
                }
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    ..
                }),
            ) => {
                // The text, cut to the array (C allows leaving out the NUL), then zeros.
                let take = bytes.len().min(*n as usize);
                out.extend(byte_directives(&bytes[..take]));
                if *n as usize > take {
                    out.push(format!(".zero {}", *n as usize - take));
                }
            }
            (_, Init::Expr(e)) => {
                let value = consteval::eval(e).expect("sema: static initializers are constant");
                let directive = match ty.size() {
                    Some(1) => ".byte",
                    Some(2) => ".half",
                    _ => ".word",
                };
                let operand = match value {
                    Const::Int(v, _) => v.to_string(),
                    Const::Addr { base: None, offset } => offset.to_string(),
                    Const::Addr {
                        base: Some(base),
                        offset,
                    } => {
                        let sym = match base {
                            Base::Sym(s) => s,
                            Base::Str(bytes) => self.string(&bytes),
                        };
                        match offset {
                            0 => sym,
                            o if o > 0 => format!("{sym}+{o}"),
                            o => format!("{sym}{o}"),
                        }
                    }
                };
                out.push(format!("{directive} {operand}"));
            }
            _ => unreachable!("sema checked initializer shapes"),
        }
    }

    /// Emits every string literal, NUL-terminated, in `.rodata`.
    fn strings(&mut self) {
        if self.strings.is_empty() {
            return;
        }
        self.switch(".rodata");
        for (i, bytes) in std::mem::take(&mut self.strings).iter().enumerate() {
            let _ = writeln!(self.out, "\n.L.str.{i}:");
            let mut with_nul = bytes.clone();
            with_nul.push(0);
            for line in byte_directives(&with_nul) {
                let _ = writeln!(self.out, "    {line}");
            }
        }
    }
}

/// Bytes as data directives: text as `.ascii "..."` (or `.asciz` when it ends
/// with the NUL), anything else as `.byte` lists. Only escapes that every
/// assembler reads the same are used (`\n \t \" \\`).
fn byte_directives(bytes: &[u8]) -> Vec<String> {
    let printable = |b: u8| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t';
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if printable(bytes[i]) {
            while i < bytes.len() && printable(bytes[i]) {
                i += 1;
            }
            let mut text = String::new();
            for &b in &bytes[start..i] {
                match b {
                    b'"' => text.push_str("\\\""),
                    b'\\' => text.push_str("\\\\"),
                    b'\n' => text.push_str("\\n"),
                    b'\t' => text.push_str("\\t"),
                    _ => text.push(b as char),
                }
            }
            // A NUL right after the text: one `.asciz` says both.
            if i + 1 == bytes.len() && bytes[i] == 0 {
                out.push(format!(".asciz \"{text}\""));
                i += 1;
            } else {
                out.push(format!(".ascii \"{text}\""));
            }
        } else {
            while i < bytes.len() && !printable(bytes[i]) && i - start < 16 {
                i += 1;
            }
            let list: Vec<String> = bytes[start..i].iter().map(u8::to_string).collect();
            out.push(format!(".byte {}", list.join(", ")));
        }
    }
    out
}

/// One line of a function's assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    /// `name:`
    Label(String),
    /// An instruction, already formatted, and the most bytes it can assemble
    /// to (pseudo-instructions like `li`, `la` and `call` can take two words).
    Insn(String, u32),
    /// A conditional jump, `op regs, target` (`beqz a0, .L.end.3`, `beq a0,
    /// t0, .L.case.f.2`), whose reach is limited: see [`relax_branches`].
    Branch {
        op: &'static str,
        regs: &'static str,
        target: String,
    },
    /// `# text`, or a directive such as `.globl f`: no bytes.
    Text(String),
}

/// Where the target of an assignment is, once its address is known.
enum Place {
    /// A scalar local, at this offset from `s0`.
    Frame(i32),
    /// Anywhere else: its address is the top temporary on the stack.
    Pushed,
}

/// Per-function code generator state. `'a` is the tree's lifetime, `'f` the
/// (shorter) borrow of the file's state.
struct FuncGen<'a, 'f> {
    func: &'a Function,
    /// Offset from `s0` of each local.
    offsets: Vec<i32>,
    frame: i32,
    /// How many argument words the named parameters take (see [`abi`]).
    named_words: usize,
    /// In a variadic function, the bytes of argument registers saved at the
    /// top of the frame (those the named parameters leave free); else 0.
    va_save: i32,
    lines: Vec<Line>,
    /// How many 4-byte temporaries expression evaluation has pushed right now.
    /// Always 0 between statements.
    depth: u32,
    /// Where `break` goes for each enclosing loop or `switch`, and where
    /// `continue` goes for each enclosing loop, innermost last.
    breaks: Vec<String>,
    continues: Vec<String>,
    file: &'f mut FileGen,
    /// Every `return` jumps here, to the epilogue.
    ret_label: String,
    /// The function's `static` locals, emitted as data after it.
    statics: Vec<&'a Declarator>,
}

impl<'a> FuncGen<'a, '_> {
    // ---- emitting lines ----

    /// Emits a one-word instruction: `mnemonic operands`.
    fn ins(&mut self, mnemonic: &str, operands: impl AsRef<str>) {
        self.ins_sized(4, mnemonic, operands);
    }

    fn ins_sized(&mut self, size: u32, mnemonic: &str, operands: impl AsRef<str>) {
        self.lines
            .push(Line::Insn(insn_text(mnemonic, operands.as_ref()), size));
    }

    /// `li reg, value`: one instruction for small values, two for others.
    fn li(&mut self, reg: &str, value: i32) {
        let size = if fits_imm12(value) { 4 } else { 8 };
        self.ins_sized(size, "li", format!("{reg}, {value}"));
    }

    /// `la reg, symbol`: `auipc` + `addi`.
    fn la(&mut self, reg: &str, symbol: &str) {
        self.ins_sized(8, "la", format!("{reg}, {symbol}"));
    }

    fn comment(&mut self, text: impl AsRef<str>) {
        self.lines.push(Line::Text(format!("# {}", text.as_ref())));
    }

    fn label(&mut self, name: &str) {
        self.lines.push(Line::Label(name.to_string()));
    }

    fn branch(&mut self, op: &'static str, regs: &'static str, target: &str) {
        self.lines.push(Line::Branch {
            op,
            regs,
            target: target.to_string(),
        });
    }

    fn branch_if_zero(&mut self, target: &str) {
        self.branch("beqz", "a0", target);
    }

    fn branch_if_nonzero(&mut self, target: &str) {
        self.branch("bnez", "a0", target);
    }

    /// A fresh label name, `.L.<what>.<n>`.
    fn new_label(&mut self, what: &str) -> String {
        self.file.labels += 1;
        format!(".L.{what}.{}", self.file.labels)
    }

    /// `rd = rs + imm` for any `imm`; large ones go through `t0`.
    fn add_imm(&mut self, rd: &str, rs: &str, imm: i32) {
        if fits_imm12(imm) {
            self.ins("addi", format!("{rd}, {rs}, {imm}"));
        } else {
            self.li("t0", imm);
            self.ins("add", format!("{rd}, {rs}, t0"));
        }
    }

    /// A load or store (`op`) of `reg` at `offset(s0)`. Offsets beyond 12 bits
    /// (frames over 2 KiB) compute the address in `t0` first.
    fn access_local(&mut self, op: &str, reg: &str, offset: i32) {
        if fits_imm12(offset) {
            self.ins(op, format!("{reg}, {offset}(s0)"));
        } else {
            self.add_imm("t0", "s0", offset);
            self.ins(op, format!("{reg}, 0(t0)"));
        }
    }

    fn offset(&self, id: LocalId) -> i32 {
        self.offsets[id.0]
    }

    // ---- the stack of temporaries ----

    /// Pushes `a0`.
    fn push(&mut self) {
        self.ins("addi", "sp, sp, -4");
        self.ins("sw", "a0, 0(sp)");
        self.depth += 1;
    }

    /// Pops the top temporary into `reg`.
    fn pop(&mut self, reg: &str) {
        self.ins("lw", format!("{reg}, 0(sp)"));
        self.ins("addi", "sp, sp, 4");
        self.depth -= 1;
    }

    // ---- functions ----

    fn function(&mut self, body: &'a [Stmt]) {
        let f = self.func;

        // A header that maps every local to its slot, for whoever reads the output.
        let mut params: Vec<String> = f.params.iter().map(|p| p.ty.declare(&p.name)).collect();
        if f.variadic {
            params.push("...".to_string());
        }
        let params = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };
        let frame = self.frame;
        self.comment(format!(
            "{}: frame {frame} bytes",
            f.ret.declare(&format!("{}({params})", f.name))
        ));
        if self.va_save > 0 {
            let regs = match self.named_words {
                7 => "a7".to_string(),
                n => format!("a{n}-a7"),
            };
            self.comment(format!("    {}(s0)  {regs}, saved for va_arg", -self.va_save));
        }
        for (i, l) in f.locals.iter().enumerate() {
            let off = self.offsets[i];
            let what = if i < f.params.len() {
                format!("param {}", l.ty.declare(&l.name))
            } else if l.temp {
                // A hidden local for a call or a return (sema's names say what).
                format!("temp {} ({})", l.ty, l.name)
            } else {
                format!("local {}", l.ty.declare(&l.name))
            };
            self.comment(format!("  {off:>5}(s0)  {what}"));
        }
        // An inline definition (C99 `inline` without `extern`) provides no
        // external definition: merlin keeps it local to the file, like a
        // `static` function.
        if !f.is_static && !f.inline_only {
            self.lines.push(Line::Text(format!(".globl {}", f.name)));
        }
        // A function symbol with its size, as clang marks them, so that
        // merlin-objdump, Grail's traces and tests can tell which function an
        // address is in (the `.size` follows the epilogue).
        self.lines
            .push(Line::Text(format!(".type {}, @function", f.name)));
        self.label(&f.name);

        // Prologue: save ra and the caller's s0, set up s0, make room for locals.
        let (top, va_save) = (self.top(), self.va_save);
        self.add_imm("sp", "sp", -top);
        self.ins("sw", format!("ra, {}(sp)", top - va_save - 4));
        self.ins("sw", format!("s0, {}(sp)", top - va_save - 8));
        self.add_imm("s0", "sp", top);
        if frame > top {
            self.add_imm("sp", "sp", -(frame - top));
        }
        // A variadic function stores the argument registers its named
        // parameters leave free right below s0, where the caller's stack
        // words continue them: every variadic argument is then in one run
        // of words from there up, which `va_arg` walks.
        if f.variadic && self.named_words < abi::ARG_REGS {
            self.comment("the variadic arguments in registers, next to those on the stack");
            for r in self.named_words..abi::ARG_REGS {
                let off = -4 * (abi::ARG_REGS - r) as i32;
                self.ins("sw", format!("a{r}, {off}(s0)"));
            }
        }
        // Parameters arrive in a0-a7 and then on the stack, from 0(s0) up
        // (see [`abi`]); give them their slots like any local. A big struct
        // result's address comes first, in a0.
        let Type::Func(ft) = f.ty() else {
            unreachable!("a function's type")
        };
        if let Some(ret_ptr) = f.ret_ptr {
            let off = self.offset(ret_ptr);
            self.access_local("sw", "a0", off);
        }
        let (words, _) = abi::arguments(&ft);
        for (i, (p, (word, pass))) in f.params.iter().zip(words).enumerate() {
            let off = self.offsets[i];
            match pass {
                Pass::Scalar => {
                    let reg = self.incoming(word);
                    self.access_local(store_op(&p.ty), &reg, off);
                }
                // A small struct: its words, as they lie in memory (the slot is
                // a whole number of words).
                Pass::Words(n) => {
                    for w in 0..n {
                        let reg = self.incoming(word + w as usize);
                        self.access_local("sw", &reg, off + 4 * w as i32);
                    }
                }
                // A big struct: the caller's copy, at the address in the
                // register, is copied into the slot. (Changing the caller's
                // copy in place would do too; a copy keeps every parameter
                // in the frame.)
                Pass::Ref => {
                    let src = match arg_word(word) {
                        ArgWord::Reg(r) => r,
                        // Every register argument is in its slot by now, so
                        // a0 is free.
                        ArgWord::Stack(at) => {
                            self.ins("lw", format!("a0, {at}(s0)"));
                            "a0".to_string()
                        }
                    };
                    self.add_imm("t4", "s0", off);
                    self.copy("t4", &src, &p.ty);
                }
            }
        }

        for s in body {
            self.stmt(s);
        }

        // Falling off the end returns 0. A final `return` needs no jump to the
        // epilogue, which comes right after it.
        if self.lines.last() == Some(&Line::Insn(insn_text("j", &self.ret_label), 4)) {
            self.lines.pop();
        } else {
            self.li("a0", 0);
        }

        // Epilogue: the result is in a0. Free the frame from s0 (whatever its
        // size), restore ra and s0, return.
        let ret = self.ret_label.clone();
        self.label(&ret);
        self.ins("addi", format!("sp, s0, {}", -top));
        self.ins("lw", format!("ra, {}(sp)", top - va_save - 4));
        self.ins("lw", format!("s0, {}(sp)", top - va_save - 8));
        self.ins("addi", format!("sp, sp, {top}"));
        self.ins("ret", "");
        self.lines.push(Line::Text(format!(".size {0}, . - {0}", f.name)));
    }

    /// The top of the frame, which the prologue sets up first: `ra`, `s0`
    /// and a variadic function's saved registers, rounded up to 16 bytes so
    /// that `sp` stays aligned at every step. 16 for most functions.
    fn top(&self) -> i32 {
        (SAVED_REGS + self.va_save + 15) & !15
    }

    /// In the prologue: the register holding argument word `word`. A word
    /// on the stack is loaded into `t1` first.
    fn incoming(&mut self, word: usize) -> String {
        match arg_word(word) {
            ArgWord::Reg(r) => r,
            ArgWord::Stack(at) => {
                self.ins("lw", format!("t1, {at}(s0)"));
                "t1".to_string()
            }
        }
    }

    // ---- statements ----

    fn stmt(&mut self, s: &'a Stmt) {
        debug_assert_eq!(self.depth, 0, "temporaries left on the stack between statements");
        match &s.kind {
            StmtKind::Block(body) => {
                for s in body {
                    self.stmt(s);
                }
            }
            StmtKind::Decl(decls) => {
                for d in decls {
                    self.local_decl(d);
                }
            }
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::Empty => {}
            StmtKind::If { cond, then, els } => {
                //     <cond>                    <cond>
                //     beqz a0, .L.end.N         beqz a0, .L.else.N
                //     <then>                    <then>
                //   .L.end.N:                   j    .L.end.N
                //                             .L.else.N:
                //                               <els>
                //                             .L.end.N:
                self.expr(cond);
                match els {
                    None => {
                        let end = self.new_label("end");
                        self.branch_if_zero(&end);
                        self.stmt(then);
                        self.label(&end);
                    }
                    Some(els) => {
                        let else_ = self.new_label("else");
                        let end = self.new_label("end");
                        self.branch_if_zero(&else_);
                        self.stmt(then);
                        self.ins("j", &end);
                        self.label(&else_);
                        self.stmt(els);
                        self.label(&end);
                    }
                }
            }
            StmtKind::While { cond, body } => {
                //   .L.continue.N:      (`continue` re-tests the condition)
                //     <cond>
                //     beqz a0, .L.break.N
                //     <body>
                //     j    .L.continue.N
                //   .L.break.N:
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.label(&cont);
                self.expr(cond);
                self.branch_if_zero(&brk);
                self.loop_body(body, &cont, &brk);
                self.ins("j", &cont);
                self.label(&brk);
            }
            StmtKind::DoWhile { body, cond } => {
                //   .L.do.N:
                //     <body>
                //   .L.continue.N:      (`continue` tests the condition)
                //     <cond>
                //     bnez a0, .L.do.N
                //   .L.break.N:
                let top = self.new_label("do");
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.label(&top);
                self.loop_body(body, &cont, &brk);
                self.label(&cont);
                self.expr(cond);
                self.branch_if_nonzero(&top);
                self.label(&brk);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                //     <init>
                //   .L.loop.N:
                //     <cond>            (no condition: loop forever)
                //     beqz a0, .L.break.N
                //     <body>
                //   .L.continue.N:      (`continue` still runs the step)
                //     <step>
                //     j    .L.loop.N
                //   .L.break.N:
                if let Some(init) = init {
                    self.stmt(init);
                }
                let top = self.new_label("loop");
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.label(&top);
                if let Some(cond) = cond {
                    self.expr(cond);
                    self.branch_if_zero(&brk);
                }
                self.loop_body(body, &cont, &brk);
                self.label(&cont);
                if let Some(step) = step {
                    self.expr(step);
                }
                self.ins("j", &top);
                self.label(&brk);
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                    if e.ty.is_record() {
                        self.return_record(&e.ty);
                    }
                }
                let ret = self.ret_label.clone();
                self.ins("j", ret);
            }
            StmtKind::Break => {
                let brk = self
                    .breaks
                    .last()
                    .expect("sema: break in a loop or switch")
                    .clone();
                self.ins("j", brk);
            }
            StmtKind::Continue => {
                let cont = self.continues.last().expect("sema: continue in a loop").clone();
                self.ins("j", cont);
            }
            StmtKind::Switch {
                cond,
                body,
                cases,
                default,
            } => {
                // A chain of compares, one per `case`, in source order; then
                // `default` (or the end, when there is none). The body is
                // emitted as it is, its `case` and `default` labels where they
                // stand, so control falls through from one to the next as C
                // says:
                //
                //     <cond>                   (promoted: a 32-bit value)
                //     li   t0, 1
                //     beq  a0, t0, .L.case.f.0
                //     li   t0, 7
                //     beq  a0, t0, .L.case.f.1
                //     j    .L.case.f.2         (default; or .L.break.N)
                //     <body>                   (with .L.case.f.<id>: labels)
                //   .L.break.N:
                let brk = self.new_label("break");
                self.expr(cond);
                for &(value, id) in cases {
                    // The bit pattern: an `unsigned` 0xffffffff is `li t0, -1`.
                    self.li("t0", IntKind::Int.wrap(value) as i32);
                    let target = self.case_label(id);
                    self.branch("beq", "a0, t0", &target);
                }
                let otherwise = match default {
                    Some(id) => self.case_label(*id),
                    None => brk.clone(),
                };
                self.ins("j", otherwise);
                self.breaks.push(brk.clone());
                self.stmt(body);
                self.breaks.pop();
                self.label(&brk);
            }
            StmtKind::Case { id, body, .. } | StmtKind::Default { id, body } => {
                let label = self.case_label(*id);
                self.label(&label);
                self.stmt(body);
            }
            StmtKind::Label { name, body } => {
                let label = self.goto_label(name);
                self.label(&label);
                self.stmt(body);
            }
            StmtKind::Goto(name) => {
                let label = self.goto_label(name);
                self.ins("j", label);
            }
        }
    }

    fn loop_body(&mut self, body: &'a Stmt, cont: &str, brk: &str) {
        self.continues.push(cont.to_string());
        self.breaks.push(brk.to_string());
        self.stmt(body);
        self.continues.pop();
        self.breaks.pop();
    }

    /// The label of `case` or `default` number `id` (numbered by sema, per
    /// function).
    fn case_label(&self, id: usize) -> String {
        format!(".L.case.{}.{id}", self.func.name)
    }

    /// The label of the C label `name`: `.L.label.<function>.<name>`.
    fn goto_label(&self, name: &str) -> String {
        format!(".L.label.{}.{name}", self.func.name)
    }

    /// Returns the struct whose address is in `a0` from the function: in
    /// `a0` (and `a1`) when it is small, otherwise copied to where the caller
    /// asked, the address that came in `a0` (see [`abi`]).
    fn return_record(&mut self, ty: &Type) {
        match self.func.ret_ptr {
            Some(ret_ptr) => {
                let off = self.offset(ret_ptr);
                self.access_local("lw", "a1", off);
                self.copy("a1", "a0", ty);
            }
            None => {
                let Pass::Words(n) = abi::classify(ty) else {
                    unreachable!("a big result has a result address")
                };
                self.ins("mv", "t1, a0");
                self.load_words("t1", 0, n, ty);
            }
        }
    }

    fn local_decl(&mut self, d: &'a Declarator) {
        let id = match &d.var {
            // A static local is data, emitted after the function.
            Some(VarRef::Global(_)) => return self.statics.push(d),
            Some(VarRef::Local(id)) => *id,
            None => unreachable!("sema resolves every declarator"),
        };
        // Without an initializer the local is left as it is (C: indeterminate).
        let Some(init) = &d.init else { return };
        let off = self.offset(id);
        // An array, or a struct given member by member: clear the whole
        // object, then store what the initializer gives.
        if d.ty.is_array() || (d.ty.is_record() && matches!(init, Init::List(..))) {
            self.comment(format!("{} = ...", d.ty.declare(&d.name)));
            self.clear_local(off, d.ty.size().unwrap() as i32);
        }
        self.init_local(&d.ty, init, off);
    }

    /// Stores an initializer into the local object at `off(s0)`; arrays and
    /// structs initialized by a list have been cleared, so what the list
    /// leaves out is zero already.
    fn init_local(&mut self, ty: &Type, init: &'a Init, off: i32) {
        match (ty.unqual(), init) {
            // Zero: cleared already. (In a union, every item but the last
            // is one of these.)
            (_, Init::List(items, _)) if items.is_empty() => {}
            (Type::Array(elem, _), Init::List(items, _)) => {
                let size = elem.size().unwrap() as i32;
                for (i, item) in items.iter().enumerate() {
                    self.init_local(elem, item, off + i as i32 * size);
                }
            }
            (Type::Record(r), Init::List(items, _)) => {
                // Sema's list is fully braced: one item per member, in order.
                let body = r.body().expect("sema: locals have complete types");
                for (m, item) in body.members.iter().zip(items) {
                    self.init_local(&m.ty, item, off + m.offset as i32);
                }
            }
            (Type::Record(_), Init::Expr(e)) => {
                // A struct initialized by a struct: copy it.
                self.expr(e);
                self.add_imm("a1", "s0", off);
                self.copy("a1", "a0", ty);
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    ..
                }),
            ) => {
                for (i, &b) in bytes.iter().take(*n as usize).enumerate() {
                    if b != 0 {
                        self.li("a0", b.into());
                        self.access_local("sb", "a0", off + i as i32);
                    }
                }
            }
            (_, Init::Expr(e)) => {
                self.expr(e);
                self.access_local(store_op(ty), "a0", off);
            }
            _ => unreachable!("sema checked initializer shapes"),
        }
    }

    /// Clears `size` bytes at `off(s0)` (word-aligned, see [`layout`]): word
    /// stores, or a loop for more than 16 words, then the last bytes.
    fn clear_local(&mut self, off: i32, size: i32) {
        let words = size / 4;
        if words > 16 {
            self.add_imm("t1", "s0", off); //  t1 = start
            self.li("t0", words * 4); //       t0 = end
            self.ins("add", "t0, t1, t0");
            self.label("2");
            self.ins("sw", "zero, 0(t1)");
            self.ins("addi", "t1, t1, 4");
            self.ins("bltu", "t1, t0, 2b");
        } else {
            for w in 0..words {
                self.access_local("sw", "zero", off + 4 * w);
            }
        }
        for b in words * 4..size {
            self.access_local("sb", "zero", off + b);
        }
    }

    // ---- expressions: each leaves its value in a0 ----

    fn expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Num(v) => {
                // The bit pattern: an `unsigned int` 0xffffffff is `li a0, -1`.
                self.li("a0", IntKind::Int.wrap(*v) as i32);
            }
            ExprKind::Str(bytes) => {
                let label = self.file.string(bytes);
                self.la("a0", &label);
            }
            ExprKind::Var { var, .. } => match var {
                Some(VarRef::Local(id)) => {
                    let off = self.offset(*id);
                    if e.ty.is_addressed() {
                        self.add_imm("a0", "s0", off); // an array or struct: its address
                    } else {
                        self.access_local(load_op(&e.ty), "a0", off);
                    }
                }
                Some(VarRef::Global(sym)) => {
                    self.la("a0", sym);
                    self.load(&e.ty);
                }
                None => unreachable!("sema resolves every variable"),
            },
            ExprKind::Assign {
                op: None,
                target,
                value,
            } if target.ty.is_record() => {
                // Struct assignment copies the bytes; its value is the target.
                self.addr(target);
                self.push();
                self.expr(value); // a0 = the source's address
                self.pop("a1");
                self.copy("a1", "a0", &target.ty);
                self.ins("mv", "a0, a1");
            }
            ExprKind::Assign {
                op: None,
                target,
                value,
            } => {
                let place = self.place(target);
                self.expr(value); // already converted to the target's type by sema
                self.store_place(place, &target.ty);
            }
            ExprKind::Assign {
                op: Some(op),
                target,
                value,
            } => {
                let place = self.place(target);
                self.load_place(&place, &target.ty);
                self.push();
                self.expr(value);
                self.pop("a1"); // a1 = the target's value, a0 = value
                let (t, v) = (target.ty.decay(), value.ty.decay());
                self.binary(*op, &t, &v);
                let result = arith_type(*op, &t, &v).expect("sema checked the operands");
                self.convert(&result, &target.ty);
                self.store_place(place, &target.ty);
            }
            ExprKind::IncDec { inc, prefix, target } => {
                let ty = &target.ty;
                let step = match ty.unqual() {
                    Type::Ptr(to) => to.size().expect("sema: pointer to a complete type") as i32,
                    _ => 1,
                };
                let delta = if *inc { step } else { -step };
                let place = self.place(target);
                self.load_place(&place, ty);
                if ty.int_kind() == Some(IntKind::Bool) {
                    // `b++` sets a `_Bool`, `b--` flips it: the old value
                    // cannot be worked out from the new one, so keep it.
                    self.ins("mv", "t1, a0");
                    self.add_imm("a0", "a0", delta);
                    self.wrap_int(ty);
                    self.store_place(place, ty);
                    if !prefix {
                        self.ins("mv", "a0, t1");
                    }
                    return;
                }
                self.add_imm("a0", "a0", delta);
                self.wrap_int(ty);
                self.store_place(place, ty);
                if !prefix {
                    // x++ gives the old value: the new one minus the step, wrapped again.
                    self.add_imm("a0", "a0", -delta);
                    self.wrap_int(ty);
                }
            }
            ExprKind::Binary(BinOp::LogAnd, l, r) => {
                //     <l>
                //     beqz a0, .L.and.N   # l is 0: so is the result, skip r
                //     <r>
                //   .L.and.N:
                //     snez a0, a0         # 0 or 1
                let end = self.new_label("and");
                self.expr(l);
                self.branch_if_zero(&end);
                self.expr(r);
                self.label(&end);
                self.ins("snez", "a0, a0");
            }
            ExprKind::Binary(BinOp::LogOr, l, r) => {
                // The mirror image: a nonzero l skips r and becomes 1.
                let end = self.new_label("or");
                self.expr(l);
                self.branch_if_nonzero(&end);
                self.expr(r);
                self.label(&end);
                self.ins("snez", "a0, a0");
            }
            ExprKind::Binary(op, l, r) => {
                self.expr(l);
                self.push();
                self.expr(r);
                self.pop("a1");
                // Now a1 = l, a0 = r.
                self.binary(*op, &l.ty.decay(), &r.ty.decay());
            }
            ExprKind::Unary(op, x) => {
                self.expr(x);
                match op {
                    UnOp::Neg => self.ins("neg", "a0, a0"),    // sub a0, zero, a0
                    UnOp::Plus => {}                           // promotion only: nothing to do
                    UnOp::Not => self.ins("seqz", "a0, a0"),   // a0 = (a0 == 0)
                    UnOp::BitNot => self.ins("not", "a0, a0"), // xori a0, a0, -1
                }
            }
            ExprKind::Addr(x) => self.addr(x),
            ExprKind::Deref(p) => {
                self.expr(p);
                self.load(&e.ty);
            }
            // A constant converted (`char c = 1;`): just the converted value.
            ExprKind::Cast { x, .. } if e.ty.is_integer() && matches!(x.kind, ExprKind::Num(_)) => {
                let Some(Const::Int(v, _)) = consteval::eval(e) else {
                    unreachable!("an integer literal cast to an integer type is constant")
                };
                self.li("a0", IntKind::Int.wrap(v) as i32);
            }
            ExprKind::Cast { x, .. } => {
                self.expr(x);
                self.convert(&x.ty, &e.ty);
            }
            ExprKind::SizeofType(_) | ExprKind::SizeofExpr(_) => unreachable!("sema folds sizeof"),
            ExprKind::Cond(c, a, b) => {
                //     <c>
                //     beqz a0, .L.else.N
                //     <a>
                //     j    .L.end.N
                //   .L.else.N:
                //     <b>
                //   .L.end.N:
                let else_ = self.new_label("else");
                let end = self.new_label("end");
                self.expr(c);
                self.branch_if_zero(&else_);
                self.expr(a);
                self.ins("j", &end);
                self.label(&else_);
                self.expr(b);
                self.label(&end);
            }
            ExprKind::Comma(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Member { .. } => {
                self.addr(e);
                self.load(&e.ty);
            }
            ExprKind::Call {
                callee,
                args,
                result,
                arg_copies,
            } => self.call(callee, args, *result, arg_copies),
            ExprKind::CompoundLit { init, local } => {
                self.compound_literal(&e.ty, init, *local);
                if !e.ty.is_addressed() {
                    self.load(&e.ty);
                }
            }
            ExprKind::VaStart { ap, .. } => self.va_start(ap),
            ExprKind::VaArg(ap) => self.va_arg(ap, &e.ty),
            ExprKind::VaEnd(_) | ExprKind::VaCopy(..) => unreachable!("sema rewrites them"),
        }
    }

    /// `(type){ ... }`: initializes its hidden local (cleared first, like a
    /// local initialized by a list) and leaves its address in `a0`.
    fn compound_literal(&mut self, ty: &Type, init: &'a Init, local: Option<LocalId>) {
        let off = self.offset(local.expect("sema gives compound literals a local"));
        self.comment(format!("compound literal ({ty})"));
        if ty.is_array() || ty.is_record() {
            self.clear_local(off, ty.size().expect("complete") as i32);
        }
        self.init_local(ty, init, off);
        self.add_imm("a0", "s0", off);
    }

    /// `va_start(ap, last)`: ap = the address of the first variadic
    /// argument, in the registers saved at the top of the frame, or else
    /// among the caller's stack words, past the named ones.
    fn va_start(&mut self, ap: &'a Expr) {
        let place = self.place(ap);
        let first = match arg_word(self.named_words) {
            ArgWord::Reg(_) => -self.va_save,
            ArgWord::Stack(at) => at,
        };
        self.add_imm("a0", "s0", first);
        self.store_place(place, &ap.ty);
    }

    /// `va_arg(ap, ty)`: the argument is where ap points, and ap moves past
    /// its words. A scalar is loaded from there; a small struct's value is
    /// its address, there; a big one's address is the word there.
    fn va_arg(&mut self, ap: &'a Expr, ty: &Type) {
        let pass = abi::classify(ty);
        let place = self.place(ap);
        self.load_place(&place, &ap.ty);
        self.ins("mv", "t1, a0");
        self.add_imm("a0", "a0", 4 * pass.registers() as i32);
        self.store_place(place, &ap.ty);
        match pass {
            Pass::Scalar => self.ins(load_op(ty), "a0, 0(t1)"),
            Pass::Words(_) => self.ins("mv", "a0, t1"),
            Pass::Ref => self.ins("lw", "a0, 0(t1)"),
        }
    }

    /// Loads the value of type `ty` at the address in `a0`. Arrays, structs
    /// and functions are not loaded: their value is that address.
    fn load(&mut self, ty: &Type) {
        if !ty.is_addressed() {
            self.ins(load_op(ty), "a0, 0(a0)");
        }
    }

    /// Leaves the address of the lvalue `e` in `a0`.
    fn addr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Var { var, .. } => match var {
                Some(VarRef::Local(id)) => {
                    let off = self.offset(*id);
                    self.add_imm("a0", "s0", off);
                }
                Some(VarRef::Global(sym)) => self.la("a0", sym),
                None => unreachable!("sema resolves every variable"),
            },
            // The address of *p is p.
            ExprKind::Deref(p) => self.expr(p),
            ExprKind::Str(_) => self.expr(e),
            ExprKind::CompoundLit { init, local } => self.compound_literal(&e.ty, init, *local),
            // A member is at its offset from the start of the struct. The
            // struct's value is its address (also when it is not an lvalue,
            // like a call's result, kept in a hidden local), and `p->m`'s `p`
            // is one too.
            ExprKind::Member { base, offset, .. } => {
                self.expr(base);
                if *offset != 0 {
                    self.add_imm("a0", "a0", *offset as i32);
                }
            }
            _ => unreachable!("sema only takes the address of lvalues"),
        }
    }

    /// Prepares to store to `e`; see [`Place`].
    fn place(&mut self, e: &'a Expr) -> Place {
        if let ExprKind::Var {
            var: Some(VarRef::Local(id)),
            ..
        } = &e.kind
        {
            return Place::Frame(self.offset(*id));
        }
        self.addr(e);
        self.push();
        Place::Pushed
    }

    /// Loads the value at a place into `a0`; a pushed address stays pushed.
    fn load_place(&mut self, place: &Place, ty: &Type) {
        match place {
            Place::Frame(off) => self.access_local(load_op(ty), "a0", *off),
            Place::Pushed => {
                self.ins("lw", "a0, 0(sp)"); // the address, left on the stack
                self.load(ty);
            }
        }
    }

    /// Stores `a0` to a place, popping a pushed address.
    fn store_place(&mut self, place: Place, ty: &Type) {
        match place {
            Place::Frame(off) => self.access_local(store_op(ty), "a0", off),
            Place::Pushed => {
                self.pop("a1");
                self.ins(store_op(ty), "a0, 0(a1)");
            }
        }
    }

    /// Converts the value in `a0` from type `from` to type `to`. Only a
    /// narrowing to `char` or `short` changes bits: everything else is 32 bits
    /// already, and an array's value is its address, already a pointer.
    fn convert(&mut self, from: &Type, to: &Type) {
        let (from, to) = (from.unqual(), to.unqual());
        if to.int_kind() == Some(IntKind::Bool) {
            // To `_Bool`: 0 stays 0, anything else (a pointer too) becomes 1.
            if from != to {
                self.ins("snez", "a0, a0");
            }
            return;
        }
        if let (Type::Int(f), Type::Int(t)) = (from, to) {
            // A narrower or equal type whose values all fit is already right.
            let fits = (f.size() < t.size() && (t.signed() || !f.signed()))
                || (f.size() == t.size() && f.signed() == t.signed());
            if fits {
                return;
            }
        }
        self.wrap_int(to);
    }

    /// If `ty` is narrower than 32 bits, cuts `a0` down to it and widens it
    /// back by its signedness.
    fn wrap_int(&mut self, ty: &Type) {
        let Some(k) = ty.int_kind() else { return };
        let shift = 32 - 8 * k.size();
        match (k.size(), k.signed()) {
            _ if k == IntKind::Bool => self.ins("snez", "a0, a0"),
            (4, _) => {}
            (1, false) => self.ins("andi", "a0, a0, 255"),
            (_, signed) => {
                self.ins("slli", format!("a0, a0, {shift}"));
                let right = if signed { "srai" } else { "srli" };
                self.ins(right, format!("a0, a0, {shift}"));
            }
        }
    }

    /// Multiplies `reg` by `size`: the scaling of pointer arithmetic.
    fn scale(&mut self, reg: &str, size: u32) {
        match log2(size) {
            Some(0) => {}
            Some(n) => self.ins("slli", format!("{reg}, {reg}, {n}")),
            None => {
                self.li("t0", size as i32);
                self.ins("mul", format!("{reg}, {reg}, t0"));
            }
        }
    }

    /// `a0 = a1 op a0` for the operators that always evaluate both sides, with
    /// `l` and `r` the (decayed) types of a1 and a0.
    fn binary(&mut self, op: BinOp, l: &Type, r: &Type) {
        // Comparisons, division and remainder follow the common type's
        // signedness; a pointer compares as an unsigned address.
        let unsigned = l.is_unsigned() || r.is_unsigned();
        let pick = |s: &'static str, u: &'static str| if unsigned { u } else { s };
        let elem_size = |t: &Type| t.pointee().and_then(Type::size).expect("sema: complete pointee");
        match op {
            BinOp::Add => {
                // A pointer plus an integer moves by whole elements.
                if l.is_ptr() {
                    self.scale("a0", elem_size(l));
                } else if r.is_ptr() {
                    self.scale("a1", elem_size(r));
                }
                self.ins("add", "a0, a1, a0");
            }
            BinOp::Sub if l.is_ptr() && r.is_ptr() => {
                // The difference of two pointers counts elements.
                self.ins("sub", "a0, a1, a0");
                match log2(elem_size(l)) {
                    Some(0) => {}
                    Some(n) => self.ins("srai", format!("a0, a0, {n}")),
                    None => {
                        self.li("t0", elem_size(l) as i32);
                        self.ins("div", "a0, a0, t0");
                    }
                }
            }
            BinOp::Sub => {
                if l.is_ptr() {
                    self.scale("a0", elem_size(l));
                }
                self.ins("sub", "a0, a1, a0");
            }
            BinOp::Mul => self.ins("mul", "a0, a1, a0"),
            BinOp::Div => self.ins(pick("div", "divu"), "a0, a1, a0"),
            BinOp::Rem => self.ins(pick("rem", "remu"), "a0, a1, a0"),
            BinOp::Shl => self.ins("sll", "a0, a1, a0"),
            // Only the left operand's type matters for a shift.
            BinOp::Shr => {
                let op = if l.is_unsigned() { "srl" } else { "sra" };
                self.ins(op, "a0, a1, a0");
            }
            BinOp::BitAnd => self.ins("and", "a0, a1, a0"),
            BinOp::BitOr => self.ins("or", "a0, a1, a0"),
            BinOp::BitXor => self.ins("xor", "a0, a1, a0"),
            BinOp::Lt => self.ins(pick("slt", "sltu"), "a0, a1, a0"), // l < r
            BinOp::Gt => self.ins(pick("slt", "sltu"), "a0, a0, a1"), // r < l
            BinOp::Le => {
                self.ins(pick("slt", "sltu"), "a0, a0, a1"); // l <= r is !(r < l)
                self.ins("xori", "a0, a0, 1");
            }
            BinOp::Ge => {
                self.ins(pick("slt", "sltu"), "a0, a1, a0"); // l >= r is !(l < r)
                self.ins("xori", "a0, a0, 1");
            }
            BinOp::Eq => {
                self.ins("xor", "a0, a1, a0"); // 0 exactly when l == r
                self.ins("seqz", "a0, a0");
            }
            BinOp::Ne => {
                self.ins("xor", "a0, a1, a0");
                self.ins("snez", "a0, a0");
            }
            BinOp::LogAnd | BinOp::LogOr => unreachable!("handled in expr: they short-circuit"),
        }
    }

    /// A call: `f(args)` with `f` a function's name (`call f`), or through any
    /// other expression of function or pointer-to-function type (`jalr`).
    /// Arguments and the result travel as [`abi`] says.
    fn call(
        &mut self,
        callee: &'a Expr,
        args: &'a [Expr],
        result: Option<LocalId>,
        copies: &[Option<LocalId>],
    ) {
        let ft: FuncType = callee
            .ty
            .decay()
            .pointee()
            .and_then(Type::func_type)
            .expect("sema: the callee is a function")
            .clone();
        let layout = ArgLayout::new(&ft, args);
        let (types, regs, used) = (&layout.types, &layout.words, layout.used);
        let direct = callee.direct_callee();
        if used > abi::ARG_REGS {
            let hidden = if abi::returns_in_memory(&ft.ret) {
                result
            } else {
                None
            };
            self.call_with_stack(callee, args, copies, hidden, &layout);
            self.call_result(&ft, result);
            return;
        }
        // Through a pointer: the function's address is evaluated first and
        // waits on the stack, under the arguments, until they are loaded.
        if direct.is_none() {
            self.expr(callee);
            if args.is_empty() {
                self.ins("mv", "t1, a0");
            } else {
                self.push();
            }
        }
        // Evaluate every argument before loading any argument register: a later
        // argument may itself contain a call, which would clobber a0-a7. So all
        // but the last are pushed; the last one, already in a0, goes straight
        // to its register(s), and then the others are popped into theirs. A
        // struct's value is its address: its words are loaded from there.
        if let Some(last) = args.len().checked_sub(1) {
            for i in 0..last {
                self.argument(&args[i], copies[i]);
                self.push();
            }
            self.argument(&args[last], copies[last]);
            match regs[last] {
                (0, Pass::Scalar | Pass::Ref) => {}
                (r, Pass::Scalar | Pass::Ref) => self.ins("mv", format!("a{r}, a0")),
                (r, Pass::Words(n)) => {
                    self.ins("mv", "t1, a0");
                    self.load_words("t1", r, n, &types[last]);
                }
            }
            for i in (0..last).rev() {
                match regs[i] {
                    (r, Pass::Scalar | Pass::Ref) => self.pop(&format!("a{r}")),
                    (r, Pass::Words(n)) => {
                        self.pop("t1");
                        self.load_words("t1", r, n, &types[i]);
                    }
                }
            }
            if direct.is_none() {
                self.pop("t1");
            }
        }
        // A big struct result goes to the hidden local: its address in a0.
        if abi::returns_in_memory(&ft.ret) {
            let off = self.offset(result.expect("sema: a struct result has a local"));
            self.add_imm("a0", "s0", off);
        }
        // Temporaries of an enclosing expression may still be pushed (in
        // `x + f(y)`, x is). The convention wants sp 16-byte aligned at the
        // call, so pad the stack to the next multiple of 16 around it.
        let pad = (16 - (self.depth as i32 * 4) % 16) % 16;
        if pad != 0 {
            self.ins("addi", format!("sp, sp, {}", -pad));
        }
        self.jump_to(direct);
        if pad != 0 {
            self.ins("addi", format!("sp, sp, {pad}"));
        }
        self.call_result(&ft, result);
    }

    /// The call instruction: `call f`, or `jalr t1` through a pointer.
    fn jump_to(&mut self, direct: Option<&str>) {
        match direct {
            // `call` is `auipc ra` + `jalr ra`: two words.
            Some(name) => self.ins_sized(8, "call", name),
            None => self.ins("jalr", "t1"),
        }
    }

    /// After a call: a struct result is kept in its hidden local (a small
    /// one arrives in a0/a1 and is stored there); the call's value is that
    /// address.
    fn call_result(&mut self, ft: &FuncType, result: Option<LocalId>) {
        if let Some(result) = result {
            let off = self.offset(result);
            if let Pass::Words(n) = abi::classify(&ft.ret) {
                for w in 0..n {
                    self.access_local("sw", &format!("a{w}"), off + 4 * w as i32);
                }
            }
            self.add_imm("a0", "s0", off);
        }
    }

    /// A call whose arguments take more than `a0-a7` (`used` words, placed
    /// as `words` says): every argument is evaluated and pushed, left to
    /// right; then room is made below them for the stack words, keeping
    /// `sp` 16-byte aligned at the call, and each word is copied to its
    /// place, on the stack or in a register:
    ///
    /// ```text
    ///     <arg 0>  push ... <arg n-1>  push
    ///     addi sp, sp, -area           # the outgoing words: 0(sp), 4(sp), ...
    ///     lw   t1, <arg 8>(sp)         # a stack word
    ///     sw   t1, 0(sp)
    ///     lw   a0, <arg 0>(sp)         # a register
    ///     ...
    ///     call f
    ///     addi sp, sp, area + 4n       # the words and the pushed arguments
    /// ```
    fn call_with_stack(
        &mut self,
        callee: &'a Expr,
        args: &'a [Expr],
        copies: &[Option<LocalId>],
        // The hidden local for a big struct result, if any.
        result: Option<LocalId>,
        layout: &ArgLayout,
    ) {
        let (types, words, used) = (&layout.types, &layout.words, layout.used);
        let direct = callee.direct_callee();
        if direct.is_none() {
            self.expr(callee);
            self.push();
        }
        for (i, a) in args.iter().enumerate() {
            self.argument(a, copies[i]);
            self.push();
        }
        let n = args.len() as i32;
        let pushed = 4 * self.depth as i32;
        let stack = 4 * abi::stack_words(used) as i32;
        let area = (pushed + stack + 15) / 16 * 16 - pushed;
        self.comment(match abi::stack_words(used) {
            1 => "1 argument word goes on the stack".to_string(),
            n => format!("{n} argument words go on the stack"),
        });
        self.add_imm("sp", "sp", -area);
        // Where argument i was pushed: the last one is right above the area.
        let temp = |i: usize| area + 4 * (n - 1 - i as i32);
        // The stack words first (with t1, t2 and t0), then the registers.
        for (i, &(word, pass)) in words.iter().enumerate() {
            match pass {
                Pass::Scalar | Pass::Ref => {
                    if let ArgWord::Stack(at) = arg_word(word) {
                        self.ins("lw", format!("t1, {}(sp)", temp(i)));
                        self.ins("sw", format!("t1, {at}(sp)"));
                    }
                }
                Pass::Words(nw) => {
                    for w in 0..nw {
                        if let ArgWord::Stack(at) = arg_word(word + w as usize) {
                            self.ins("lw", format!("t1, {}(sp)", temp(i)));
                            self.load_word("t2", "t1", w, &types[i]);
                            self.ins("sw", format!("t2, {at}(sp)"));
                        }
                    }
                }
            }
        }
        for (i, &(word, pass)) in words.iter().enumerate() {
            match pass {
                Pass::Scalar | Pass::Ref => {
                    if let ArgWord::Reg(r) = arg_word(word) {
                        self.ins("lw", format!("{r}, {}(sp)", temp(i)));
                    }
                }
                Pass::Words(nw) => {
                    if word < abi::ARG_REGS {
                        self.ins("lw", format!("t1, {}(sp)", temp(i)));
                    }
                    for w in 0..nw {
                        if let ArgWord::Reg(r) = arg_word(word + w as usize) {
                            self.load_word(&r, "t1", w, &types[i]);
                        }
                    }
                }
            }
        }
        // A big struct result goes to the hidden local: its address in a0.
        if let Some(result) = result {
            let off = self.offset(result);
            self.add_imm("a0", "s0", off);
        }
        if direct.is_none() {
            self.ins("lw", format!("t1, {}(sp)", area + 4 * n));
        }
        self.jump_to(direct);
        let popped = n + i32::from(direct.is_none());
        self.add_imm("sp", "sp", area + 4 * popped);
        self.depth -= popped as u32;
    }

    /// Evaluates an argument into `a0`: a scalar's value, or a struct's
    /// address. A struct passed by reference is first copied to its hidden
    /// local (`copy`), and that copy's address is what is passed.
    fn argument(&mut self, arg: &'a Expr, copy: Option<LocalId>) {
        self.expr(arg);
        if let Some(copy) = copy {
            let off = self.offset(copy);
            self.add_imm("a1", "s0", off);
            self.copy("a1", "a0", &arg.ty);
            self.ins("mv", "a0, a1");
        }
    }

    /// Loads the `n` words of the struct of type `ty` at the address in
    /// `src` into `a<first>`, `a<first+1>`: how a small struct is passed
    /// and returned.
    fn load_words(&mut self, src: &str, first: usize, n: u32, ty: &Type) {
        for w in 0..n {
            self.load_word(&format!("a{}", first + w as usize), src, w, ty);
        }
    }

    /// Loads word `w` of the struct of type `ty` at the address in `src` into
    /// `reg`. A struct aligned to less than 4 bytes may not be at a word
    /// boundary, so its word is put together from aligned bytes or halves
    /// (in `t0`); bytes past the struct's end read as zero.
    fn load_word(&mut self, reg: &str, src: &str, w: u32, ty: &Type) {
        let size = ty.size().expect("a complete struct");
        let unit = ty.align().min(4);
        if unit == 4 {
            self.ins("lw", format!("{reg}, {}({src})", 4 * w));
            return;
        }
        let load = if unit == 1 { "lbu" } else { "lhu" };
        for k in (0..4).step_by(unit as usize) {
            let at = 4 * w + k;
            if at >= size {
                break;
            }
            if k == 0 {
                self.ins(load, format!("{reg}, {at}({src})"));
            } else {
                self.ins(load, format!("t0, {at}({src})"));
                self.ins("slli", format!("t0, t0, {}", 8 * k));
                self.ins("or", format!("{reg}, {reg}, t0"));
            }
        }
    }

    /// Copies the struct of type `ty` from the address in `src` to the one in
    /// `dst` (neither of them `t0-t3`, both left as they are). It goes in
    /// units of the struct's alignment (at most a word), so no access is
    /// misaligned: unrolled up to 16 units, a loop beyond. Clobbers `t0`, and
    /// `t1-t3` for the loop.
    fn copy(&mut self, dst: &str, src: &str, ty: &Type) {
        let size = ty.size().expect("sema: a complete struct");
        let unit = ty.align().min(4);
        let (load, store) = match unit {
            1 => ("lbu", "sb"),
            2 => ("lhu", "sh"),
            _ => ("lw", "sw"),
        };
        if size / unit <= 16 {
            for at in (0..size).step_by(unit as usize) {
                self.ins(load, format!("t0, {at}({src})"));
                self.ins(store, format!("t0, {at}({dst})"));
            }
        } else {
            self.ins("mv", format!("t1, {src}")); // t1 = from
            self.ins("mv", format!("t2, {dst}")); // t2 = to
            self.li("t3", size as i32); //            t3 = end of from
            self.ins("add", "t3, t1, t3");
            self.label("2");
            self.ins(load, "t0, 0(t1)");
            self.ins(store, "t0, 0(t2)");
            self.ins("addi", format!("t1, t1, {unit}"));
            self.ins("addi", format!("t2, t2, {unit}"));
            self.ins("bltu", "t1, t3, 2b");
        }
    }
}

/// `mnemonic operands`, with the operands lined up in a column.
fn insn_text(mnemonic: &str, operands: &str) -> String {
    if operands.is_empty() {
        mnemonic.to_string()
    } else {
        format!("{mnemonic:<5} {operands}")
    }
}

/// B-type branches reach -4096..=+4094 bytes from themselves.
const BRANCH_REACH: std::ops::RangeInclusive<i64> = -4096..=4094;

/// Decides which conditional branches need the long form (the opposite branch
/// over a `j`). Returns one flag per line, true for a branch that does.
///
/// Addresses are estimated with every pseudo-instruction at its largest
/// (`li` as two words, say), so a distance is never underestimated; a branch
/// that is safe here is safe in the assembled code. Making a branch long moves
/// code after it, which can push another branch out of reach, so this repeats
/// until nothing changes. It always ends: branches only ever become long.
fn relax_branches(lines: &[Line]) -> Vec<bool> {
    let mut long = vec![false; lines.len()];
    loop {
        // Where each line starts and where each label is, with the current choice.
        let mut addr = Vec::with_capacity(lines.len());
        let mut labels = HashMap::new();
        let mut pc: i64 = 0;
        for (i, line) in lines.iter().enumerate() {
            addr.push(pc);
            pc += match line {
                Line::Label(name) => {
                    labels.insert(name.as_str(), pc);
                    0
                }
                Line::Insn(_, size) => i64::from(*size),
                Line::Branch { .. } if long[i] => 8,
                Line::Branch { .. } => 4,
                Line::Text(_) => 0,
            };
        }
        let mut changed = false;
        for (i, line) in lines.iter().enumerate() {
            if let Line::Branch { target, .. } = line {
                let distance = labels[target.as_str()] - addr[i];
                if !long[i] && !BRANCH_REACH.contains(&distance) {
                    long[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return long;
        }
    }
}

/// Writes a function's lines as assembly text.
fn render(lines: &[Line], long: &[bool], out: &mut String) {
    for (line, &long) in lines.iter().zip(long) {
        match line {
            Line::Label(name) => {
                let _ = writeln!(out, "{name}:");
            }
            Line::Insn(text, _) => {
                let _ = writeln!(out, "    {text}");
            }
            Line::Text(text) => {
                let _ = writeln!(out, "    {text}");
            }
            Line::Branch { op, regs, target } => {
                if long {
                    // Too far for a branch: skip over a `j` when the condition fails.
                    let inverse = match *op {
                        "beqz" => "bnez",
                        "bnez" => "beqz",
                        "beq" => "bne",
                        "bne" => "beq",
                        _ => unreachable!("codegen emits no other branches"),
                    };
                    let _ = writeln!(out, "    {}", insn_text(inverse, &format!("{regs}, 1f")));
                    let _ = writeln!(out, "    {}", insn_text("j", target));
                    let _ = writeln!(out, "1:");
                } else {
                    let _ = writeln!(out, "    {}", insn_text(op, &format!("{regs}, {target}")));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> String {
        crate::compile(src).unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// The instructions of `f`, without comments, labels or indentation.
    fn body_of(asm: &str, f: &str) -> Vec<String> {
        asm.lines()
            .skip_while(|l| *l != format!("{f}:"))
            .skip(1)
            .take_while(|l| *l != format!(".L.return.{f}:"))
            .map(str::trim)
            .filter(|l| !l.starts_with('#') && !l.ends_with(':'))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// The instructions `main` runs for `body`, after the prologue.
    fn main_body(globals: &str, body: &str) -> String {
        let asm = compile(&format!("{globals}\nint main() {{ {body} }}"));
        body_of(&asm, "main")[4..].join("; ")
    }

    fn locals(decls: &str) -> (Vec<i32>, i32) {
        let p = crate::parse(&format!("int f() {{ {decls} }}")).unwrap();
        let f = p.funcs().next().unwrap();
        layout(&f.locals, SAVED_REGS).unwrap()
    }

    #[test]
    fn frame_layout() {
        // Only ints: local #i at -(12 + 4*i), as in v0.
        assert_eq!(locals("int a, b, c;"), (vec![-12, -16, -20], 32));
        assert_eq!(locals("int a, b;"), (vec![-12, -16], 16));
        assert_eq!(locals(""), (vec![], 16));
        // Each type at its own alignment, packed downwards.
        assert_eq!(
            locals("char c; short s; char d; int i;"),
            (vec![-9, -12, -13, -20], 32)
        );
        // Arrays are word-aligned.
        assert_eq!(locals("char c; char buf[5]; char d;"), (vec![-9, -16, -17], 32));
        assert_eq!(locals("int a[10];"), (vec![-48], 48));
    }

    #[test]
    fn frame_size_limit() {
        // Each array fits, but not both: an error, not a frame that wrapped around.
        let err = crate::compile("int main() { char a[2147483640]; char b[100]; return 0; }").unwrap_err();
        assert_eq!(
            err[0].to_string(),
            "1:5: the locals of 'main' need a stack frame larger than 2 GiB"
        );
        let p = crate::parse("int f() { char a[2147483600]; }").unwrap();
        assert_eq!(
            layout(&p.funcs().next().unwrap().locals, SAVED_REGS).unwrap().1,
            2147483616
        );
    }

    #[test]
    fn smallest_function() {
        let asm = compile("int main() { return 42; }");
        assert_eq!(
            asm,
            "    .text

    # int main(void): frame 16 bytes
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    li    a0, 42
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main
"
        );
        // Each function symbol carries its type and size, as clang's do: ten
        // instructions here, and the static one's too.
        let obj = merlin_as::assemble(&compile(
            "int main(void) { return 42; } static int f(void) { return 1; }",
        ))
        .unwrap();
        for name in ["main", "f"] {
            let s = obj.symbols.iter().find(|s| s.name == name).unwrap();
            assert_eq!((s.size, s.ty), (40, merlin_as::asm::SymType::Func), "{name}");
        }
    }

    #[test]
    fn variables_are_sized_objects() {
        // The kernel's tests find which buffer an address is in from the
        // symbol's size, in .bss, .data and .rodata alike, static or not.
        let src = "struct s { int a; char b; } bufs[3]; int n = 5; static const char k[7] = \"abc\";
                   int f(void) { static short count; return k[0] + count; }";
        let asm = compile(src);
        assert!(
            asm.contains("    .type bufs, @object\n    .size bufs, 24\nbufs:\n"),
            "{asm}"
        );
        let obj = merlin_as::assemble(&asm).unwrap();
        for (name, size) in [("bufs", 24), ("n", 4), ("k", 7), ("f.count", 2)] {
            let s = obj.symbols.iter().find(|s| s.name == name).unwrap();
            assert_eq!((s.size, s.ty), (size, merlin_as::asm::SymType::Object), "{name}");
        }
    }

    #[test]
    fn prototypes_and_externs_emit_nothing() {
        let asm = compile("int f(int a); extern int g; int main() { return f(g); }");
        assert!(!asm.contains("f:"), "{asm}");
        assert!(!asm.contains("g:"), "{asm}");
        assert!(asm.contains("call  f\n"), "{asm}");
        assert!(asm.contains("la    a0, g\n"), "{asm}");
        assert_eq!(asm.matches(".globl").count(), 1);
    }

    #[test]
    fn static_functions_are_not_global() {
        let asm = compile("static int f() { return 1; } int main() { return f(); }");
        assert!(!asm.contains(".globl f"), "{asm}");
        assert!(asm.contains("\nf:\n"), "{asm}");
    }

    #[test]
    fn falling_off_the_end_returns_zero() {
        let body = body_of(&compile("int main() { int x = 3; }"), "main");
        assert_eq!(body.last().unwrap(), "li a0, 0");
        // A final return jumps nowhere: the epilogue is next.
        let body = body_of(&compile("int main() { return 3; }"), "main");
        assert_eq!(body.last().unwrap(), "li a0, 3");
        // A return elsewhere jumps to it.
        let asm = compile("int main(int a) { if (a) return 1; return 2; }");
        assert!(asm.contains("j     .L.return.main\n"), "{asm}");
    }

    #[test]
    fn parameters_are_stored_in_their_slots() {
        let asm = compile("int f(int a, char b, short c) { return c; } int main() { return f(1, 2, 3); }");
        let body = body_of(&asm, "f");
        assert_eq!(
            body,
            [
                "addi sp, sp, -16",
                "sw ra, 12(sp)",
                "sw s0, 8(sp)",
                "addi s0, sp, 16",
                // 8 + 4 + 1 (+1 to align) + 2 = 16 bytes: the save area is the frame.
                "sw a0, -12(s0)",
                "sb a1, -13(s0)",
                "sh a2, -16(s0)",
                "lh a0, -16(s0)",
            ]
        );
        assert!(
            asm.contains("# int f(int a, char b, short c): frame 16 bytes\n"),
            "{asm}"
        );
        assert!(asm.contains("#     -13(s0)  param char b\n"), "{asm}");
    }

    #[test]
    fn binary_operators() {
        let op = |ty: &str, src: &str| {
            let body = body_of(
                &compile(&format!("int main({ty} x, {ty} y) {{ return {src}; }}")),
                "main",
            );
            // Skip the prologue (4 lines), the parameter stores (2), `lw` x, the
            // push (2) and `lw` y: what is left is the pop and the operator.
            body[10..]
                .join("; ")
                .replace("lw a1, 0(sp); addi sp, sp, 4; ", "")
        };
        assert_eq!(op("int", "x + y"), "add a0, a1, a0");
        assert_eq!(op("int", "x - y"), "sub a0, a1, a0");
        assert_eq!(op("int", "x * y"), "mul a0, a1, a0");
        assert_eq!(op("int", "x / y"), "div a0, a1, a0");
        assert_eq!(op("int", "x % y"), "rem a0, a1, a0");
        assert_eq!(op("int", "x << y"), "sll a0, a1, a0");
        assert_eq!(op("int", "x >> y"), "sra a0, a1, a0");
        assert_eq!(op("int", "x & y"), "and a0, a1, a0");
        assert_eq!(op("int", "x | y"), "or a0, a1, a0");
        assert_eq!(op("int", "x ^ y"), "xor a0, a1, a0");
        assert_eq!(op("int", "x < y"), "slt a0, a1, a0");
        assert_eq!(op("int", "x > y"), "slt a0, a0, a1");
        assert_eq!(op("int", "x <= y"), "slt a0, a0, a1; xori a0, a0, 1");
        assert_eq!(op("int", "x >= y"), "slt a0, a1, a0; xori a0, a0, 1");
        assert_eq!(op("int", "x == y"), "xor a0, a1, a0; seqz a0, a0");
        assert_eq!(op("int", "x != y"), "xor a0, a1, a0; snez a0, a0");
        // Unsigned operations.
        assert_eq!(op("unsigned", "x / y"), "divu a0, a1, a0");
        assert_eq!(op("unsigned", "x % y"), "remu a0, a1, a0");
        assert_eq!(op("unsigned", "x >> y"), "srl a0, a1, a0");
        assert_eq!(op("unsigned", "x < y"), "sltu a0, a1, a0");
        assert_eq!(op("unsigned long", "x >= y"), "sltu a0, a1, a0; xori a0, a0, 1");
        // unsigned char promotes to int: signed.
        let body = body_of(
            &compile("int main(unsigned char x, unsigned char y) { return x / y; }"),
            "main",
        );
        assert_eq!(body.last().unwrap(), "div a0, a1, a0");
        assert!(body.contains(&"lbu a0, -9(s0)".to_string()), "{body:?}");
    }

    #[test]
    fn pointer_arithmetic_scales() {
        let p = |decls: &str, e: &str| main_body(decls, &format!("return {e};"));
        // int *: 4 bytes, a shift by 2.
        assert!(p("int *p; int i;", "*(p + i)").ends_with("slli a0, a0, 2; add a0, a1, a0; lw a0, 0(a0)"));
        assert!(p("int *p; int i;", "*(i + p)").ends_with("slli a1, a1, 2; add a0, a1, a0; lw a0, 0(a0)"));
        assert!(p("char *p; int i;", "*(p - i)").ends_with("sub a0, a1, a0; lbu a0, 0(a0)"));
        assert!(p("short *p, *q;", "p - q").ends_with("sub a0, a1, a0; srai a0, a0, 1"));
        // 12-byte elements: multiply and divide.
        assert!(p("int m[4][3]; int i;", "m[i][0]").contains("li t0, 12; mul a0, a0, t0"));
    }

    #[test]
    fn narrow_types_load_and_store_by_size() {
        assert_eq!(
            main_body("", "signed char c = 1; unsigned short s = 2; c = s; return c;"),
            "li a0, 1; sb a0, -9(s0); li a0, 2; sh a0, -12(s0); lhu a0, -12(s0); \
             slli a0, a0, 24; srai a0, a0, 24; sb a0, -9(s0); lb a0, -9(s0)"
        );
        // Widening is free.
        assert!(main_body("", "unsigned char c = 1; int i = c; return i;")
            .contains("lbu a0, -9(s0); sw a0, -16(s0)"));
    }

    #[test]
    fn conversions() {
        let cast = |from: &str, to: &str| {
            let body = body_of(
                &compile(&format!("int main({from} x) {{ return ({to})x; }}")),
                "main",
            );
            body[6..].join("; ")
        };
        assert_eq!(cast("int", "char"), "andi a0, a0, 255");
        assert_eq!(cast("int", "unsigned char"), "andi a0, a0, 255");
        assert_eq!(cast("int", "signed char"), "slli a0, a0, 24; srai a0, a0, 24");
        assert_eq!(cast("int", "short"), "slli a0, a0, 16; srai a0, a0, 16");
        assert_eq!(cast("int", "unsigned short"), "slli a0, a0, 16; srli a0, a0, 16");
        assert_eq!(cast("int", "unsigned"), "");
        assert_eq!(cast("char", "short"), "");
        assert_eq!(cast("char", "unsigned char"), "");
        assert_eq!(cast("char", "signed char"), "slli a0, a0, 24; srai a0, a0, 24");
        assert_eq!(cast("signed char", "char"), "andi a0, a0, 255");
        assert_eq!(
            cast("short", "unsigned short"),
            "slli a0, a0, 16; srli a0, a0, 16"
        );
    }

    #[test]
    fn places() {
        // A local: straight to its slot.
        assert_eq!(
            main_body("", "int x; x = 5;"),
            "li a0, 5; sw a0, -12(s0); li a0, 0"
        );
        // A global: through its address.
        assert_eq!(
            main_body("int g;", "g = 5;"),
            "la a0, g; addi sp, sp, -4; sw a0, 0(sp); li a0, 5; lw a1, 0(sp); addi sp, sp, 4; sw a0, 0(a1); \
             li a0, 0"
        );
        // Compound assignment evaluates the target once and keeps its address.
        assert_eq!(
            main_body("char *p;", "*p += 300;"),
            "la a0, p; lw a0, 0(a0); addi sp, sp, -4; sw a0, 0(sp); lw a0, 0(sp); lbu a0, 0(a0); \
             addi sp, sp, -4; sw a0, 0(sp); li a0, 300; lw a1, 0(sp); addi sp, sp, 4; add a0, a1, a0; \
             andi a0, a0, 255; lw a1, 0(sp); addi sp, sp, 4; sb a0, 0(a1); li a0, 0"
        );
        // Increments step by the pointed-to size; postfix gives the old value back.
        assert_eq!(
            main_body("", "int *p; return p++ == 0;").split("; xor").next().unwrap(),
            "lw a0, -12(s0); addi a0, a0, 4; sw a0, -12(s0); addi a0, a0, -4; addi sp, sp, -4; sw a0, 0(sp); \
             li a0, 0; lw a1, 0(sp); addi sp, sp, 4"
        );
        assert_eq!(
            main_body("", "unsigned char c; return --c;"),
            "lbu a0, -9(s0); addi a0, a0, -1; andi a0, a0, 255; sb a0, -9(s0)"
        );
    }

    #[test]
    fn arrays_are_addresses() {
        assert_eq!(
            main_body("", "int a[2]; return a[1];"),
            "addi a0, s0, -16; addi sp, sp, -4; sw a0, 0(sp); li a0, 1; lw a1, 0(sp); \
             addi sp, sp, 4; slli a0, a0, 2; add a0, a1, a0; lw a0, 0(a0)"
        );
        // &a and a are the same address; a global array is `la`.
        assert!(main_body("int g[3];", "return &g == 0;").starts_with("la a0, g; addi sp"));
    }

    #[test]
    fn local_array_initializers() {
        assert_eq!(
            main_body("", "int a[3] = {7}; return a[0];").split("; addi a0, s0").next().unwrap(),
            "addi sp, sp, -16; sw zero, -20(s0); sw zero, -16(s0); sw zero, -12(s0); li a0, 7; sw a0, -20(s0)"
        );
        assert_eq!(
            main_body("", "char s[] = \"a\\0b\"; return s[2];")
                .split("; addi a0, s0")
                .next()
                .unwrap(),
            "sw zero, -12(s0); li a0, 97; sb a0, -12(s0); li a0, 98; sb a0, -10(s0)"
        );
        // Big arrays are cleared with a loop.
        let body = main_body("", "int a[100] = {1}; return a[99];");
        assert!(
            body.contains("addi t1, s0, -408; li t0, 400; add t0, t1, t0; sw zero, 0(t1); addi t1, t1, 4; bltu t1, t0, 2b"),
            "{body}"
        );
    }

    #[test]
    fn global_data() {
        let asm = compile(
            "int g = 5; char c = 'x'; short h[3] = {1, -1}; int *p = &g + 1; char s[] = \"hi\\n\";
             static unsigned u; char *str = \"a\\\"b\\\\\"; char raw[3] = {1, 2, 255}; int zeros[2] = {0};
             int main() { static int n = 3; return n; }",
        );
        let data = &asm;
        for want in [
            // No empty `.text` first: sections are switched as needed.
            "    .data\n\n    # int g: 4 bytes\n    .globl g\n    .balign 4\n    .type g, @object\n    .size g, 4\ng:\n    .word 5\n",
            "\n    # char c: 1 byte\n    .globl c\n    .type c, @object\n    .size c, 1\nc:\n    .byte 120\n",
            "\n    .balign 2\n    .type h, @object\n    .size h, 6\nh:\n    .half 1\n    .half -1\n    .zero 2\n",
            "p:\n    .word g+4\n",
            "s:\n    .ascii \"hi\\n\"\n    .zero 1\n",
            "    .bss\n\n    # static unsigned int u: 4 bytes\n    .balign 4\n    .type u, @object\n    .size u, 4\nu:\n    .zero 4\n",
            "str:\n    .word .L.str.0\n",
            "raw:\n    .byte 1\n    .byte 2\n    .byte 255\n",
            "zeros:\n    .word 0\n    .zero 4\n",
            "\n    # static int n: 4 bytes\n    .balign 4\n    .type main.n, @object\n    .size main.n, 4\nmain.n:\n    .word 3\n",
            "    .section .rodata\n\n.L.str.0:\n    .asciz \"a\\\"b\\\\\"\n",
        ] {
            assert!(data.contains(want), "missing {want:?} in\n{data}");
        }
        assert!(data.starts_with("    .data\n"), "{data}");
        assert!(!data.contains(".globl u"), "static");
        assert!(!data.contains(".globl main.n"), "static local");
        assert!(asm.contains("la    a0, main.n\n"), "{asm}");
    }

    #[test]
    fn string_literals() {
        let asm = compile("char *f() { return \"one\"; } int main() { return f()[0] + \"\\x01\\x02z\"[1]; }");
        assert!(asm.contains("la    a0, .L.str.0\n"), "{asm}");
        assert!(asm.ends_with(
            "    .section .rodata\n\n.L.str.0:\n    .asciz \"one\"\n\n.L.str.1:\n    .byte 1, 2\n    .asciz \"z\"\n"
        ), "{asm}");
    }

    #[test]
    fn byte_directive_forms() {
        assert_eq!(byte_directives(b"abc"), [".ascii \"abc\""]);
        assert_eq!(byte_directives(b"abc\0"), [".asciz \"abc\""]);
        assert_eq!(byte_directives(b"\0"), [".byte 0"]);
        assert_eq!(
            byte_directives(b"a\0b\0"),
            [".ascii \"a\"", ".byte 0", ".asciz \"b\""]
        );
        assert_eq!(byte_directives(&[200, 201]), [".byte 200, 201"]);
        assert_eq!(byte_directives(&[1; 20]).len(), 2, "16 per line");
    }

    #[test]
    fn calls_pad_the_stack_to_16_bytes() {
        // No temporaries pushed: no padding.
        let asm = compile("int f(int a) { return a; } int main() { return f(1); }");
        let body = body_of(&asm, "main");
        assert_eq!(&body[4..], ["li a0, 1", "call f"]);
        // `1 + f(2)`: 1 is pushed (4 bytes), so 12 more bytes keep sp aligned.
        let asm = compile("int f(int a) { return a; } int main() { return 1 + f(2); }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.contains("li a0, 2; addi sp, sp, -12; call f; addi sp, sp, 12; lw a1, 0(sp)"),
            "{body}"
        );
    }

    #[test]
    fn arguments_are_evaluated_before_loading_registers() {
        let asm = compile("int f(int a, int b) { return a - b; } int main() { return f(f(5, 1), f(2, 3)); }");
        let body = body_of(&asm, "main").join("; ");
        // Both inner calls happen before a0/a1 are loaded for the outer one.
        let outer = body.rfind("call f").unwrap();
        assert!(
            body[..outer].ends_with("call f; addi sp, sp, 12; mv a1, a0; lw a0, 0(sp); addi sp, sp, 4; "),
            "{body}"
        );
    }

    #[test]
    fn arguments_and_results_are_converted() {
        let asm = compile("char f(char c) { return c + 1; } int main() { return f(300); }");
        // The argument is cut to a char (a constant: at compile time), and so is the result.
        assert_eq!(body_of(&asm, "main")[4..].join("; "), "li a0, 44; call f");
        assert!(body_of(&asm, "f")
            .join("; ")
            .ends_with("add a0, a1, a0; andi a0, a0, 255"));
    }

    #[test]
    fn labels_are_unique_across_functions() {
        let asm = compile(
            "int f(int a) { while (a) a = a - 1; do a++; while (a < 3); return a ? 1 : 2; }
             int main() { int i; for (i = 0; i < 3; i = i + 1) if (i) continue; else break; return f(i); }",
        );
        let mut defined: Vec<&str> = asm.lines().filter(|l| l.ends_with(':')).collect();
        let n = defined.len();
        defined.sort();
        defined.dedup();
        assert_eq!(defined.len(), n, "{asm}");
        assert!(asm.contains("j     .L.continue."), "{asm}");
        assert!(asm.contains("j     .L.break."), "{asm}");
        assert!(asm.contains("bnez  a0, .L.do."), "{asm}");
    }

    #[test]
    fn short_circuit_operators() {
        let asm = compile("int main(int a, int b) { return a && b; }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.ends_with("lw a0, -12(s0); beqz a0, .L.and.1; lw a0, -16(s0); snez a0, a0"),
            "{body}"
        );
        let asm = compile("int main(int a, int b) { return a || b; }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.ends_with("lw a0, -12(s0); bnez a0, .L.or.1; lw a0, -16(s0); snez a0, a0"),
            "{body}"
        );
    }

    #[test]
    fn big_frames_use_t0() {
        let decls: String = (0..600).map(|i| format!("int v{i} = {i};")).collect();
        let asm = compile(&format!("int main() {{ {decls} return v599; }}"));
        // 8 + 600 * 4 = 2408 bytes, rounded up to 2416; 16 come from the save area.
        let body = body_of(&asm, "main").join("; ");
        assert!(body.contains("li t0, -2400; add sp, sp, t0"), "{body}");
        // v509 is at -12 - 4 * 509 = -2048, the last slot a 12-bit offset reaches.
        assert!(body.contains("li a0, 509; sw a0, -2048(s0)"), "{body}");
        assert!(
            body.contains("li a0, 510; li t0, -2052; add t0, s0, t0; sw a0, 0(t0)"),
            "{body}"
        );
        assert!(
            body.ends_with("li t0, -2408; add t0, s0, t0; lw a0, 0(t0)"),
            "{body}"
        );
    }

    #[test]
    fn far_branches_are_rewritten() {
        let lines = |n| {
            let mut v = vec![Line::Branch {
                op: "beqz",
                regs: "a0",
                target: "end".into(),
            }];
            v.extend((0..n).map(|_| Line::Insn("nop".into(), 4)));
            v.push(Line::Label("end".into()));
            v
        };
        // The branch and 1022 words before the label: +4092 bytes, in reach.
        assert!(!relax_branches(&lines(1022))[0]);
        // +4096 is not (forward, the limit is +4094).
        assert!(relax_branches(&lines(1023))[0]);

        // Backward: a label then a branch to it.
        let back = |n| {
            let mut v = vec![Line::Label("top".into())];
            v.extend((0..n).map(|_| Line::Insn("nop".into(), 4)));
            v.push(Line::Branch {
                op: "bnez",
                regs: "a0",
                target: "top".into(),
            });
            v
        };
        assert!(!*relax_branches(&back(1024)).last().unwrap()); // -4096
        assert!(*relax_branches(&back(1025)).last().unwrap()); // -4100

        let l = lines(2000);
        let mut out = String::new();
        render(&l[..1], &relax_branches(&l)[..1], &mut out);
        assert_eq!(out, "    bnez  a0, 1f\n    j     end\n1:\n");
    }

    #[test]
    fn making_one_branch_long_can_push_another_out_of_reach() {
        // Branch A spans branch B and 1022 words: exactly in reach, until B
        // grows to two words. B spans 2000 words and is long from the start.
        let mut v = vec![Line::Branch {
            op: "beqz",
            regs: "a0",
            target: "a".into(),
        }];
        v.push(Line::Branch {
            op: "beqz",
            regs: "a0",
            target: "b".into(),
        });
        v.extend((0..1021).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("a".into()));
        v.extend((0..2000).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("b".into()));
        assert_eq!(&relax_branches(&v)[..2], [true, true]);
    }

    #[test]
    fn far_compare_branches_are_rewritten() {
        let mut v = vec![Line::Branch {
            op: "beq",
            regs: "a0, t0",
            target: "far".into(),
        }];
        v.extend((0..2000).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("far".into()));
        let mut out = String::new();
        render(&v[..1], &relax_branches(&v)[..1], &mut out);
        assert_eq!(out, "    bne   a0, t0, 1f\n    j     far\n1:\n");
    }

    #[test]
    fn members_are_at_their_offsets() {
        let s = "struct s { char c; int i; };";
        assert_eq!(
            main_body(s, "struct s v; v.i = 5;"),
            "addi a0, s0, -16; addi a0, a0, 4; addi sp, sp, -4; sw a0, 0(sp); li a0, 5; lw a1, 0(sp); \
             addi sp, sp, 4; sw a0, 0(a1); li a0, 0"
        );
        // `p->i` adds the offset to the pointer's value.
        assert_eq!(
            main_body(s, "struct s *p; return p->i;"),
            "lw a0, -12(s0); addi a0, a0, 4; lw a0, 0(a0)"
        );
        // A struct's value is its address: `.c` at offset 0 adds nothing.
        assert_eq!(
            main_body(s, "struct s v; return v.c;"),
            "addi a0, s0, -16; lbu a0, 0(a0)"
        );
    }

    #[test]
    fn struct_assignment_copies() {
        // Word-aligned: one word at a time, unrolled.
        assert_eq!(
            main_body("struct s { int a, b; } x, y;", "x = y;"),
            "la a0, x; addi sp, sp, -4; sw a0, 0(sp); la a0, y; lw a1, 0(sp); addi sp, sp, 4; \
             lw t0, 0(a0); sw t0, 0(a1); lw t0, 4(a0); sw t0, 4(a1); mv a0, a1; li a0, 0"
        );
        // Aligned to 2: halves, so no access is misaligned.
        assert!(main_body("struct s { short a, b, c; } x, y;", "x = y;").contains(
            "lhu t0, 0(a0); sh t0, 0(a1); lhu t0, 2(a0); sh t0, 2(a1); lhu t0, 4(a0); sh t0, 4(a1)"
        ));
        // Big: a loop.
        let body = main_body("struct s { int v[40]; } x, y;", "x = y;");
        assert!(
            body.contains(
                "mv t1, a0; mv t2, a1; li t3, 160; add t3, t1, t3; lw t0, 0(t1); sw t0, 0(t2); \
                 addi t1, t1, 4; addi t2, t2, 4; bltu t1, t3, 2b"
            ),
            "{body}"
        );
    }

    #[test]
    fn small_structs_travel_in_registers() {
        let asm = compile(
            "struct s { int a, b; }; struct s f(struct s x) { return x; }
             int main() { struct s v; return f(v).b; }",
        );
        // The callee stores both words, and returns them from the struct's address.
        assert_eq!(
            body_of(&asm, "f")[4..],
            [
                "sw a0, -16(s0)",
                "sw a1, -12(s0)",
                "addi a0, s0, -16",
                "mv t1, a0",
                "lw a0, 0(t1)",
                "lw a1, 4(t1)",
            ]
        );
        // The caller loads them, and keeps the result in a hidden local.
        assert_eq!(
            body_of(&asm, "main")[5..].join("; "),
            "addi a0, s0, -16; mv t1, a0; lw a0, 0(t1); lw a1, 4(t1); call f; sw a0, -24(s0); \
             sw a1, -20(s0); addi a0, s0, -24; addi a0, a0, 4; lw a0, 0(a0)"
        );
        assert!(
            asm.contains("#     -24(s0)  temp struct s (call result)\n"),
            "{asm}"
        );
        // Aligned to less than a word: each word is put together from aligned pieces.
        let asm =
            compile("struct s { char c[3]; }; int f(struct s x); int main() { struct s v; return f(v); }");
        assert!(
            body_of(&asm, "main").join("; ").contains(
                "mv t1, a0; lbu a0, 0(t1); lbu t0, 1(t1); slli t0, t0, 8; or a0, a0, t0; \
                 lbu t0, 2(t1); slli t0, t0, 16; or a0, a0, t0; call f"
            ),
            "{asm}"
        );
    }

    #[test]
    fn big_structs_travel_by_reference() {
        let asm = compile(
            "struct b { int v[3]; }; struct b f(int k, struct b x) { return x; }
             int main() { struct b v; return f(1, v).v[2]; }",
        );
        // The callee keeps the result's address (a0) and copies its argument
        // from the caller's copy (a2: the result address took a0).
        let f = body_of(&asm, "f").join("; ");
        assert!(
            f.starts_with(
                "addi sp, sp, -16; sw ra, 12(sp); sw s0, 8(sp); addi s0, sp, 16; addi sp, sp, -16; \
                 sw a0, -28(s0); sw a1, -12(s0); addi t4, s0, -24; lw t0, 0(a2); sw t0, 0(t4)"
            ),
            "{f}"
        );
        // `return x` copies x to where the caller asked.
        assert!(
            f.ends_with(
                "addi a0, s0, -24; lw a1, -28(s0); lw t0, 0(a0); sw t0, 0(a1); lw t0, 4(a0); \
                 sw t0, 4(a1); lw t0, 8(a0); sw t0, 8(a1)"
            ),
            "{f}"
        );
        // The caller copies the argument, passes the copy's address and the result's.
        let main = body_of(&asm, "main").join("; ");
        assert!(
            main.contains(
                "addi a0, s0, -20; addi a1, s0, -32; lw t0, 0(a0); sw t0, 0(a1); lw t0, 4(a0); \
                 sw t0, 4(a1); lw t0, 8(a0); sw t0, 8(a1); mv a0, a1; mv a2, a0; lw a1, 0(sp); \
                 addi sp, sp, 4; addi a0, s0, -44; call f; addi a0, s0, -44"
            ),
            "{main}"
        );
    }

    #[test]
    fn calls_through_pointers() {
        let asm = compile("int f(int a, int b); int main() { int (*p)(int, int) = f; return p(1, 2); }");
        assert_eq!(
            body_of(&asm, "main")[4..].join("; "),
            "la a0, f; sw a0, -12(s0); lw a0, -12(s0); addi sp, sp, -4; sw a0, 0(sp); li a0, 1; \
             addi sp, sp, -4; sw a0, 0(sp); li a0, 2; mv a1, a0; lw a0, 0(sp); addi sp, sp, 4; \
             lw t1, 0(sp); addi sp, sp, 4; jalr t1"
        );
        let asm = compile("int main() { int (*p)(void) = 0; return p(); }");
        assert!(body_of(&asm, "main")
            .join("; ")
            .ends_with("lw a0, -12(s0); mv t1, a0; jalr t1"));
        // A function table is data with relocations.
        let asm = compile("int f(void); int g(void); int (*t[2])(void) = {f, g}; int main() { return 0; }");
        assert!(asm.contains("t:\n    .word f\n    .word g\n"), "{asm}");
    }

    #[test]
    fn switch_is_a_compare_chain() {
        let asm = compile(
            "int main(int x) { switch (x) { case 1: return 5; case -2: default: break; } return 0; }",
        );
        assert_eq!(
            body_of(&asm, "main")[5..].join("; "),
            "lw a0, -12(s0); li t0, 1; beq a0, t0, .L.case.main.0; li t0, -2; beq a0, t0, .L.case.main.1; \
             j .L.case.main.2; li a0, 5; j .L.return.main; j .L.break.1; li a0, 0"
        );
        assert!(asm.contains("\n.L.case.main.1:\n.L.case.main.2:\n"), "{asm}");
        // No default: past the body.
        let asm = compile("int main(unsigned x) { switch (x) { case 0xffffffff: x = 1; } return x; }");
        assert!(
            body_of(&asm, "main")
                .join("; ")
                .contains("li t0, -1; beq a0, t0, .L.case.main.0; j .L.break.1"),
            "{asm}"
        );
    }

    #[test]
    fn goto_jumps_to_function_labels() {
        let asm = compile("int main() { goto out; out: return 1; }");
        assert_eq!(body_of(&asm, "main")[4..], ["j .L.label.main.out", "li a0, 1"]);
        assert!(asm.contains("\n.L.label.main.out:\n"), "{asm}");
    }

    #[test]
    fn bool_conversions() {
        let cast = |from: &str| {
            body_of(
                &compile(&format!("int main({from} x) {{ return (_Bool)x; }}")),
                "main",
            )[6..]
                .join("; ")
        };
        assert_eq!(cast("int"), "snez a0, a0");
        assert_eq!(cast("char *"), "snez a0, a0");
        assert_eq!(cast("_Bool"), "");
        assert_eq!(
            main_body("", "_Bool b = 5; return b;"),
            "li a0, 1; sb a0, -9(s0); lbu a0, -9(s0)"
        );
        // `b--` cannot be undone from the new value: the old one is kept in t1.
        assert_eq!(
            main_body("", "_Bool b; return b--;"),
            "lbu a0, -9(s0); mv t1, a0; addi a0, a0, -1; snez a0, a0; sb a0, -9(s0); mv a0, t1"
        );
    }

    #[test]
    fn qualified_objects() {
        let asm = compile(
            "const int k = 3; const char msg[] = \"hi\"; const int z; volatile int v = 1; const volatile int cv = 2;
             int main() { return k + v; }",
        );
        for want in [
            "    .section .rodata\n\n    # const int k: 4 bytes\n    .globl k\n    .balign 4\n    .type k, @object\n    .size k, 4\nk:\n    .word 3\n",
            "msg:\n    .ascii \"hi\"\n    .zero 1\n",
            "z:\n    .zero 4\n",
            "    .data\n\n    # volatile int v: 4 bytes\n",
            "# const volatile int cv: 4 bytes",
        ] {
            assert!(asm.contains(want), "missing {want:?} in\n{asm}");
        }
        assert!(
            !asm[asm.find("cv:").unwrap() - 200..].contains(".rodata"),
            "{asm}"
        );
    }

    /// Volatile objects are memory-mapped registers: every access in the
    /// source must be exactly one load or store, in source order, even when
    /// its value is discarded or it repeats the previous one. merlin has no
    /// optimizer, so today this holds by construction; the test keeps it so.
    #[test]
    fn volatile_accesses_are_kept() {
        // The loads and stores of `f` that do not go through sp or s0 (the
        // frame and the expression stack): here, exactly the volatile ones.
        let accesses = |src: &str, f: &str| -> Vec<String> {
            body_of(&compile(src), f)
                .into_iter()
                .filter(|l| {
                    let op = l.split(' ').next().unwrap();
                    matches!(op, "lw" | "lh" | "lhu" | "lb" | "lbu" | "sw" | "sh" | "sb")
                        && !l.ends_with("(sp)")
                        && !l.ends_with("(s0)")
                })
                .collect()
        };
        // `v; v;` are two reads, `v = 1; v = 1;` two writes, `v + v` two
        // more reads.
        assert_eq!(
            accesses(
                "volatile int v; int main() { v; v; v = 1; v = 1; return v + v; }",
                "main"
            ),
            [
                "lw a0, 0(a0)",
                "lw a0, 0(a0)",
                "sw a0, 0(a1)",
                "sw a0, 0(a1)",
                "lw a0, 0(a0)",
                "lw a0, 0(a0)"
            ]
        );
        // Through a pointer to a volatile struct and a cast of an address,
        // as a driver writes them.
        let src = "struct regs { int status; short data; };
             int g(volatile struct regs *r) {
                 r->data; (void)r->status; r->data = 5; r->data = 5;
                 *(volatile int *)0x10000000; *(volatile unsigned char *)0x10000000 |= 4;
                 while (r->status & 1) {}
                 return r->data + r->data;
             }";
        assert_eq!(
            accesses(src, "g"),
            [
                "lh a0, 0(a0)",  // r->data;
                "lw a0, 0(a0)",  // (void)r->status;
                "sh a0, 0(a1)",  // r->data = 5;
                "sh a0, 0(a1)",  // r->data = 5;
                "lw a0, 0(a0)",  // *(volatile int *)0x10000000;
                "lbu a0, 0(a0)", // ... |= 4 reads once
                "sb a0, 0(a1)",  // and writes once
                "lw a0, 0(a0)",  // r->status, in the loop condition
                "lh a0, 0(a0)",  // r->data +
                "lh a0, 0(a0)",  // r->data
            ]
        );
    }

    #[test]
    fn struct_data() {
        let asm = compile(
            "struct s { char c; int i; short h[3]; }; struct s g = {1, 2, {3}}; union u { char c; int i; } w = {7};
             int main() { return 0; }",
        );
        assert!(
            asm.contains(
                "g:\n    .byte 1\n    .zero 3\n    .word 2\n    .half 3\n    .zero 4\n    .zero 2\n"
            ),
            "{asm}"
        );
        assert!(asm.contains("w:\n    .byte 7\n    .zero 3\n"), "{asm}");
    }

    #[test]
    fn inline_definitions_are_local() {
        let asm = compile("inline int f(void) { return 1; } extern inline int g(void) { return 2; } int main() { return f(); }");
        assert!(!asm.contains(".globl f"), "{asm}");
        assert!(asm.contains(".globl g"), "{asm}");
    }

    #[test]
    fn register_names_cannot_be_symbols() {
        for name in ["sp", "a0", "x5", "fp", "zero"] {
            for src in [
                format!("int {name}() {{ return 0; }} int main() {{ return 0; }}"),
                format!("int {name}; int main() {{ return 0; }}"),
            ] {
                let errs = crate::compile(&src).unwrap_err();
                assert_eq!(
                    errs[0].to_string(),
                    format!("1:5: '{name}' is a register name, which cannot be used as a symbol name")
                );
            }
        }
        // Locals may have any name: they have no symbol.
        compile("int main() { int a0 = 1; return a0; }");
    }
}
