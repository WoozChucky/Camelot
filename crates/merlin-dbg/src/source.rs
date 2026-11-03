//! Source lines: which line of which file each address comes from, from the
//! programs' DWARF line tables (`.debug_line`, read through `elf::lines`), and
//! the source files themselves, for showing a line.
//!
//! A line table is a list of *rows*, each an address and the place in the source
//! the code from there on comes from, until the next row. A row marked `is_stmt`
//! starts a statement: that is where a breakpoint on a line goes and where
//! stepping by line stops. merlin writes one before each statement, a function's
//! first line before its prologue, and marks the first statement after the
//! prologue `prologue_end` (merlin's `debug` module).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A row, with its file as an index into [`Lines::files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    addr: u32,
    file: usize,
    line: u32,
    is_stmt: bool,
    prologue_end: bool,
}

/// A run of code and its rows, in address order: `start..end`.
#[derive(Debug, Clone)]
struct Run {
    start: u32,
    end: u32,
    rows: Vec<Row>,
}

/// Where an address comes from: a file (its path as the line table gives it) and
/// a line.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    pub file: String,
    pub line: u32,
}

impl std::fmt::Display for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

#[derive(Default)]
pub struct Lines {
    /// Every file named, once each: paths with `/`.
    files: Vec<String>,
    /// The runs of every program loaded, by start address.
    runs: Vec<Run>,
    /// Where to look for source files (`directory`), before the current directory.
    dirs: Vec<PathBuf>,
    /// Source files read so far, by path (`None`: not found).
    texts: HashMap<String, Option<Vec<String>>>,
}

/// Parses an ELF file merlin-dbg can debug: an RV32 one (ELF32), whose
/// addresses all fit in 32 bits, until merlin-dbg follows Excalibur to RV64.
pub(crate) fn parse_elf32(file: &[u8]) -> Result<elf::Elf<'_>, String> {
    let elf = elf::Elf::parse(file).map_err(|e| e.to_string())?;
    if elf.class != elf::Class::Elf32 {
        return Err("ELF64 (RV64) programs are not supported yet".into());
    }
    Ok(elf)
}

