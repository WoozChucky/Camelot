//! The order of the compile unit's entries, as clang 23 has it.
//!
//! LLVM writes a unit's entries in the order it makes them (`DwarfDebug`,
//! `DwarfCompileUnit`, `DwarfUnit`), and it makes them in phases:
//!
//! 1. as the module begins, the enums (the unit's `enums` list) and then the
//!    retained types (the types of explicit casts, which clang keeps:
//!    `CGDebugInfo::EmitExplicitCastType`);
//! 2. as each function's code is done, its `DW_TAG_subprogram`, in the order
//!    of the code, with its variables and blocks but none of their types;
//! 3. as the module ends, the unit's globals (its `globals` list, string
//!    literals among them);
//! 4. then each function's static locals and the types it defines, inside its
//!    entry (its `retainedNodes`);
//! 5. then each function's return type;
//! 6. then the types of each function's parameters and locals, in the order
//!    they are declared.
//!
//! Phases 4 to 6 go through the functions as 2 does, in the order of their
//! code (LLVM's list of the subprograms it made), not the order they were
//! emitted in: a `static` function emitted last may have its code first.
//!
//! Making an entry makes the types it refers to that are not made yet, depth
//! first, each one added before those it refers to (a pointer before what it
//! points to, a struct before its members' types, an array before its element
//! type and then `__ARRAY_SIZE_TYPE__`). A type is added to the unit unless it
//! belongs elsewhere: an anonymous type a member is of to its record (which is
//! made first), a type a function defines to the function.
//!
//! The lists of phases 1, 3 and 4 are clang's front end's, in the order it
//! makes them, which is the order it emits the file's functions and globals
//! (`CodeGenModule`): the external functions, and the external globals with
//! an initializer, as they come; then the tentative definitions (globals
//! without an initializer); then, as they are first used, the `static`
//! functions and globals (an unused one is not emitted at all). merlin's code
//! generator emits them in that order too ("Order" in [`crate::codegen`]),
//! so its items are in it already. Emitting a function makes its signature's
//! types and then, as its code comes, its locals', static locals' (after
//! their initializers'), casts' and string literals', and the enums of the
//! enumeration constants it uses; emitting a global makes its initializer's
//! casts' types and string literals and then its type. An enum goes to the
//! `enums` list as it is made, and a function's type or static local to its
//! `retainedNodes`.
//!
//! A compound literal of the file's scope is not described (as in clang), but
//! its string literals and casts are, as clang emits them: as it emits the
//! initializer that names the compound literal, which merlin does too.
//!
//! Phase 2 follows the code, which is in the order clang's module has the
//! functions: the order each was first defined or referred to, so a `static`
//! function emitted later than the others may come before some of them.

use super::{Event, Lines, TypeKey, TypeNode};
use std::collections::HashMap;

/// An entry of the unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Child {
    Type(usize),
    Function(usize),
    Global(usize),
    Literal(usize),
}

/// An entry of a function that clang makes after its variables and blocks: a
/// type it defines, or a static local (by symbol).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Local {
    Type(usize),
    Static(String),
}

/// Where each entry goes.
pub(super) struct Order {
    /// The unit's children.
    pub unit: Vec<Child>,
    /// Each function's types and static locals, after its blocks.
    pub locals: Vec<Vec<Local>>,
}

impl Lines {
    /// The types a type's entry refers to, in the order LLVM makes them
    /// (`__ARRAY_SIZE_TYPE__` after an array's element type).
    fn referred(&self, t: usize) -> Vec<usize> {
        match &self.types[t] {
            TypeNode::Base(_) | TypeNode::ArraySize => Vec::new(),
            TypeNode::Ptr(to) | TypeNode::Qual(_, to) | TypeNode::Typedef(_, to, ..) => {
                to.iter().copied().collect()
            }
            TypeNode::Array(elem, _) => {
                let mut v = vec![*elem];
                v.extend(self.type_ids.get(&TypeKey::ArraySize));
                v
            }
            TypeNode::Record(_, members, ..) => members.iter().filter_map(|m| m.1).collect(),
            TypeNode::Func(ret, params, _) => ret.iter().chain(params.iter().flatten()).copied().collect(),
            TypeNode::Enum(_, of, ..) => vec![*of],
        }
    }

