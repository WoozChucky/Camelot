//! A reference interpreter for the checked AST, used only by the tests.
//!
//! It gives the harness's `// expect:` lines a second opinion and a quick way
//! to tell a codegen bug from a front-end bug. It shares nothing with code
//! generation but the checked tree: memory is a flat byte array, globals and
//! string literals are laid out from a low address, each call's locals get a
//! fresh area further up, and every value is the number it is (an `i128`, in
//! the range of its C type; a pointer is an address). The program's target
//! gives the sizes: ILP32 on RV32, LP64 on RV64.
//! A struct's value is its address (as an array's), and assigning, passing or
//! returning it copies its bytes. Functions get made-up addresses below the
//! data, so pointers to them compare and convert like any pointer but cannot
//! be read through.
//!
//! Semantics match what the generated code must do on Excalibur (spec section
//! 10, "Merlin C subset"): arithmetic wrapping at the width of its type (32
//! bits for `int`, as the W instructions do on RV64), `div`/`rem` rounding
//! toward zero with RISC-V's results for `INT_MIN / -1` and division by zero,
//! shifts by the low 5 bits of the amount (6 for a 64-bit type), comparisons
//! and `! && ||` give 0 or 1, and falling off the end of a function returns 0.
//!
//! A variadic function's extra arguments are laid out as the calling
//! convention has them in words (see `merlin::abi`; a word is a register, 4
//! or 8 bytes), in an area of the callee's frame that `va_start` points at: a
//! scalar takes a word, a small struct its bytes rounded up to words, a big
//! one a word holding the address of a copy.
//!
//! `switch` and `goto` run by seeking: the statements are walked without
//! being run until the `case`, `default` or label is found, and run normally
//! from there (into and out of blocks and loops, as C's jumps go).

use merlin::abi::{self, Pass};
use merlin::ast::*;
use merlin::consteval::{self, Base, Const};
use merlin::types::{ptr_size, IntKind, Type, Xlen};
use std::collections::HashMap;

/// A value: an integer in the range of its type, or an address.
type Val = i128;

/// Runs `entry()` and returns its result. Uses a thread with a big stack, since
/// every interpreted call nests several Rust calls (debug builds use big frames).
pub fn run(prog: &Program, entry: &str) -> Result<i32, String> {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn_scoped(s, || run_here(prog, entry))
            .unwrap()
            .join()
            .unwrap()
    })
}

/// Enough for every harness program; catches accidental infinite loops.
const MAX_STEPS: u64 = 50_000_000;
/// Keeps runaway recursion from overflowing the interpreter's stack.
const MAX_DEPTH: usize = 1_000;
/// Where the made-up function addresses start: below the data, so reading
/// through a function pointer traps.
const FUNCS_START: u32 = 0x100;
/// Where globals and string literals start: address 0 stays invalid, so null
/// pointers trap.
const DATA_START: u32 = 0x1000;
/// Where the frames of the calls in progress start.
const FRAMES_START: u32 = 8 << 20;
/// Bytes of memory in all.
const MEMORY: usize = 16 << 20;

fn run_here(prog: &Program, entry: &str) -> Result<i32, String> {
    let mut it = Interp {
        x: prog.xlen,
        funcs: HashMap::new(),
        mem: vec![0; MEMORY],
        symbols: HashMap::new(),
        strings: HashMap::new(),
        data_top: DATA_START,
        top: FRAMES_START,
        steps: 0,
        depth: 0,
    };
    // Every function gets an address (a declared one too: calling it is an
    // error only if it happens), its definition if it has one.
    for f in prog.funcs() {
        let next = FUNCS_START + 4 * it.symbols.len() as u32;
        let addr = *it.symbols.entry(f.name.clone()).or_insert(next);
        let def = it.funcs.entry(addr).or_insert((f.name.as_str(), None));
        if f.body.is_some() {
            def.1 = Some(f);
        }
    }
    // Lay out every defined global and static local, then initialize them
    // (an initializer may point at a variable defined further down).
    let mut defs = Vec::new();
    for item in &prog.items {
        match item {
            Item::Decl(ds) => defs.extend(ds.iter().filter(|d| d.define)),
            Item::Func(f) => {
                if let Some(body) = &f.body {
                    statics(body, &mut defs);
                }
            }
        }
    }
    for d in &defs {
        let addr = it.alloc_data(&d.ty);
        it.symbols.insert(symbol(d), addr);
    }
    for d in &defs {
        if let Some(init) = &d.init {
            let addr = it.symbols[&symbol(d)];
            it.static_init(&d.ty, init, addr)?;
        }
    }
    let entry = *it.symbols.get(entry).ok_or("no entry function")?;
    it.call(entry, Vec::new(), None).map(|v| v as i32)
}

