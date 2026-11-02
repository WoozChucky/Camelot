//! A reference interpreter for the checked AST, used only by the tests.
//!
//! It gives the harness's `// expect:` lines a second opinion and a quick way
//! to tell a codegen bug from a front-end bug. It shares nothing with code
//! generation but the checked tree: memory is a flat byte array, globals and
//! string literals are laid out from a low address, each call's locals get a
//! fresh area further up, and every value is 32 bits interpreted by its C type.
//! A struct's value is its address (as an array's), and assigning, passing or
//! returning it copies its bytes. Functions get made-up addresses below the
//! data, so pointers to them compare and convert like any pointer but cannot
//! be read through.
//!
//! Semantics match what the generated code must do on Excalibur (spec section
//! 10, "Merlin C subset"): 32-bit wrapping arithmetic, `div`/`rem` rounding
//! toward zero with RISC-V's results for `i32::MIN / -1` and division by zero,
//! shifts by the low 5 bits of the amount, comparisons and `! && ||` give 0 or
//! 1, and falling off the end of a function returns 0.
//!
//! `switch` and `goto` run by seeking: the statements are walked without
//! being run until the `case`, `default` or label is found, and run normally
//! from there (into and out of blocks and loops, as C's jumps go).

use merlin::ast::*;
use merlin::consteval::{self, Base, Const};
use merlin::types::Type;
use std::collections::HashMap;

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

fn size_of(ty: &Type) -> u32 {
    ty.size().expect("sema: objects have complete types")
}

struct Interp<'a> {
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
    Return(u32),
    /// A `goto` to this label, which the function seeks from its start.
    Goto(String),
}

/// What a jump looks for while statements are skipped.
#[derive(PartialEq)]
enum Target {
    Case(usize),
    Label(String),
}

impl<'a> Interp<'a> {
    /// Reserves memory for an object of type `ty` at `*top`, zeroed.
    fn alloc_at(mem: &mut [u8], top: &mut u32, limit: u32, ty: &Type) -> u32 {
        let align = ty.align().max(1);
        let addr = top.div_ceil(align) * align;
        let size = size_of(ty);
        *top = addr + size;
        assert!(*top < limit, "interpreter out of memory");
        mem[addr as usize..(addr + size) as usize].fill(0);
        addr
    }

    /// Memory for a global or a string literal, for the whole run.
    fn alloc_data(&mut self, ty: &Type) -> u32 {
        Self::alloc_at(&mut self.mem, &mut self.data_top, FRAMES_START, ty)
    }

