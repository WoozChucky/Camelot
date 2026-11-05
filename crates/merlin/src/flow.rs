//! What of a function can run, as clang's analysis-based warnings see it
//! (`clang/lib/Analysis/CFG.cpp`, `clang/lib/Sema/AnalysisBasedWarnings.cpp`).
//!
//! clang builds a control-flow graph of each function body, leaving out
//! the edges a condition it can evaluate never takes (its CFG builder's
//! `tryEvaluateBool`: `if (0)`, `while (1)`'s exit, the operand of `0 && x`
//! or of a constant `?:` that is not chosen, the cases of a `switch` on a
//! constant but the matching one), and asks two things of it:
//!
//! * Which statements are reachable from the entry. The warnings of what
//!   the code does when it runs (`DiagRuntimeBehavior`: dividing by zero, a
//!   shift too far) are given only for those ([`Flow::live`]).
//! * Whether the end of the body is reachable by falling off it, and
//!   whether a `return` is: `-Wreturn-type` ([`Flow::falls_off`],
//!   [`Flow::returns`]).
//!
//! A label is reachable if a reachable `goto` names it, which may come after
//! it: the walk is repeated until the labels it found reached stop growing.

use crate::ast::*;
use crate::consteval;
use crate::types::Xlen;

use std::collections::HashSet;

/// What [`analyze`] finds of a function body.
pub struct Flow {
    /// The expressions that may be evaluated, by [`ExprId`]: not by
    /// position, which all of a macro's expansion shares.
    pub live: HashSet<u64>,
    /// Whether control can fall off the end of the body.
    pub falls_off: bool,
    /// Whether a `return` can be reached.
    pub returns: bool,
}

/// Walks a function's checked body.
pub fn analyze(body: &[Stmt], x: Xlen) -> Flow {
    let mut labels = HashSet::new();
    loop {
        let mut w = Walk {
            x,
            labels: &labels,
            gotos: HashSet::new(),
            live: HashSet::new(),
            returns: false,
            breaks: Vec::new(),
            continues: Vec::new(),
            switches: Vec::new(),
        };
        let falls_off = w.block(body, true);
        if w.gotos.is_subset(&labels) {
            return Flow {
                live: w.live,
                falls_off,
                returns: w.returns,
            };
        }
        labels.extend(w.gotos);
    }
}

/// The value clang's CFG builder gives a condition, if it can: its
/// `tryEvaluateBool`. Of `&&` and `||`, one operand may decide it (`x || 1`
/// is true); otherwise, whatever constant folding gives (an object's
/// address is not null).
fn truth(e: &Expr, x: Xlen) -> Option<bool> {
    if let ExprKind::Binary(op @ (BinOp::LogAnd | BinOp::LogOr), l, r) = &e.kind {
        let or = *op == BinOp::LogOr;
        return match (truth(l, x), truth(r, x)) {
            (Some(a), _) if a == or => Some(a),
            (Some(_), Some(b)) => Some(b),
            (None, Some(b)) if b == or => Some(b),
            _ => None,
        };
    }
    if e.ty.is_integer() {
        return consteval::condition(e, x).map(|v| v != 0);
    }
    match consteval::eval(e, x)? {
        consteval::Const::Int(v, _) => Some(v != 0),
        consteval::Const::Addr { base: Some(_), .. } => Some(true),
        consteval::Const::Addr { base: None, offset } => Some(offset != 0),
    }
}

/// One walk of a body, with the labels found reached so far.
struct Walk<'a> {
    x: Xlen,
    labels: &'a HashSet<String>,
    /// The labels a reachable `goto` names.
    gotos: HashSet<String>,
    live: HashSet<u64>,
    returns: bool,
    /// For each loop or `switch` around, whether a reachable `break` leaves
    /// it; for each loop, whether a reachable `continue` goes on with it.
    breaks: Vec<bool>,
    continues: Vec<bool>,
    /// For each `switch` around, which of its labels it jumps to.
    switches: Vec<Entry>,
}

/// Which labels of a `switch` it jumps to.
#[derive(Clone, Copy)]
enum Entry {
    /// Any: the condition is not constant; and `default`, unless the
    /// switch covers every constant of its enum.
    All { default: bool },
    /// The case of this value, if one has it (the bool), else `default`.
    Only(i128, bool),
    /// None: the switch is not reached.
    Nothing,
}