/// Every `static` local in a function body.
fn statics<'a>(body: &'a [Stmt], out: &mut Vec<&'a Declarator>) {
    for s in body {
        let one = std::slice::from_ref;
        match &s.kind {
            StmtKind::Decl(ds) => out.extend(ds.iter().filter(|d| matches!(d.var, Some(VarRef::Global(_))))),
            StmtKind::Block(b) => statics(b, out),
            StmtKind::If { then, els, .. } => {
                statics(one(then), out);
                if let Some(e) = els {
                    statics(one(e), out);
                }
            }
            StmtKind::While { body, .. }
            | StmtKind::DoWhile { body, .. }
            | StmtKind::Switch { body, .. }
            | StmtKind::Case { body, .. }
            | StmtKind::Default { body, .. }
            | StmtKind::Label { body, .. } => statics(one(body), out),
            StmtKind::For { init, body, .. } => {
                if let Some(i) = init {
                    statics(one(i), out);
                }
                statics(one(body), out);
            }
            _ => {}
        }
    }
}

fn symbol(d: &Declarator) -> String {
    match &d.var {
        Some(VarRef::Global(s)) => s.clone(),
        _ => unreachable!("only globals and statics are laid out"),
    }
}

fn size_of(ty: &Type, x: Xlen) -> u32 {
    ty.size(x).expect("sema: objects have complete types")
}

/// The address a pointer value holds, if memory has it (else one past it,
/// which every access refuses).
fn address(v: Val) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

struct Interp<'a> {
    /// The target: sizes, and the width of arithmetic.
    x: Xlen,
    /// Each function's name and definition, by its made-up address.
    funcs: HashMap<u32, (&'a str, Option<&'a Function>)>,
    mem: Vec<u8>,
    /// Address of every global symbol (functions too).
    symbols: HashMap<String, u32>,
    /// Address of each string literal node (a literal is one object).
    strings: HashMap<*const Expr, u32>,
    /// The next free byte after the globals and string literals.
    data_top: u32,
    /// The next free byte after the frames of the calls in progress.
    top: u32,
    steps: u64,
    depth: usize,
}

enum Flow {
    Next,
    Break,
    Continue,
    /// A value, or for a struct its address.
    Return(Val),
    /// A `goto` to this label, which the function seeks from its start.
    Goto(String),
}

/// What a jump looks for while statements are skipped.
#[derive(PartialEq)]
enum Target {
    Case(usize),
    Label(String),
}

/// What every byte of a fresh local holds (see `Interp::alloc_frame`).
pub const POISON: u8 = 0xa5;

impl<'a> Interp<'a> {
    /// Reserves memory for an object of type `ty` at `*top`, zeroed.
    fn alloc_at(mem: &mut [u8], top: &mut u32, limit: u32, ty: &Type, x: Xlen) -> u32 {
        let align = ty.align(x).max(1);
        let addr = top.div_ceil(align) * align;
        let size = size_of(ty, x);
        *top = addr + size;
        assert!(*top < limit, "interpreter out of memory");
        mem[addr as usize..(addr + size) as usize].fill(0);
        addr
    }

    /// Memory for a global or a string literal, for the whole run.
    fn alloc_data(&mut self, ty: &Type) -> u32 {
        Self::alloc_at(&mut self.mem, &mut self.data_top, FRAMES_START, ty, self.x)
    }