    /// Memory for a local, until its call returns.
    fn alloc_frame(&mut self, ty: &Type) -> u32 {
        Self::alloc_at(&mut self.mem, &mut self.top, MEMORY as u32, ty)
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

    /// Loads a value of type `ty`, widened to 32 bits by its signedness.
    fn load(&self, addr: u32, ty: &Type) -> Result<u32, String> {
        let size = size_of(ty);
        let a = self.check(addr, size)?;
        let mut bytes = [0u8; 4];
        bytes[..size as usize].copy_from_slice(&self.mem[a..a + size as usize]);
        Ok(convert(u32::from_le_bytes(bytes), ty))
    }

    fn store(&mut self, addr: u32, ty: &Type, v: u32) -> Result<(), String> {
        let size = size_of(ty);
        let a = self.check(addr, size)?;
        self.mem[a..a + size as usize].copy_from_slice(&v.to_le_bytes()[..size as usize]);
        Ok(())
    }

    /// Copies an object of type `ty` (a struct) from `src` to `dst`.
    fn copy(&mut self, dst: u32, src: u32, ty: &Type) -> Result<(), String> {
        let size = size_of(ty);
        let s = self.check(src, size)?;
        let d = self.check(dst, size)?;
        self.mem.copy_within(s..s + size as usize, d);
        Ok(())
    }

    /// Stores the value `v` of type `ty` at `addr`: a struct's value is the
    /// address of its bytes, which are copied.
    fn put(&mut self, addr: u32, ty: &Type, v: u32) -> Result<(), String> {
        if ty.is_record() {
            self.copy(addr, v, ty)
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
            (Type::Array(elem, _), Init::List(items, _)) => {
                for (i, item) in items.iter().enumerate() {
                    self.static_init(elem, item, addr + i as u32 * size_of(elem))?;
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
                let v = match consteval::eval(e).ok_or("initializer is not constant")? {
                    Const::Int(v, _) => v as u32,
                    Const::Addr { base: None, offset } => offset as u32,
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
                        at.wrapping_add(offset as u32)
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
    /// address is returned.
    fn call(&mut self, addr: u32, args: Vec<u32>, result: Option<u32>) -> Result<u32, String> {
        let (name, f) = *self
            .funcs
            .get(&addr)
            .ok_or_else(|| format!("call through {addr:#x}, which is not a function"))?;
        let f = f.ok_or_else(|| format!("call to '{name}', which has no definition"))?;
        if self.depth == MAX_DEPTH {
            return Err(format!("recursion deeper than {MAX_DEPTH} calls"));
        }
        // A fresh frame: every local gets its own (zeroed) object.
        let saved_top = self.top;
        let frame: Vec<u32> = f.locals.iter().map(|l| self.alloc_frame(&l.ty)).collect();
        for (i, v) in args.into_iter().enumerate() {
            self.put(frame[i], &f.locals[i].ty, v)?;
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
            self.copy(dst, value, &f.ret)?;
            value = dst;
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
                        .find(|(value, _)| *value as u32 == v)
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
                            let size = size_of(&d.ty) as usize;
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
            StmtKind::Empty => Flow::Next,
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
            (Type::Array(elem, _), Init::List(items, _)) => {
                for (i, item) in items.iter().enumerate() {
                    self.local_init(elem, item, addr + i as u32 * size_of(elem), frame)?;
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
            ExprKind::Deref(p) => self.expr(p, frame),
            ExprKind::Str(b) => Ok(self.string(e, b)),
            // `s.m`: `s`'s value is its address; `p->m`: `p` is one.
            ExprKind::Member { base, offset, .. } => Ok(self.expr(base, frame)?.wrapping_add(*offset)),
            _ => Err(format!("{}: not an lvalue", e.pos)),
        }
    }

    /// The value of an lvalue: loaded, or the address itself for an array, a
    /// struct or a function.
    fn value_at(&self, addr: u32, ty: &Type) -> Result<u32, String> {
        if ty.is_addressed() {
            Ok(addr)
        } else {
            self.load(addr, ty)
        }
    }

    fn expr(&mut self, e: &'a Expr, frame: &[u32]) -> Result<u32, String> {
        self.tick()?;
        Ok(match &e.kind {
            ExprKind::Num(v) => *v as u32,
            ExprKind::Var { .. } | ExprKind::Str(_) | ExprKind::Deref(_) | ExprKind::Member { .. } => {
                let a = self.addr(e, frame)?;
                self.value_at(a, &e.ty)?
            }
            ExprKind::Addr(x) => self.addr(x, frame)?,
            ExprKind::Assign { op, target, value } => {
                let a = self.addr(target, frame)?;
                let v = match op {
                    None => self.expr(value, frame)?,
                    Some(op) => {
                        let old = self.load(a, &target.ty)?;
                        let v = self.expr(value, frame)?;
                        let (t, vt) = (target.ty.decay(), value.ty.decay());
                        convert(binary(*op, old, &t, v, &vt)?, &target.ty)
                    }
                };
                self.put(a, &target.ty, v)?;
                // A struct assignment's value is the target.
                if target.ty.is_record() {
                    a
                } else {
                    v
                }
            }
            ExprKind::IncDec { inc, prefix, target } => {
                let a = self.addr(target, frame)?;
                let old = self.load(a, &target.ty)?;
                let step = match target.ty.unqual() {
                    Type::Ptr(t) => size_of(t),
                    _ => 1,
                };
                let new = convert(
                    if *inc {
                        old.wrapping_add(step)
                    } else {
                        old.wrapping_sub(step)
                    },
                    &target.ty,
                );
                self.store(a, &target.ty, new)?;
                if *prefix {
                    new
                } else {
                    old
                }
            }
            ExprKind::Binary(BinOp::LogAnd, l, r) => {
                (self.expr(l, frame)? != 0 && self.expr(r, frame)? != 0) as u32
            }
            ExprKind::Binary(BinOp::LogOr, l, r) => {
                (self.expr(l, frame)? != 0 || self.expr(r, frame)? != 0) as u32
            }
            ExprKind::Binary(op, l, r) => {
                let a = self.expr(l, frame)?;
                let b = self.expr(r, frame)?;
                binary(*op, a, &l.ty.decay(), b, &r.ty.decay())?
            }
            ExprKind::Unary(op, x) => {
                let v = self.expr(x, frame)?;
                match op {
                    UnOp::Neg => v.wrapping_neg(),
                    UnOp::Plus => v,
                    UnOp::Not => (v == 0) as u32,
                    UnOp::BitNot => !v,
                }
            }
            ExprKind::Cast { x, .. } => {
                let v = self.expr(x, frame)?;
                convert(v, &e.ty)
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
                let f = self.expr(callee, frame)?;
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.expr(a, frame)?);
                }
                self.call(f, vals, result.map(|id| frame[id.0]))?
            }
        })
    }
}

/// A 32-bit value converted to `ty`: narrow integers are cut and widened by
/// their signedness (`_Bool` becomes 0 or 1), everything else is already 32
/// bits.
fn convert(v: u32, ty: &Type) -> u32 {
    match ty.unqual() {
        Type::Int(k) => k.wrap(v as i64) as u32,
        Type::Void => 0,
        _ => v,
    }
}

/// `a op b` with `a` of (decayed) type `lt` and `b` of type `rt`.
fn binary(op: BinOp, a: u32, lt: &Type, b: u32, rt: &Type) -> Result<u32, String> {
    let unsigned = lt.is_unsigned() || rt.is_unsigned();
    let elem = |t: &Type| t.pointee().map(size_of).unwrap_or(1);
    let less = |x: u32, y: u32| if unsigned { x < y } else { (x as i32) < (y as i32) };
    Ok(match op {
        BinOp::Add if lt.is_ptr() => a.wrapping_add(b.wrapping_mul(elem(lt))),
        BinOp::Add if rt.is_ptr() => a.wrapping_mul(elem(rt)).wrapping_add(b),
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub if lt.is_ptr() && rt.is_ptr() => ((a.wrapping_sub(b) as i32) / elem(lt) as i32) as u32,
        BinOp::Sub if lt.is_ptr() => a.wrapping_sub(b.wrapping_mul(elem(lt))),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::Mul => a.wrapping_mul(b),
        // RISC-V division by zero: quotient all ones, remainder the dividend.
        BinOp::Div if b == 0 => u32::MAX,
        BinOp::Rem if b == 0 => a,
        BinOp::Div if unsigned => a / b,
        BinOp::Rem if unsigned => a % b,
        BinOp::Div => (a as i32).wrapping_div(b as i32) as u32,
        BinOp::Rem => (a as i32).wrapping_rem(b as i32) as u32,
        BinOp::Shl => a << (b & 31),
        BinOp::Shr if lt.is_unsigned() => a >> (b & 31),
        BinOp::Shr => ((a as i32) >> (b & 31)) as u32,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Eq => (a == b) as u32,
        BinOp::Ne => (a != b) as u32,
        BinOp::Lt => less(a, b) as u32,
        BinOp::Gt => less(b, a) as u32,
        BinOp::Le => !less(b, a) as u32,
        BinOp::Ge => !less(a, b) as u32,
        BinOp::LogAnd | BinOp::LogOr => return Err("short-circuit operators are evaluated by expr".into()),
    })
}