impl Walk<'_> {
    /// The statements of a block; whether control comes out of it.
    fn block(&mut self, body: &[Stmt], live: bool) -> bool {
        body.iter().fold(live, |live, s| self.stmt(s, live))
    }

    /// A statement reached if `live`; whether control comes out of it at
    /// its end.
    fn stmt(&mut self, s: &Stmt, live: bool) -> bool {
        match &s.kind {
            StmtKind::Block(body) => self.block(body, live),
            StmtKind::Unevaluated(_) | StmtKind::Empty { .. } => live,
            StmtKind::Decl(decls) => {
                for d in decls {
                    if let Some(init) = &d.init {
                        self.init(init, live);
                    }
                }
                live
            }
            StmtKind::Expr(e) => {
                self.expr(e, live);
                live
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e, live);
                }
                self.returns |= live;
                false
            }
            StmtKind::If { cond, then, els } => {
                self.expr(cond, live);
                let t = truth(cond, self.x);
                let then_out = self.stmt(then, live && t != Some(false));
                let else_live = live && t != Some(true);
                let else_out = match els {
                    Some(e) => self.stmt(e, else_live),
                    None => else_live,
                };
                then_out || else_out
            }
            StmtKind::While { cond, body } => {
                self.expr(cond, live);
                let t = truth(cond, self.x);
                let (_, broke, _) = self.in_loop(|w| w.stmt(body, live && t != Some(false)));
                (live && t != Some(true)) || broke
            }
            StmtKind::DoWhile { body, cond } => {
                let (out, broke, went_on) = self.in_loop(|w| w.stmt(body, live));
                let cond_live = out || went_on;
                self.expr(cond, cond_live);
                (cond_live && truth(cond, self.x) != Some(true)) || broke
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
                ..
            } => {
                let live = match init {
                    Some(i) => self.stmt(i, live),
                    None => live,
                };
                let t = match cond {
                    Some(c) => {
                        self.expr(c, live);
                        truth(c, self.x)
                    }
                    None => Some(true),
                };
                let (out, broke, went_on) = self.in_loop(|w| w.stmt(body, live && t != Some(false)));
                if let Some(e) = step {
                    self.expr(e, out || went_on);
                }
                (live && t != Some(true)) || broke
            }
            StmtKind::Switch {
                cond,
                body,
                cases,
                default,
                covered,
            } => {
                self.expr(cond, live);
                let v = consteval::int(cond, self.x);
                // A switch that covers its enum takes a case or nothing:
                // clang's CFG has no edge to `default`, nor past the switch.
                let matched = *covered || v.is_some_and(|v| cases.iter().any(|&(c, _)| c == v));
                self.switches.push(match v {
                    _ if !live => Entry::Nothing,
                    None => Entry::All { default: !covered },
                    Some(v) => Entry::Only(v, matched),
                });
                self.breaks.push(false);
                // Nothing before the first case runs but what a label reaches.
                let out = self.stmt(body, false);
                let broke = self.breaks.pop().expect("pushed");
                self.switches.pop();
                // With no case taken, the switch is passed by.
                let passed = live && !covered && default.is_none() && (v.is_none() || !matched);
                out || broke || passed
            }
            StmtKind::Case { value, body, .. } => {
                let entry = match self.switches.last() {
                    Some(Entry::All { .. }) => true,
                    Some(&Entry::Only(v, _)) => v == *value,
                    Some(Entry::Nothing) | None => false,
                };
                self.stmt(body, live || entry)
            }
            StmtKind::Default { body, .. } => {
                let entry = match self.switches.last() {
                    Some(&Entry::All { default }) => default,
                    Some(&Entry::Only(_, matched)) => !matched,
                    Some(Entry::Nothing) | None => false,
                };
                self.stmt(body, live || entry)
            }
            StmtKind::Label { name, body } => {
                let reached = self.labels.contains(name);
                self.stmt(body, live || reached)
            }
            StmtKind::Goto(name, _) => {
                if live {
                    self.gotos.insert(name.clone());
                }
                false
            }
            StmtKind::Break => {
                if let Some(b) = self.breaks.last_mut() {
                    *b |= live;
                }
                false
            }
            StmtKind::Continue => {
                if let Some(c) = self.continues.last_mut() {
                    *c |= live;
                }
                false
            }
        }
    }

    /// A loop's body, walked by `f`: whether control comes out of its end,
    /// whether a `break` leaves the loop, and whether a `continue` goes on
    /// with it.
    fn in_loop(&mut self, f: impl FnOnce(&mut Self) -> bool) -> (bool, bool, bool) {
        self.breaks.push(false);
        self.continues.push(false);
        let out = f(self);
        let went_on = self.continues.pop().expect("pushed");
        (out, self.breaks.pop().expect("pushed"), went_on)
    }

    fn init(&mut self, init: &Init, live: bool) {
        match init {
            Init::Expr(e) => self.expr(e, live),
            Init::List(items, _) => items.iter().for_each(|i| self.init(i, live)),
            Init::Designated(_, i) => self.init(i, live),
        }
    }

    /// An expression evaluated if `live`, and what in it is.
    fn expr(&mut self, e: &Expr, live: bool) {
        if live {
            self.live.insert(e.id.0);
        }
        match &e.kind {
            ExprKind::Binary(op @ (BinOp::LogAnd | BinOp::LogOr), a, b) => {
                self.expr(a, live);
                let t = truth(a, self.x);
                let decided = if *op == BinOp::LogAnd {
                    Some(false)
                } else {
                    Some(true)
                };
                self.expr(b, live && t != decided);
            }
            ExprKind::Cond(c, a, b) => {
                self.expr(c, live);
                let t = truth(c, self.x);
                self.expr(a, live && t != Some(false));
                self.expr(b, live && t != Some(true));
            }
            ExprKind::Binary(_, a, b) | ExprKind::Comma(a, b) | ExprKind::VaCopy(a, b) => {
                self.expr(a, live);
                self.expr(b, live);
            }
            ExprKind::Assign { target, value, .. } => {
                self.expr(target, live);
                self.expr(value, live);
            }
            ExprKind::VaStart { ap, last } => {
                self.expr(ap, live);
                self.expr(last, live);
            }
            ExprKind::IncDec { target: x, .. }
            | ExprKind::Unary(_, x)
            | ExprKind::Addr(x)
            | ExprKind::Deref(x)
            | ExprKind::Cast { x, .. }
            | ExprKind::Member { base: x, .. }
            | ExprKind::VaArg(x, _)
            | ExprKind::VaEnd(x) => self.expr(x, live),
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee, live);
                args.iter().for_each(|a| self.expr(a, live));
            }
            ExprKind::CompoundLit { init, .. } => self.init(init, live),
            // `sizeof`'s operand is never evaluated.
            ExprKind::SizeofExpr(..)
            | ExprKind::SizeofType(_)
            | ExprKind::Num(_)
            | ExprKind::Enumerator { .. }
            | ExprKind::Str(_)
            | ExprKind::Var { .. } => {}
        }
    }
}
