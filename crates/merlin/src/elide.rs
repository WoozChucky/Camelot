//! The statements a constant condition leaves out, as clang's code generator
//! leaves them out at `-O0` (`clang/lib/CodeGen/CGStmt.cpp`).
//!
//! clang decides a condition at compile time when it is an integer constant
//! expression (`ConstantFoldsToSimpleInteger`). Then `if (0) a; else b;`
//! emits only `b`, and `switch (2) { case 1: a(); case 2: b(); break; }` only
//! `b();`. Code that cannot be reached by falling into it is still emitted if
//! a jump from elsewhere can reach it: a statement with a `goto` label, or a
//! `case` or `default` of a `switch` around it, is kept (`ContainsLabel`).
//! The functions here answer the questions clang asks of the statements
//! ([`contains_label`], [`contains_break`], [`might_add_decl`]) and pick the
//! statements a `switch` on a constant runs ([`case_statements`]); code
//! generation does the rest.
//!
//! merlin's tree has no statement for a declaration that only declares
//! types (`struct s { int a; };`, `typedef int T;`): the parser leaves it
//! out, so it counts for nothing here, where clang's `DeclStmt` would.

use crate::ast::{Stmt, StmtKind};

/// The statements directly inside `s`.
fn children(s: &Stmt) -> Vec<&Stmt> {
    match &s.kind {
        StmtKind::Block(body) => body.iter().collect(),
        StmtKind::If { then, els, .. } => std::iter::once(&**then).chain(els.as_deref()).collect(),
        StmtKind::While { body, .. }
        | StmtKind::DoWhile { body, .. }
        | StmtKind::Switch { body, .. }
        | StmtKind::Case { body, .. }
        | StmtKind::Default { body, .. }
        | StmtKind::Label { body, .. } => vec![body],
        StmtKind::For { init, body, .. } => init.as_deref().into_iter().chain([&**body]).collect(),
        StmtKind::Decl(_)
        | StmtKind::Expr(_)
        | StmtKind::Empty
        | StmtKind::Unevaluated(_)
        | StmtKind::Return(_)
        | StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Goto(_) => Vec::new(),
    }
}

/// Whether `s` has a label a jump from outside it can reach: a `goto`
/// label, or, unless `ignore_cases`, a `case` or `default` (those of a
/// `switch` inside `s` are that switch's). clang's `ContainsLabel`.
pub fn contains_label(s: &Stmt, ignore_cases: bool) -> bool {
    match &s.kind {
        StmtKind::Label { .. } => true,
        StmtKind::Case { .. } | StmtKind::Default { .. } if !ignore_cases => true,
        StmtKind::Switch { body, .. } => contains_label(body, true),
        _ => children(s).into_iter().any(|c| contains_label(c, ignore_cases)),
    }
}

/// Whether `s` has a `break` that leaves the statement around it: one not
/// inside a loop or `switch` of its own. clang's `containsBreak`.
pub fn contains_break(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Switch { .. }
        | StmtKind::While { .. }
        | StmtKind::DoWhile { .. }
        | StmtKind::For { .. } => false,
        StmtKind::Break => true,
        _ => children(s).into_iter().any(contains_break),
    }
}

/// Whether `s` may declare a variable in the scope it is in: a declaration,
/// or a labelled one. A statement that is a scope of its own declares
/// nothing outside it. clang's `mightAddDeclToScope`.
pub fn might_add_decl(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::If { .. }
        | StmtKind::Switch { .. }
        | StmtKind::While { .. }
        | StmtKind::DoWhile { .. }
        | StmtKind::For { .. }
        | StmtKind::Block(_) => false,
        StmtKind::Decl(_) => true,
        _ => children(s).into_iter().any(might_add_decl),
    }
}