    /// Memory for a local, until its call returns. A local declared without
    /// an initializer is indeterminate in C, and the generated code leaves
    /// whatever the stack held, so it starts filled with [`POISON`] rather
    /// than zeros: a case that reads one gets a value no expectation is
    /// likely to match, rather than a 0 that passes here and not in Grail.
    fn alloc_frame(&mut self, ty: &Type) -> u32 {
        let addr = Self::alloc_at(&mut self.mem, &mut self.top, MEMORY as u32, ty, self.x);
        let size = self.size(ty);
        self.mem[addr as usize..(addr + size) as usize].fill(POISON);
        addr
    }

    fn size(&self, ty: &Type) -> u32 {
        size_of(ty, self.x)
    }

    fn check(&self, addr: u32, size: u32) -> Result<usize, String> {
        let end = addr as u64 + size as u64;
        let in_data = addr >= DATA_START && end <= self.data_top as u64;
        let in_frames = addr >= FRAMES_START && end <= self.top as u64;
        if !in_data && !in_frames {
            return Err(format!(
                "access to {size} bytes at {addr:#x}, outside every object"
            ));
        }
        Ok(addr as usize)
    }

    /// Loads a value of type `ty`: its bytes, read as the type reads them.
    fn load(&self, addr: u32, ty: &Type) -> Result<Val, String> {
        let size = self.size(ty);
        let a = self.check(addr, size)?;
        let mut bytes = [0u8; 16];
        bytes[..size as usize].copy_from_slice(&self.mem[a..a + size as usize]);
        Ok(convert(i128::from_le_bytes(bytes), ty, self.x))
    }

    /// Stores the value `v` of type `ty`: its low bytes, two's complement.
    fn store(&mut self, addr: u32, ty: &Type, v: Val) -> Result<(), String> {
        let size = self.size(ty);
        let a = self.check(addr, size)?;
        self.mem[a..a + size as usize].copy_from_slice(&v.to_le_bytes()[..size as usize]);
        Ok(())
    }

    /// Copies an object of type `ty` (a struct) from `src` to `dst`.
    fn copy(&mut self, dst: u32, src: u32, ty: &Type) -> Result<(), String> {
        let size = self.size(ty);
        let s = self.check(src, size)?;
        let d = self.check(dst, size)?;
        self.mem.copy_within(s..s + size as usize, d);
        Ok(())
    }

    /// Stores the value `v` of type `ty` at `addr`: a struct's value is the
    /// address of its bytes, which are copied.
    fn put(&mut self, addr: u32, ty: &Type, v: Val) -> Result<(), String> {
        if ty.is_record() {
            self.copy(addr, address(v), ty)
        } else {
            self.store(addr, ty, v)
        }
    }

    fn string(&mut self, e: &Expr, bytes: &[u8]) -> u32 {
        if let Some(&a) = self.strings.get(&(e as *const Expr)) {
            return a;
        }
        let a = self.alloc_data(&Type::array(Type::CHAR, Some(bytes.len() as u32 + 1)));
        self.mem[a as usize..a as usize + bytes.len()].copy_from_slice(bytes);
        self.strings.insert(e as *const Expr, a);
        a
    }

