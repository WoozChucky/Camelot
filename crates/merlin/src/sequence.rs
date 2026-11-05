//! clang's `-Wunsequenced`: two modifications of one variable, or a
//! modification and a read of it, that C leaves unsequenced in one full
//! expression (`i = i++ + 1`, `a[i] = i++`), which is undefined behaviour
//! (C99 6.5p2). This follows clang's `SequenceChecker` (SemaChecking.cpp)
//! as it runs on C:
//!
//! * The expression is walked in order, each read of a variable (an
//!   lvalue converted to its value) and each modification (`=`, `op=`,
//!   `++`, `--`) noted with the *region* it is in. Regions form a tree:
//!   an operator that orders its operands (`,`, `&&`, `||`, `?:`) gives
//!   each a region of its own under the current one, and the regions of
//!   what it ordered are merged into it once it is done.
//! * Two usages are unsequenced when the older one's region (or what it was
//!   merged into) is the newer one's or one of its ancestors
//!   ([`Tree::unsequenced`]): `i + i++` are in one region; in `(i++, i)`
//!   the comma's left and right regions are siblings, so ordered.
//! * A modification is a value as soon as it happens (`++x` in C++) or a
//!   side effect that only comes after (`x++`, `x = 1` in C): a read
//!   conflicts with a modification that is a value before it is made and
//!   with a side effect after. A sequenced subexpression (the left operand
//!   of `,`, `&&`, `||`, a `?:`'s condition, a call's operands) makes its
//!   side effects values once it is done.
//! * Each variable is reported once, at the modification, among the
//!   warnings of what runs (`DiagRuntimeBehavior`).

use crate::ast::*;
use crate::consteval;
use crate::types::Xlen;
use crate::Pos;
use std::collections::HashMap;

/// How a variable is used.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Read.
    Use,
    /// Modified, the new value there for what follows at once.
    ModAsValue,
    /// Modified, the store a side effect that comes later.
    ModAsSideEffect,
}

/// A usage: where (clang's `getExprLoc` of the expression), what
/// expression it is of, and in which region.
#[derive(Clone, Copy)]
struct Usage {
    at: Pos,
    of: ExprId,
    region: usize,
}

#[derive(Default)]
struct Usages {
    uses: [Option<Usage>; 3],
    diagnosed: bool,
}

/// The regions: each one's parent, and whether it was merged into it.
struct Tree {
    nodes: Vec<(usize, bool)>,
}

impl Tree {
    fn allocate(&mut self, parent: usize) -> usize {
        self.nodes.push((parent, false));
        self.nodes.len() - 1
    }

    fn merge(&mut self, region: usize) {
        self.nodes[region].1 = true;
    }

    /// The region `r` counts as: the first not merged into its parent.
    fn representative(&self, mut r: usize) -> usize {
        while self.nodes[r].1 {
            r = self.nodes[r].0;
        }
        r
    }

    /// Whether what is in `old` is unsequenced with what is in `cur` (the
    /// more recent): `old` stands for `cur` or one of its ancestors.
    fn unsequenced(&self, cur: usize, old: usize) -> bool {
        let (mut c, target) = (self.representative(cur), self.representative(old));
        while c >= target {
            if c == target {
                return true;
            }
            if c == 0 {
                break;
            }
            c = self.nodes[c].0;
        }
        false
    }
}

/// What a walk found: each warning, with the expression it is of.
pub struct Found {
    pub at: Pos,
    pub of: ExprId,
    pub msg: String,
}

struct Checker {
    tree: Tree,
    region: usize,
    usages: HashMap<VarRef, (String, Usages)>,
    /// The side effects noted in the sequenced subexpression being
    /// walked, with what they replaced, to make them values after it.
    side_effects: Option<Vec<(VarRef, Option<Usage>)>>,
    found: Vec<Found>,
    xlen: Xlen,
}

/// The `-Wunsequenced` warnings of the full expression `e`, in order.
pub fn check(e: &Expr, xlen: Xlen) -> Vec<Found> {
    let mut c = Checker {
        tree: Tree {
            nodes: vec![(0, false)],
        },
        region: 0,
        usages: HashMap::new(),
        side_effects: None,
        found: Vec::new(),
        xlen,
    };
    c.value(e);
    c.found
}