/// The statements a `switch` with the constant condition `value` runs, in
/// order, when they can be picked out of its `body` (clang's
/// `FindCaseStatementsForValue`): the `case` of that value (or `default`,
/// as `cases` and `default` give their ids, sema's), what follows it up to
/// the `break` that ends the switch, without the labels. Empty when no
/// `case` matches and there is no `default`. `None` when they cannot be
/// emitted alone, and the whole `switch` is generated as usual: a label
/// among the statements skipped, a `break` nested in what runs, or a
/// declaration whose scope would be lost.
pub fn case_statements<'s>(
    body: &'s Stmt,
    value: i128,
    cases: &[(i128, usize)],
    default: Option<usize>,
) -> Option<Vec<&'s Stmt>> {
    let Some(case) = cases
        .iter()
        .find(|&&(v, _)| v == value)
        .map(|&(_, id)| id)
        .or(default)
    else {
        // Nothing runs: the body can go if nothing can jump into it.
        return (!contains_label(body, true)).then(Vec::new);
    };
    let (mut found, mut out) = (false, Vec::new());
    match collect(body, Some(case), &mut found, &mut out) {
        Collect::Failure => None,
        _ => found.then_some(out),
    }
}

/// What [`collect`] found in a statement (clang's `CSFC_Result`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Collect {
    /// The statements cannot be picked out.
    Failure,
    /// The statement is skipped (not reached), or ends with the `break`.
    Success,
    /// The statement runs and falls through to the next.
    FallThrough,
}

/// clang's `CollectStatementsForCase`: looking for the `case` with id `case`
/// (`None` once found, in the statements that run), adds to `out` what runs
/// of `s`.
fn collect<'s>(s: &'s Stmt, case: Option<usize>, found: &mut bool, out: &mut Vec<&'s Stmt>) -> Collect {
    use Collect::*;
    match &s.kind {
        // The case looked for: what it labels runs. Any other is skipped
        // over, its statement looked into.
        StmtKind::Case { id, body, .. } | StmtKind::Default { id, body } => {
            if case == Some(*id) {
                *found = true;
                return collect(body, None, found, out);
            }
            return collect(body, case, found, out);
        }
        // The `break` that ends what runs.
        StmtKind::Break if case.is_none() => return Success,
        StmtKind::Block(body) => return collect_block(s, body, case, found, out),
        _ => {}
    }
    // Any other statement: skipped if it has no label, or run whole if it
    // has no `break` of the switch.
    if case.is_some() {
        return if contains_label(s, true) { Failure } else { Success };
    }
    if contains_break(s) {
        return Failure;
    }
    out.push(s);
    FallThrough
}