    /// Writes a global's or static's constant initializer.
    fn static_init(&mut self, ty: &Type, init: &Init, addr: u32) -> Result<(), String> {
        match (ty.unqual(), init) {
            // Zero (a subobject a designator skipped): memory starts zeroed.
            (_, Init::List(items, _)) if items.is_empty() => {}
            (Type::Array(elem, _), Init::List(items, _)) => {
                for (i, item) in items.iter().enumerate() {
                    self.static_init(elem, item, addr + i as u32 * self.size(elem))?;
                }
            }
            (Type::Record(r), Init::List(items, _)) => {
                let body = r.body().ok_or("incomplete struct")?;
                for (m, item) in body.members.iter().zip(items) {
                    self.static_init(&m.ty, item, addr + m.offset)?;
                }
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(b),
                    ..
                }),
            ) => {
                let n = b.len().min(*n as usize);
                self.mem[addr as usize..addr as usize + n].copy_from_slice(&b[..n]);
            }
            (_, Init::Expr(e)) => {
                let v = match consteval::eval(e, self.x).ok_or("initializer is not constant")? {
                    Const::Int(v, _) => v,
                    Const::Addr { base: None, offset } => offset,
                    Const::Addr {
                        base: Some(base),
                        offset,
                    } => {
                        let at = match base {
                            Base::Sym(s) => *self.symbols.get(&s).ok_or(format!("'{s}' is never defined"))?,
                            Base::Str(b) => {
                                let a = self.alloc_data(&Type::array(Type::CHAR, Some(b.len() as u32 + 1)));
                                self.mem[a as usize..a as usize + b.len()].copy_from_slice(&b);
                                a
                            }
                        };
                        Val::from(at) + offset
                    }
                };
                self.store(addr, ty, v)?;
            }
            _ => return Err("bad initializer shape".into()),
        }
        Ok(())
    }

    /// Calls the function at address `addr`. A struct argument is the
    /// address of its bytes; a struct result is copied to `result` (the
    /// caller's hidden local) before the callee's frame goes, and that
    /// address is returned. In a variadic function, the frame's last entry
    /// is the address of the extra arguments' words.
    fn call(&mut self, addr: u32, args: Vec<(Val, Type)>, result: Option<u32>) -> Result<Val, String> {
        let (name, f) = *self
            .funcs
            .get(&addr)
            .ok_or_else(|| format!("call through {addr:#x}, which is not a function"))?;
        let f = f.ok_or_else(|| format!("call to '{name}', which has no definition"))?;
        if self.depth == MAX_DEPTH {
            return Err(format!("recursion deeper than {MAX_DEPTH} calls"));
        }
        // A fresh frame: every local gets its own (poisoned) object.
        let saved_top = self.top;
        let mut frame: Vec<u32> = f.locals.iter().map(|l| self.alloc_frame(&l.ty)).collect();
        let named = f.params.len();
        if args.len() != named && !f.variadic {
            return Err(format!("'{name}' called with {} arguments", args.len()));
        }
        for (i, (v, _)) in args.iter().enumerate().take(named) {
            self.put(frame[i], &f.locals[i].ty, *v)?;
        }
        if f.variadic {
            // The words of all the arguments, as the calling convention
            // numbers them (an aligned pair at an even word), from an area
            // aligned to two words; va_start points at the first word the
            // named ones leave.
            let x = self.x;
            let types: Vec<Type> = args.iter().map(|(_, t)| t.clone()).collect();
            let (places, total) = abi::call_arguments(&f.ret, &types, named, x);
            let (_, first) = abi::call_arguments(&f.ret, &types[..named], named, x);
            // A word is a register: a `long` on either width.
            let word = Type::Int(IntKind::Long);
            let w = ptr_size(x);
            // Two words more, so that the area is aligned and has an address
            // of its own.
            let area_ty = Type::array(word.clone(), Some(total as u32 + 2));
            let base = self.alloc_frame(&area_ty).next_multiple_of(2 * w);
            frame.push(base + w * first as u32);
            for ((v, t), &(n, pass)) in args.iter().zip(&places).skip(named) {
                let at = base + w * n as u32;
                match pass {
                    Pass::Scalar => self.store(at, &word, *v)?,
                    Pass::Pair => self.store(at, t, *v)?,
                    Pass::Words(_) => self.copy(at, address(*v), t)?,
                    Pass::Ref => {
                        let copy = self.alloc_frame(t);
                        self.copy(copy, address(*v), t)?;
                        self.store(at, &word, copy.into())?;
                    }
                }
            }
        }
        self.depth += 1;
        let body = f.body.as_ref().unwrap();
        let mut seek = None;
        let mut value = 0;
        loop {
            match self.block(body, &frame, &mut seek)? {
                Flow::Return(v) => {
                    value = v;
                    break;
                }
                // Labels belong to the whole function: look from the top.
                Flow::Goto(label) => seek = Some(Target::Label(label)),
                _ => break,
            }
        }
        if let Some(dst) = result {
            self.copy(dst, address(value), &f.ret)?;
            value = dst.into();
        }
        self.depth -= 1;
        self.top = saved_top;
        Ok(value)
    }

    fn tick(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(format!("more than {MAX_STEPS} steps"));
        }
        Ok(())
    }

    fn block(&mut self, body: &'a [Stmt], frame: &[u32], seek: &mut Option<Target>) -> Result<Flow, String> {
        for s in body {
            match self.stmt(s, frame, seek)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Next)
    }

    /// Runs a loop body: whether the loop goes on, or how it ends.
    fn body(
        &mut self,
        body: &'a Stmt,
        frame: &[u32],
        seek: &mut Option<Target>,
    ) -> Result<Option<Flow>, String> {
        Ok(match self.stmt(body, frame, seek)? {
            Flow::Break => Some(Flow::Next),
            Flow::Next | Flow::Continue => None,
            other => Some(other),
        })
    }

    /// Runs a statement, or while `seek` is set, looks for its target in it
    /// (and runs the rest of the statement from there once found).
    fn stmt(&mut self, s: &'a Stmt, frame: &[u32], seek: &mut Option<Target>) -> Result<Flow, String> {
        self.tick()?;
        Ok(match &s.kind {
            StmtKind::Block(body) => self.block(body, frame, seek)?,
            StmtKind::Case { id, body, .. } | StmtKind::Default { id, body } => {
                if seek.as_ref() == Some(&Target::Case(*id)) {
                    *seek = None;
                }
                self.stmt(body, frame, seek)?
            }
            StmtKind::Label { name, body } => {
                if matches!(seek, Some(Target::Label(l)) if l == name) {
                    *seek = None;
                }
                self.stmt(body, frame, seek)?
            }
            StmtKind::If { cond, then, els } => {
                if seek.is_some() {
                    let f = self.stmt(then, frame, seek)?;
                    match els {
                        Some(e) if seek.is_some() => self.stmt(e, frame, seek)?,
                        _ => f,
                    }
                } else if self.expr(cond, frame)? != 0 {
                    self.stmt(then, frame, seek)?
                } else if let Some(e) = els {
                    self.stmt(e, frame, seek)?
                } else {
                    Flow::Next
                }
            }
            StmtKind::While { cond, body } => loop {
                if seek.is_none() && self.expr(cond, frame)? == 0 {
                    break Flow::Next;
                }
                if let Some(f) = self.body(body, frame, seek)? {
                    break f;
                }
                if seek.is_some() {
                    break Flow::Next; // the target is not in this loop
                }
            },
            StmtKind::DoWhile { body, cond } => loop {
                if let Some(f) = self.body(body, frame, seek)? {
                    break f;
                }
                if seek.is_some() {
                    break Flow::Next;
                }
                if self.expr(cond, frame)? == 0 {
                    break Flow::Next;
                }
            },
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                if let (Some(i), None) = (init, &seek) {
                    self.stmt(i, frame, seek)?;
                }
                loop {
                    if let (Some(c), None) = (cond, &seek) {
                        if self.expr(c, frame)? == 0 {
                            break Flow::Next;
                        }
                    }
                    if let Some(f) = self.body(body, frame, seek)? {
                        break f;
                    }
                    if seek.is_some() {
                        break Flow::Next;
                    }
                    if let Some(e) = step {
                        self.expr(e, frame)?;
                    }
                }
            }
            StmtKind::Switch {
                cond,
                body,
                cases,
                default,
            } => {
                if seek.is_none() {
                    let v = self.expr(cond, frame)?;
                    let target = cases
                        .iter()
                        .find(|(value, _)| *value == v)
                        .map(|&(_, id)| id)
                        .or(*default);
                    let Some(id) = target else {
                        return Ok(Flow::Next);
                    };
                    *seek = Some(Target::Case(id));
                }
                match self.stmt(body, frame, seek)? {
                    Flow::Break => Flow::Next,
                    other => other,
                }
            }
            // While seeking, nothing else runs.
            _ if seek.is_some() => Flow::Next,
            StmtKind::Decl(decls) => {
                for d in decls {
                    if let (Some(VarRef::Local(id)), Some(init)) = (&d.var, &d.init) {
                        let addr = frame[id.0];
                        if d.ty.is_array() || d.ty.is_record() {
                            let size = self.size(&d.ty) as usize;
                            self.mem[addr as usize..addr as usize + size].fill(0);
                        }
                        self.local_init(&d.ty, init, addr, frame)?;
                    }
                }
                Flow::Next
            }
            StmtKind::Expr(e) => {
                self.expr(e, frame)?;
                Flow::Next
            }
            StmtKind::Empty | StmtKind::Unevaluated(_) => Flow::Next,
            StmtKind::Return(e) => Flow::Return(match e {
                Some(e) => self.expr(e, frame)?,
                None => 0,
            }),
            StmtKind::Break => Flow::Break,
            StmtKind::Continue => Flow::Continue,
            StmtKind::Goto(label) => Flow::Goto(label.clone()),
        })
    }

    fn local_init(&mut self, ty: &Type, init: &'a Init, addr: u32, frame: &[u32]) -> Result<(), String> {
        match (ty.unqual(), init) {
            // Zero: the object was cleared first.
            (_, Init::List(items, _)) if items.is_empty() => {}
            (Type::Array(elem, _), Init::List(items, _)) => {
                for (i, item) in items.iter().enumerate() {
                    self.local_init(elem, item, addr + i as u32 * self.size(elem), frame)?;
                }
            }
            (Type::Record(r), Init::List(items, _)) => {
                let body = r.body().ok_or("incomplete struct")?;
                for (m, item) in body.members.iter().zip(items) {
                    self.local_init(&m.ty, item, addr + m.offset, frame)?;
                }
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(b),
                    ..
                }),
            ) => {
                let n = b.len().min(*n as usize);
                self.mem[addr as usize..addr as usize + n].copy_from_slice(&b[..n]);
            }
            (_, Init::Expr(e)) => {
                let v = self.expr(e, frame)?;
                self.put(addr, ty, v)?;
            }
            _ => return Err("bad initializer shape".into()),
        }
        Ok(())
    }

    /// The address of an lvalue (or of any struct value).
    fn addr(&mut self, e: &'a Expr, frame: &[u32]) -> Result<u32, String> {
        match &e.kind {
            ExprKind::Var { var, .. } => match var {
                Some(VarRef::Local(id)) => Ok(frame[id.0]),
                Some(VarRef::Global(s)) => self
                    .symbols
                    .get(s)
                    .copied()
                    .ok_or_else(|| format!("'{s}' is never defined")),
                None => Err("unresolved variable".into()),
            },
            ExprKind::Deref(p) => Ok(address(self.expr(p, frame)?)),
            ExprKind::Str(b) => Ok(self.string(e, b)),
            ExprKind::CompoundLit { init, local } => {
                let addr = frame[local.ok_or("compound literal without a local")?.0];
                let size = self.size(&e.ty) as usize;
                self.mem[addr as usize..addr as usize + size].fill(0);
                self.local_init(&e.ty, init, addr, frame)?;
                Ok(addr)
            }
            // `s.m`: `s`'s value is its address; `p->m`: `p` is one.
            ExprKind::Member { base, offset, .. } => {
                Ok(address(self.expr(base, frame)? + Val::from(*offset)))
            }
            _ => Err(format!("{}: not an lvalue", e.pos)),
        }
    }

    /// The value of an lvalue: loaded, or the address itself for an array, a
    /// struct or a function.
    fn value_at(&self, addr: u32, ty: &Type) -> Result<Val, String> {
        if ty.is_addressed() {
            Ok(addr.into())
        } else {
            self.load(addr, ty)
        }
    }

    fn expr(&mut self, e: &'a Expr, frame: &[u32]) -> Result<Val, String> {
        self.tick()?;
        let x = self.x;
        Ok(match &e.kind {
            ExprKind::Num(v) | ExprKind::Enumerator { value: v, .. } => *v,
            ExprKind::Var { .. }
            | ExprKind::Str(_)
            | ExprKind::Deref(_)
            | ExprKind::Member { .. }
            | ExprKind::CompoundLit { .. } => {
                let a = self.addr(e, frame)?;
                self.value_at(a, &e.ty)?
            }
            ExprKind::Addr(p) => self.addr(p, frame)?.into(),
            ExprKind::Assign { op, target, value } => {
                let a = self.addr(target, frame)?;
                let v = match op {
                    None => self.expr(value, frame)?,
                    Some(op) => {
                        let old = self.load(a, &target.ty)?;
                        let v = self.expr(value, frame)?;
                        let (t, vt) = (target.ty.decay(), value.ty.decay());
                        convert(binary(*op, old, &t, v, &vt, x)?, &target.ty, x)
                    }
                };
                self.put(a, &target.ty, v)?;
                // A struct assignment's value is the target.
                if target.ty.is_record() {
                    a.into()
                } else {
                    v
                }
            }
            ExprKind::IncDec { inc, prefix, target } => {
                let a = self.addr(target, frame)?;
                let old = self.load(a, &target.ty)?;
                let step = match target.ty.unqual() {
                    Type::Ptr(t) => self.size(t).into(),
                    _ => 1,
                };
                let new = convert(if *inc { old + step } else { old - step }, &target.ty, x);
                self.store(a, &target.ty, new)?;
                if *prefix {
                    new
                } else {
                    old
                }
            }
            ExprKind::Binary(BinOp::LogAnd, l, r) => {
                (self.expr(l, frame)? != 0 && self.expr(r, frame)? != 0).into()
            }
            ExprKind::Binary(BinOp::LogOr, l, r) => {
                (self.expr(l, frame)? != 0 || self.expr(r, frame)? != 0).into()
            }
            ExprKind::Binary(op, l, r) => {
                let a = self.expr(l, frame)?;
                let b = self.expr(r, frame)?;
                binary(*op, a, &l.ty.decay(), b, &r.ty.decay(), x)?
            }
            ExprKind::Unary(op, operand) => {
                let v = self.expr(operand, frame)?;
                // The result has the promoted type, `e.ty`.
                match op {
                    UnOp::Neg => convert(-v, &e.ty, x),
                    UnOp::Plus => v,
                    UnOp::Not => (v == 0).into(),
                    UnOp::BitNot => convert(!v, &e.ty, x),
                }
            }
            ExprKind::Cast { x: from, .. } => {
                let v = self.expr(from, frame)?;
                convert(v, &e.ty, x)
            }
            ExprKind::SizeofType(_) | ExprKind::SizeofExpr(_) => return Err("sizeof survived sema".into()),
            ExprKind::Cond(c, a, b) => {
                if self.expr(c, frame)? != 0 {
                    self.expr(a, frame)?
                } else {
                    self.expr(b, frame)?
                }
            }
            ExprKind::Comma(a, b) => {
                self.expr(a, frame)?;
                self.expr(b, frame)?
            }
            ExprKind::Call {
                callee, args, result, ..
            } => {
                // The callee, then the arguments left to right; C leaves the
                // order unspecified, so harness programs must not depend on it.
                let f = address(self.expr(callee, frame)?);
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push((self.expr(a, frame)?, a.ty.decay()));
                }
                self.call(f, vals, result.map(|id| frame[id.0]))?
            }
            ExprKind::VaStart { ap, .. } => {
                let a = self.addr(ap, frame)?;
                let area = *frame.last().ok_or("va_start without a frame")?;
                self.store(a, &ap.ty, area.into())?;
                0
            }
            ExprKind::VaArg(ap) => {
                let a = self.addr(ap, frame)?;
                let mut at = self.load(a, &ap.ty)?;
                let pass = abi::classify(&e.ty, x);
                // An aligned pair is at the next even word.
                if abi::aligned_pair(&e.ty, pass, x) {
                    let align = Val::from(2 * ptr_size(x));
                    at = (at + align - 1) / align * align;
                }
                let words = ptr_size(x) * pass.registers() as u32;
                self.store(a, &ap.ty, at + Val::from(words))?;
                match pass {
                    // A narrower scalar is in the low bytes of its word.
                    Pass::Scalar | Pass::Pair => self.load(address(at), &e.ty)?,
                    // The struct's bytes are right there.
                    Pass::Words(_) => at,
                    Pass::Ref => self.load(address(at), &Type::ptr(Type::Void))?,
                }
            }
            ExprKind::VaEnd(_) | ExprKind::VaCopy(..) => return Err("va_end/va_copy survived sema".into()),
        })
    }
}