/// The variable `e` designates, through casts, as a modification's target
/// (clang's `getObject`): through a comma's right operand, and for a
/// modification an assignment's target and `++x`'s operand.
fn object(e: &Expr, modified: bool) -> Option<(VarRef, String)> {
    match &e.kind {
        ExprKind::Cast { x, .. } => object(x, modified),
        ExprKind::Comma(_, b) => object(b, modified),
        ExprKind::Assign { target, .. } if modified => object(target, modified),
        ExprKind::IncDec {
            prefix: true, target, ..
        } if modified => object(target, modified),
        ExprKind::Var { name, var: Some(v) } => Some((v.clone(), name.clone())),
        _ => None,
    }
}

impl Checker {
    fn usages(&mut self, o: &(VarRef, String)) -> &mut Usages {
        &mut self
            .usages
            .entry(o.0.clone())
            .or_insert_with(|| (o.1.clone(), Usages::default()))
            .1
    }

    /// clang's `checkUsage`: the usage `cur` of `o` against its earlier
    /// one of `other` kind, if they are unsequenced.
    fn conflict(&mut self, o: &(VarRef, String), cur: Usage, other: Kind, mod_mod: bool) {
        let region = self.region;
        let tree = &self.tree;
        let u = &mut self
            .usages
            .entry(o.0.clone())
            .or_insert_with(|| (o.1.clone(), Usages::default()))
            .1;
        if u.diagnosed {
            return;
        }
        let Some(prev) = u.uses[other as usize] else {
            return;
        };
        if !tree.unsequenced(region, prev.region) {
            return;
        }
        // At the modification: the earlier usage, unless it is the read.
        let m = if other == Kind::Use { cur } else { prev };
        let msg = if mod_mod {
            format!("multiple unsequenced modifications to '{}'", o.1)
        } else {
            format!("unsequenced modification and access to '{}'", o.1)
        };
        u.diagnosed = true;
        self.found.push(Found {
            at: m.at,
            of: m.of,
            msg,
        });
    }

    /// clang's `addUsage`: notes `cur` as the usage of its kind, unless the
    /// one there is unsequenced with it.
    fn add(&mut self, o: &(VarRef, String), cur: Usage, kind: Kind) {
        let region = self.region;
        let prev = self.usages(o).uses[kind as usize];
        if prev.is_some_and(|p| self.tree.unsequenced(region, p.region)) {
            return;
        }
        if kind == Kind::ModAsSideEffect {
            if let Some(saved) = &mut self.side_effects {
                saved.push((o.0.clone(), prev));
            }
        }
        self.usages(o).uses[kind as usize] = Some(cur);
    }

    fn usage(&self, e: &Expr, at: Pos) -> Usage {
        Usage {
            at,
            of: e.id,
            region: self.region,
        }
    }

    fn pre_use(&mut self, o: &(VarRef, String), u: Usage) {
        self.conflict(o, u, Kind::ModAsValue, false);
    }

    fn post_use(&mut self, o: &(VarRef, String), u: Usage) {
        self.conflict(o, u, Kind::ModAsSideEffect, false);
        self.add(o, u, Kind::Use);
    }

    fn pre_mod(&mut self, o: &(VarRef, String), u: Usage) {
        self.conflict(o, u, Kind::ModAsValue, true);
        self.conflict(o, u, Kind::Use, false);
    }

    fn post_mod(&mut self, o: &(VarRef, String), u: Usage, kind: Kind) {
        self.conflict(o, u, Kind::ModAsSideEffect, true);
        self.add(o, u, kind);
    }

    /// Walks `f` as a sequenced subexpression: its side effects become
    /// values once it is done (clang's `SequencedSubexpression`).
    fn sequenced(&mut self, f: impl FnOnce(&mut Self)) {
        let outer = self.side_effects.replace(Vec::new());
        f(self);
        let saved = std::mem::replace(&mut self.side_effects, outer).unwrap_or_default();
        for (v, prev) in saved.into_iter().rev() {
            let (name, side) = {
                let entry = self.usages.get(&v).expect("noted");
                (entry.0.clone(), entry.1.uses[Kind::ModAsSideEffect as usize])
            };
            if let Some(side) = side {
                self.add(&(v.clone(), name), side, Kind::ModAsValue);
            }
            if let Some(entry) = self.usages.get_mut(&v) {
                entry.1.uses[Kind::ModAsSideEffect as usize] = prev;
            }
        }
    }