impl Lines {
    /// Adds the line tables of an executable (none if it has no `.debug_line`).
    pub fn add_elf(&mut self, file: &[u8]) -> Result<(), String> {
        let elf = parse_elf32(file)?;
        for table in elf.line_tables().map_err(|e| e.to_string())? {
            let ids: Vec<usize> = table
                .files
                .iter()
                .map(|f| {
                    let f = f.replace('\\', "/");
                    match self.files.iter().position(|g| *g == f) {
                        Some(i) => i,
                        None => {
                            self.files.push(f);
                            self.files.len() - 1
                        }
                    }
                })
                .collect();
            for seq in &table.sequences {
                let (Some(first), Some(last)) = (seq.rows.first(), seq.rows.last()) else {
                    continue;
                };
                let rows = seq
                    .rows
                    .iter()
                    .filter(|r| !r.end_sequence)
                    .map(|r| Row {
                        addr: r.addr as u32,
                        file: ids.get(r.file).copied().unwrap_or(usize::MAX),
                        line: r.line,
                        is_stmt: r.is_stmt,
                        prologue_end: r.prologue_end,
                    })
                    .collect();
                // The code of a sequence that starts at 0 was thrown away by
                // the linker (which writes 0 for its address): no program runs
                // at 0, the page every pointer check keeps unmapped.
                if first.addr == 0 {
                    continue;
                }
                self.runs.push(Run {
                    start: first.addr as u32,
                    end: last.addr as u32,
                    rows,
                });
            }
        }
        self.runs.sort_by_key(|r| r.start);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    fn place_of(&self, row: &Row) -> Place {
        Place {
            file: self.files.get(row.file).cloned().unwrap_or_else(|| "?".into()),
            line: row.line,
        }
    }

    /// The row in effect at `addr`.
    fn row(&self, addr: u32) -> Option<&Row> {
        let i = self.runs.partition_point(|r| r.start <= addr).checked_sub(1)?;
        let run = self.runs[..=i]
            .iter()
            .rev()
            .find(|r| (r.start..r.end).contains(&addr))?;
        let j = run.rows.partition_point(|r| r.addr <= addr).checked_sub(1)?;
        Some(&run.rows[j])
    }

    /// Where the code at `addr` comes from.
    pub fn at(&self, addr: u32) -> Option<Place> {
        self.row(addr).filter(|r| r.line != 0).map(|r| self.place_of(r))
    }

    /// Whether a statement starts at `addr`: an `is_stmt` row there.
    pub fn starts_statement(&self, addr: u32) -> bool {
        self.row(addr)
            .is_some_and(|r| r.addr == addr && r.is_stmt && r.line != 0)
    }

    /// The files whose path is `name` or ends with `/name`: `main.c` and
    /// `kernel/main.c` both name `kernel/main.c`.
    fn matching(&self, name: &str) -> Vec<usize> {
        let name = name.replace('\\', "/");
        let name = name.trim_start_matches("./");
        (0..self.files.len())
            .filter(|&i| {
                let f = &self.files[i];
                f == name || f.ends_with(&format!("/{name}"))
            })
            .collect()
    }

    /// Where a breakpoint on line `line` of `file` goes: for each function
    /// (as `function` gives each address's) with a statement on that line,
    /// the lowest address of one. A line with no code (a comment, a
    /// declaration) means the next one that has some, as in gdb. Also gives
    /// the line that was taken, or an error saying why there is none.
    pub fn breakpoints(
        &self,
        file: &str,
        line: u32,
        function: impl Fn(u32) -> Option<u32>,
    ) -> Result<(Vec<u32>, Place), String> {
        let files = self.matching(file);
        if files.is_empty() {
            return Err(format!("no source file named {file} in the line tables"));
        }
        let rows = || {
            self.runs
                .iter()
                .flat_map(|r| &r.rows)
                .filter(|r| files.contains(&r.file) && r.is_stmt)
        };
        let Some(taken) = rows().map(|r| r.line).filter(|&l| l >= line).min() else {
            return Err(format!("{file} has no code at or after line {line}"));
        };
        let mut best: HashMap<Option<u32>, (u32, usize)> = HashMap::new();
        for r in rows().filter(|r| r.line == taken) {
            let e = best.entry(function(r.addr)).or_insert((r.addr, r.file));
            if r.addr < e.0 {
                *e = (r.addr, r.file);
            }
        }
        let mut addrs: Vec<(u32, usize)> = best.into_values().collect();
        addrs.sort();
        let place = Place {
            file: self.files[addrs[0].1].clone(),
            line: taken,
        };
        Ok((addrs.into_iter().map(|(a, _)| a).collect(), place))
    }

    /// Where the function at `start..end` is past its prologue: its row marked
    /// `prologue_end`, else its second row (the first statement after its
    /// first line), else `start`.
    pub fn after_prologue(&self, start: u32, end: u32) -> u32 {
        let rows: Vec<&Row> = self
            .runs
            .iter()
            .flat_map(|r| &r.rows)
            .filter(|r| (start..end).contains(&r.addr))
            .collect();
        if let Some(r) = rows.iter().find(|r| r.prologue_end) {
            return r.addr;
        }
        rows.iter()
            .map(|r| r.addr)
            .filter(|&a| a > start)
            .min()
            .unwrap_or(start)
    }

    /// Where the lines of file `name` were looked for: `directory` first.
    pub fn add_dir(&mut self, dir: PathBuf) {
        self.dirs.insert(0, dir);
        self.texts.clear();
    }

    /// The file a path in the line tables names: under a `directory`, or as
    /// it is (relative to the current directory).
    fn read(&mut self, file: &str) -> Option<&Vec<String>> {
        if !self.texts.contains_key(file) {
            let candidates = self
                .dirs
                .iter()
                .map(|d| d.join(file))
                .chain(std::iter::once(PathBuf::from(file)));
            let text = candidates
                .filter_map(|p| std::fs::read_to_string(Path::new(&p)).ok())
                .next()
                .map(|t| t.lines().map(|l| l.trim_end_matches('\r').to_string()).collect());
            self.texts.insert(file.to_string(), text);
        }
        self.texts.get(file)?.as_ref()
    }

    /// The text of line `line` of `file`, if the file can be read.
    pub fn text(&mut self, file: &str, line: u32) -> Option<String> {
        let lines = self.read(file)?;
        lines.get((line as usize).checked_sub(1)?).cloned()
    }

    /// The number of lines of `file`, if it can be read.
    pub fn line_count(&mut self, file: &str) -> Option<u32> {
        self.read(file).map(|l| l.len() as u32)
    }

    /// The path of the file `name` names (see [`Lines::breakpoints`]), if
    /// exactly one.
    pub fn file_named(&self, name: &str) -> Result<String, String> {
        match self.matching(name).as_slice() {
            [i] => Ok(self.files[*i].clone()),
            [] => Err(format!("no source file named {name} in the line tables")),
            many => {
                let names: Vec<&str> = many.iter().map(|&i| self.files[i].as_str()).collect();
                Err(format!("{name} could be {}", names.join(" or ")))
            }
        }
    }
}
