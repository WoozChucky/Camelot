//! What merlin-dbg knows of the Camelot kernel: its processes, read from the kernel's
//! own data with the kernel's debugging information as a guide, as GDB's Linux scripts
//! (`scripts/gdb/linux`, `lx-ps`) read the task list with the kernel's types, rather
//! than from offsets written down here. Only names are assumed: the process table
//! `procs` (`kernel/proc.c`, `struct proc`, with its `state`, `pid`, `name` and
//! `pagetable`), and `cpus` (`kernel/spinlock.c`), each hart's `struct cpu` at its
//! hart id, whose `proc` is the process running on it (NULL while the hart runs the
//! scheduler). The state numbers are `kernel/include/proc.h`'s `PROC_` constants,
//! which the debugging information cannot carry (they are `#define`s): a test checks
//! the table below against the header.

use crate::vars::{Eval, Target};

type Result<T> = std::result::Result<T, String>;

/// `kernel/include/proc.h`'s process states, by number.
pub const STATES: [(u64, &str); 7] = [
    (0, "unused"),
    (1, "runnable"),
    (2, "running"),
    (3, "zombie"),
    (4, "sleeping"),
    (5, "new"),
    (6, "live"),
];

/// A state's name, or its number when the table has none.
pub fn state_name(state: u64) -> String {
    STATES
        .iter()
        .find(|(n, _)| *n == state)
        .map_or_else(|| state.to_string(), |(_, name)| name.to_string())
}

/// A slot of the process table that is in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    /// Its index in `procs`.
    pub slot: usize,
    pub pid: u64,
    pub state: u64,
    /// What it runs: its program's name, as exec named it (the path's last name).
    pub name: String,
    /// The kernel's pointer to its root page table (a kernel virtual address); 0 for
    /// a kernel process, which runs on the kernel's table.
    pub pagetable: u64,
}

fn missing(e: String) -> String {
    format!("no Camelot process table: {e} (the kernel's ELF, built with merlin -g, gives it)")
}

fn number<T: Target>(eval: &mut Eval<'_, T>, expr: &str) -> Result<u64> {
    eval.integer(expr).map(|n| n as u64)
}

/// A NUL-terminated name in a character array.
fn name<T: Target>(eval: &mut Eval<'_, T>, expr: &str) -> Result<String> {
    let bytes = eval.object(expr)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

/// The processes in the table, in slot order.
pub fn processes<T: Target>(eval: &mut Eval<'_, T>) -> Result<Vec<Process>> {
    let slots = number(eval, "sizeof procs / sizeof procs[0]").map_err(missing)?;
    let mut out = Vec::new();
    for slot in 0..slots as usize {
        let state = number(eval, &format!("procs[{slot}].state"))?;
        if state == 0 {
            continue;
        }
        out.push(Process {
            slot,
            pid: number(eval, &format!("procs[{slot}].pid"))?,
            state,
            name: name(eval, &format!("procs[{slot}].name"))?,
            pagetable: number(eval, &format!("procs[{slot}].pagetable"))?,
        });
    }
    Ok(out)
}

/// The process running on hart `hart` (`cpus[hart].proc`): its pid and name, or
/// `None` while the hart runs for the kernel itself (the scheduler, kmain).
pub fn current<T: Target>(eval: &mut Eval<'_, T>, hart: usize) -> Result<Option<(u64, String)>> {
    let proc = number(eval, &format!("cpus[{hart}].proc")).map_err(missing)?;
    if proc == 0 {
        return Ok(None);
    }
    let pid = number(eval, &format!("cpus[{hart}].proc->pid"))?;
    Ok(Some((pid, name(eval, &format!("cpus[{hart}].proc->name"))?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state names are proc.h's `PROC_` constants, number for number: the ones
    /// of its "Process states" block, which ends at the thread states.
    #[test]
    fn the_states_are_the_kernels() {
        let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/include/proc.h");
        let header = std::fs::read_to_string(header).unwrap();
        let defined: Vec<(u64, String)> = header
            .lines()
            .skip_while(|l| !l.starts_with("/* Process states."))
            .take_while(|l| !l.starts_with("/* Thread states:"))
            .filter_map(|l| l.strip_prefix("#define PROC_"))
            .filter_map(|l| {
                let mut words = l.split_whitespace();
                let name = words.next()?.to_lowercase();
                let value = words.next()?.parse().ok()?;
                Some((value, name))
            })
            .collect();
        let ours: Vec<(u64, String)> = STATES.iter().map(|(n, s)| (*n, s.to_string())).collect();
        assert_eq!(defined, ours);
        assert_eq!(state_name(6), "live");
        assert_eq!(state_name(9), "9");
    }
}