    /// `first`, then `then`, each in a region of its own, `first` a
    /// sequenced subexpression (`,`).
    fn in_order(&mut self, first: &Expr, then: &Expr) {
        let (before, after, outer) = (
            self.tree.allocate(self.region),
            self.tree.allocate(self.region),
            self.region,
        );
        self.region = before;
        self.sequenced(|c| c.value(first));
        self.region = after;
        self.value(then);
        self.region = outer;
        self.tree.merge(before);
        self.tree.merge(after);
    }

    /// `e`'s value: what it reads and modifies.
    fn value(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Var { .. } if !e.ty.is_array() && !e.ty.is_func() => {
                if let Some(o) = object(e, false) {
                    let u = self.usage(e, e.pos);
                    self.pre_use(&o, u);
                    self.post_use(&o, u);
                }
            }
            ExprKind::Cast { x, .. } => self.value(x),
            ExprKind::Assign { op, target, value } => {
                let o = object(target, true);
                let u = self.usage(e, e.pos);
                if let Some(o) = &o {
                    self.pre_mod(o, u);
                }
                self.place(target);
                // `t op= v` reads `t` too.
                if let (Some(o), Some(_)) = (&o, op) {
                    self.post_use(o, u);
                }
                self.value(value);
                if let Some(o) = &o {
                    self.post_mod(o, u, Kind::ModAsSideEffect);
                }
            }
            ExprKind::IncDec { target, .. } => {
                let Some(o) = object(target, true) else {
                    return self.place(target);
                };
                let u = self.usage(e, e.pos);
                self.pre_mod(&o, u);
                self.place(target);
                self.post_mod(&o, u, Kind::ModAsSideEffect);
            }
            ExprKind::Binary(op @ (BinOp::LogAnd | BinOp::LogOr), l, r) => {
                let (left, right, outer) = (
                    self.tree.allocate(self.region),
                    self.tree.allocate(self.region),
                    self.region,
                );
                self.region = left;
                self.sequenced(|c| c.value(l));
                // The right operand, unless the left one decides it.
                let decided = consteval::int(l, self.xlen).map(|v| v != 0);
                if decided != Some(*op == BinOp::LogOr) {
                    self.region = right;
                    self.value(r);
                }
                self.region = outer;
                self.tree.merge(left);
                self.tree.merge(right);
            }
            ExprKind::Comma(a, b) => self.in_order(a, b),
            ExprKind::Cond(c, a, b) => {
                let outer = self.region;
                let (cond, yes, no) = (
                    self.tree.allocate(outer),
                    self.tree.allocate(outer),
                    self.tree.allocate(outer),
                );
                self.region = cond;
                self.sequenced(|s| s.value(c));
                let decided = consteval::int(c, self.xlen).map(|v| v != 0);
                if decided != Some(false) {
                    self.region = yes;
                    self.value(a);
                }
                if decided != Some(true) {
                    self.region = no;
                    self.value(b);
                }
                self.region = outer;
                self.tree.merge(cond);
                self.tree.merge(yes);
                self.tree.merge(no);
            }
            ExprKind::Addr(x) => self.place(x),
            ExprKind::Member { base, arrow, .. } => {
                if *arrow {
                    self.value(base)
                } else {
                    self.place(base)
                }
            }
            ExprKind::Call { callee, args, .. } => self.sequenced(|c| {
                c.value(callee);
                for a in args {
                    c.value(a);
                }
            }),
            ExprKind::SizeofExpr(..) | ExprKind::SizeofType(_) => {}
            ExprKind::Binary(_, a, b) | ExprKind::VaCopy(a, b) => {
                self.value(a);
                self.value(b);
            }
            ExprKind::VaStart { ap, last } => {
                self.value(ap);
                self.value(last);
            }
            ExprKind::Unary(_, x) | ExprKind::Deref(x) | ExprKind::VaArg(x, _) | ExprKind::VaEnd(x) => {
                self.value(x)
            }
            _ => {}
        }
    }

    /// `e` as the object an assignment, `++`, `&` or `.` is of: not read
    /// itself, what designates it is.
    fn place(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Var { .. } => {}
            ExprKind::Cast {
                x, implicit: true, ..
            } => self.place(x),
            ExprKind::Member {
                base, arrow: false, ..
            } => self.place(base),
            _ => self.value(e),
        }
    }
}