/// A value converted to `ty`: an integer type wraps it to its range (C's
/// conversions, the out-of-range signed case the two's complement way, as
/// clang does; `_Bool` gives 0 or 1), a pointer to a pointer-sized unsigned
/// address. Anything else is as it is.
fn convert(v: Val, ty: &Type, x: Xlen) -> Val {
    match ty.unqual() {
        Type::Int(k) => k.wrap(v, x),
        Type::Ptr(_) => IntKind::ULong.wrap(v, x),
        Type::Void => 0,
        _ => v,
    }
}

/// `a op b` with `a` of (decayed) type `lt` and `b` of type `rt`.
fn binary(op: BinOp, a: Val, lt: &Type, b: Val, rt: &Type, x: Xlen) -> Result<Val, String> {
    let elem = |t: &Type| -> Val { t.pointee().map_or(1, |t| size_of(t, x)).into() };
    if let (Some(lk), Some(rk)) = (lt.int_kind(), rt.int_kind()) {
        // Integers: both operands converted to the type the operation is
        // done in (a shift's is its left operand's promoted type), the
        // result wrapped to it.
        let k = match op {
            BinOp::Shl | BinOp::Shr => lk.promoted(),
            _ => merlin::types::common(lk, rk, x),
        };
        let a = k.wrap(a, x);
        let bits = k.size(x) * 8;
        let b = if matches!(op, BinOp::Shl | BinOp::Shr) {
            // The hardware uses the low 5 bits of a 32-bit shift's amount,
            // the low 6 of a 64-bit one's.
            rk.wrap(b, x) & Val::from(bits - 1)
        } else {
            k.wrap(b, x)
        };
        let v = match op {
            BinOp::Add => a + b,
            BinOp::Sub => a - b,
            BinOp::Mul => a.wrapping_mul(b),
            // RISC-V division by zero: quotient all ones, remainder the dividend.
            BinOp::Div if b == 0 => -1,
            BinOp::Rem if b == 0 => a,
            // Truncating toward zero; `INT_MIN / -1` wraps below.
            BinOp::Div => a / b,
            BinOp::Rem => a % b,
            BinOp::Shl => a << b,
            // `a` is negative only when `k` is signed: an arithmetic shift.
            BinOp::Shr => a >> b,
            BinOp::BitAnd => a & b,
            BinOp::BitOr => a | b,
            BinOp::BitXor => a ^ b,
            BinOp::Eq => return Ok((a == b).into()),
            BinOp::Ne => return Ok((a != b).into()),
            BinOp::Lt => return Ok((a < b).into()),
            BinOp::Gt => return Ok((a > b).into()),
            BinOp::Le => return Ok((a <= b).into()),
            BinOp::Ge => return Ok((a >= b).into()),
            BinOp::LogAnd | BinOp::LogOr => {
                return Err("short-circuit operators are evaluated by expr".into())
            }
        };
        return Ok(k.wrap(v, x));
    }
    // Pointers (addresses, unsigned), with an integer or each other.
    let addr = |v: Val| IntKind::ULong.wrap(v, x);
    Ok(match op {
        BinOp::Add if lt.is_ptr() => addr(a + b * elem(lt)),
        BinOp::Add => addr(a * elem(rt) + b),
        // The difference of two pointers counts elements: a `ptrdiff_t`.
        BinOp::Sub if rt.is_ptr() => IntKind::ptrdiff_t(x).wrap(a - b, x) / elem(lt),
        BinOp::Sub => addr(a - b * elem(lt)),
        BinOp::Eq => (a == b).into(),
        BinOp::Ne => (a != b).into(),
        BinOp::Lt => (a < b).into(),
        BinOp::Gt => (a > b).into(),
        BinOp::Le => (a <= b).into(),
        BinOp::Ge => (a >= b).into(),
        _ => return Err(format!("no pointer operator {op:?}")),
    })
}