    /// The order of the unit's entries, and of each function's types and
    /// static locals, as clang's: see the module's documentation. `nested`
    /// and `local` say which types are a record's and a function's.
    pub(super) fn order(&self, nested: &[(usize, (usize, usize))], local: &[(usize, usize)]) -> Order {
        let local_of: HashMap<usize, usize> = local.iter().copied().collect();
        let record_of: HashMap<usize, usize> = nested.iter().map(|&(t, (r, _))| (t, r)).collect();

        let items = &self.items;
        // What the front end makes as it emits them: the enums, the kept
        // types, the globals and each function's types and static locals.
        let mut made = vec![false; self.types.len()];
        let mut enums = Vec::new();
        let mut retained: Vec<usize> = Vec::new();
        let mut globals = Vec::new();
        let mut locals: Vec<Vec<Local>> = vec![Vec::new(); self.functions.len()];
        fn make(
            lines: &Lines,
            t: Option<usize>,
            made: &mut [bool],
            enums: &mut Vec<usize>,
            locals: &mut [Vec<Local>],
            local_of: &HashMap<usize, usize>,
        ) {
            let Some(t) = t else { return };
            if made[t] {
                return;
            }
            made[t] = true;
            if let Some(&f) = local_of.get(&t) {
                locals[f].push(Local::Type(t));
            } else if matches!(lines.types[t], TypeNode::Enum(..)) {
                enums.push(t);
            }
            for r in lines.referred(t) {
                // `__ARRAY_SIZE_TYPE__` is LLVM's, not the front end's.
                if !matches!(lines.types[r], TypeNode::ArraySize) {
                    make(lines, Some(r), made, enums, locals, local_of);
                }
            }
        }
        for &(func, i) in items {
            let mut make = |t: Option<usize>, locals: &mut Vec<Vec<Local>>| {
                make(self, t, &mut made, &mut enums, locals, &local_of)
            };
            if func {
                let f = &self.functions[i];
                make(f.ret, &mut locals);
                for p in &f.params {
                    make(p.ty, &mut locals);
                }
                for e in &f.events {
                    match e {
                        Event::Local(t) => make(*t, &mut locals),
                        Event::Static(sym, t) => {
                            make(*t, &mut locals);
                            locals[i].push(Local::Static(sym.clone()));
                        }
                        Event::Cast(t) => {
                            make(*t, &mut locals);
                            retained.extend(t.filter(|t| !retained.contains(t)));
                        }
                        Event::Literal(l) => {
                            make(self.literals[*l].ty, &mut locals);
                            globals.push(Child::Literal(*l));
                        }
                        Event::Enum(t) => make(Some(*t), &mut locals),
                    }
                }
            } else {
                for e in &self.globals[i].events {
                    match e {
                        Event::Literal(l) => {
                            make(self.literals[*l].ty, &mut locals);
                            globals.push(Child::Literal(*l));
                        }
                        Event::Cast(t) => {
                            make(*t, &mut locals);
                            retained.extend(t.filter(|t| !retained.contains(t)));
                        }
                        _ => {}
                    }
                }
                make(self.globals[i].var.ty, &mut locals);
                globals.push(Child::Global(i));
            }
        }
        // A function's statics its code never declared (none should be).
        for (i, f) in self.functions.iter().enumerate() {
            for v in &f.body.statics {
                if let super::Location::Symbol(s) = &v.location {
                    if !locals[i].contains(&Local::Static(s.clone())) {
                        locals[i].push(Local::Static(s.clone()));
                    }
                }
            }
        }
        // And a function's types the front end did not reach.
        for &(t, f) in local {
            if !locals[f].contains(&Local::Type(t)) {
                locals[f].push(Local::Type(t));
            }
        }

        // ---- LLVM: the entries, in the order it makes them ----
        struct Maker<'a> {
            lines: &'a Lines,
            made: Vec<bool>,
            unit: Vec<Child>,
            record_of: HashMap<usize, usize>,
            local_of: HashMap<usize, usize>,
        }
        impl Maker<'_> {
            fn ty(&mut self, t: Option<usize>) {
                let Some(t) = t else { return };
                if self.made[t] {
                    return;
                }
                // An anonymous type is made with its record.
                if let Some(&r) = self.record_of.get(&t) {
                    if !self.made[r] {
                        self.ty(Some(r));
                        if self.made[t] {
                            return;
                        }
                    }
                }
                self.made[t] = true;
                if !self.record_of.contains_key(&t) && !self.local_of.contains_key(&t) {
                    self.unit.push(Child::Type(t));
                }
                for r in self.lines.referred(t) {
                    self.ty(Some(r));
                }
            }
        }
        let mut m = Maker {
            lines: self,
            made: vec![false; self.types.len()],
            unit: Vec::new(),
            record_of,
            local_of,
        };
        // 1. The enums, then the kept types.
        for &t in enums.iter().chain(&retained) {
            m.ty(Some(t));
        }
        // 2. The functions, in the order of their code.
        m.unit.extend(self.code.iter().copied().map(Child::Function));
        // 3. The globals.
        for g in globals {
            m.unit.push(g);
            m.ty(match g {
                Child::Global(i) => self.globals[i].var.ty,
                Child::Literal(l) => self.literals[l].ty,
                _ => None,
            });
        }
        // 4. Each function's types and static locals, the functions in the
        // order of their code (LLVM's list of subprograms), as 5 and 6.
        for &i in &self.code {
            for x in &locals[i] {
                match x {
                    Local::Type(t) => m.ty(Some(*t)),
                    Local::Static(sym) => {
                        let ty = self
                            .functions
                            .iter()
                            .flat_map(|f| &f.body.statics)
                            .find(|v| matches!(&v.location, super::Location::Symbol(s) if s == sym))
                            .and_then(|v| v.ty);
                        m.ty(ty);
                    }
                }
            }
        }
        // 5. Each function's return type.
        for &i in &self.code {
            m.ty(self.functions[i].ret);
        }
        // 6. Each function's parameters' and locals' types.
        for &i in &self.code {
            let f = &self.functions[i];
            for p in &f.params {
                m.ty(p.ty);
            }
            for e in &f.events {
                if let Event::Local(t) = e {
                    m.ty(*t);
                }
            }
        }
        // Anything else merlin describes (none should be left).
        for t in 0..self.types.len() {
            m.ty(Some(t));
        }
        Order { unit: m.unit, locals }
    }
}