/// [`collect`] of a block: the statements before the case are skipped (they
/// must have no label, and no declaration the rest could use), those from
/// it on run until the `break`, and those after it are skipped.
fn collect_block<'s>(
    s: &'s Stmt,
    body: &'s [Stmt],
    mut case: Option<usize>,
    found: &mut bool,
    out: &mut Vec<&'s Stmt>,
) -> Collect {
    use Collect::*;
    let started_live = *found;
    let start = out.len();
    let skippable = |rest: &[Stmt]| {
        if rest.iter().any(|s| contains_label(s, true)) {
            Failure
        } else {
            Success
        }
    };
    let mut i = 0;
    if case.is_some() {
        let mut skipped_decl = false;
        while case.is_some() && i < body.len() {
            skipped_decl |= might_add_decl(&body[i]);
            match collect(&body[i], case, found, out) {
                Failure => return Failure,
                // The case and its `break` both in that statement: the rest
                // is skipped.
                Success if *found => {
                    return if skipped_decl {
                        Failure
                    } else {
                        skippable(&body[i + 1..])
                    };
                }
                Success => {}
                // The case found: the statements after it run.
                FallThrough => {
                    case = None;
                    if skipped_decl {
                        return Failure;
                    }
                }
            }
            i += 1;
        }
        if !*found {
            return Success;
        }
    }
    let mut any_decls = false;
    while i < body.len() {
        any_decls |= might_add_decl(&body[i]);
        match collect(&body[i], None, found, out) {
            Failure => return Failure,
            FallThrough => {}
            Success => return skippable(&body[i + 1..]),
        }
        i += 1;
    }
    // Falling out of the block: its declarations' scope must stay, so the
    // block runs whole, if all of it runs and no `break` is in it.
    if any_decls {
        if started_live && !contains_break(s) {
            out.truncate(start);
            out.push(s);
        } else {
            return Failure;
        }
    }
    FallThrough
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The statements of `f`'s body, `f` being `void f(int x)` with `body`.
    fn stmts(body: &str) -> Vec<Stmt> {
        let src = format!("int a(void); int b(void);\nvoid f(int x) {{ {body} }}");
        let p = crate::parse(&src).unwrap();
        let f = p.funcs().find(|f| f.body.is_some()).unwrap();
        f.body.clone().unwrap()
    }

    /// How many statements `switch (value) body` runs, if they can be
    /// picked out.
    fn picked(body: &str, value: i128) -> Option<usize> {
        let s = stmts(&format!("switch (x) {body}"));
        let StmtKind::Switch {
            body, cases, default, ..
        } = &s[0].kind
        else {
            unreachable!()
        };
        case_statements(body, value, cases, *default).map(|v| v.len())
    }

    #[test]
    fn the_case_up_to_its_break() {
        let body = "{ case 1: a(); break; case 2: b(); case 3: a(); break; default: b(); }";
        assert_eq!(picked(body, 1), Some(1));
        assert_eq!(picked(body, 2), Some(2), "falls through into case 3");
        assert_eq!(picked(body, 7), Some(1), "default");
        assert_eq!(picked("{ case 1: a(); }", 7), Some(0), "nothing runs");
        assert_eq!(picked("{ case 1: case 2: a(); b(); }", 1), Some(2));
    }

    #[test]
    fn what_cannot_be_picked_out() {
        // A label skipped over could be jumped to.
        assert_eq!(picked("{ case 1: a(); break; l: b(); }", 1), None);
        assert_eq!(picked("{ l: a(); case 1: b(); }", 1), None);
        assert_eq!(
            picked("{ l: a(); }", 1),
            None,
            "nothing runs, but l can be reached"
        );
        // A break that is not the end of what runs.
        assert_eq!(picked("{ case 1: if (x) break; a(); }", 1), None);
        // A declaration skipped, which the case's statements could use.
        assert_eq!(picked("{ int y; case 1: a(); }", 1), None);
        // A block with a declaration, which runs whole, but has a break.
        assert_eq!(picked("{ case 1: { int y = 1; if (y) break; } }", 1), None);
    }

    #[test]
    fn a_block_with_declarations_runs_whole() {
        // The case's block declares y: it runs as a block, keeping y's scope.
        assert_eq!(picked("{ case 1: { int y = 1; x = y; } }", 1), Some(1));
        // A loop's break is its own.
        assert_eq!(picked("{ case 1: while (x) break; a(); break; }", 1), Some(2));
    }

    #[test]
    fn labels_breaks_and_declarations() {
        let s = stmts("switch (x) { case 1: x = 2; } while (x) { break; } l: ; if (x) { m: ; } int y; ");
        assert!(!contains_label(&s[0], false), "a switch's cases are its own");
        assert!(!contains_break(&s[1]), "a loop's break is its own");
        assert!(contains_label(&s[2], false));
        assert!(contains_label(&s[3], true), "a label in an if");
        assert!(might_add_decl(&s[4]));
        assert!(!might_add_decl(&s[3]));
        let case = stmts("switch (x) { case 1: a(); }");
        let StmtKind::Switch { body, .. } = &case[0].kind else {
            unreachable!()
        };
        assert!(contains_label(body, false), "a case, outside its switch");
        assert!(!contains_label(body, true));
        let brk = stmts("while (x) { if (x) break; }");
        let StmtKind::While { body, .. } = &brk[0].kind else {
            unreachable!()
        };
        assert!(contains_break(body));
    }
}
