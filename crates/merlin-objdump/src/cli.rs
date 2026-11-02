//! Command-line parsing, kept apart from `main` so it can be tested.

use crate::Options;

pub const USAGE: &str = "usage: merlin-objdump <options> file...

  -d, --disassemble       disassemble executable sections
  -r, --reloc             relocations (shown under each instruction with -d)
  -t, --syms              symbol table
  -h, --section-headers   section headers (also --headers)
  -x, --all-headers       all of -f -p -h -r -t
  -f, --file-headers      architecture and entry point
  -p, --private-headers   program headers
  -s, --full-contents     hex dump of every section
  -M no-aliases           real instructions instead of pseudo-instructions
                          (also -Mno-aliases, --disassembler-options=no-aliases)
  --no-show-raw-insn      do not print instruction words
  --help                  this text

Short options combine: -dr is -d -r.";

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Dump { opts: Options, files: Vec<String> },
    Help,
}

fn disassembler_option(opts: &mut Options, value: &str) -> Result<(), String> {
    for o in value.split(',') {
        match o {
            "no-aliases" => opts.aliases = false,
            _ => return Err(format!("unrecognized disassembler option: {o}")),
        }
    }
    Ok(())
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut opts = Options::default();
    let mut files = Vec::new();
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--help" => return Ok(Command::Help),
            "--disassemble" => opts.disassemble = true,
            "--reloc" => opts.relocs = true,
            "--syms" => opts.symbols = true,
            "--section-headers" | "--headers" => opts.section_headers = true,
            "--all-headers" => opts.set_all_headers(),
            "--file-headers" => opts.file_headers = true,
            "--private-headers" => opts.private_headers = true,
            "--full-contents" => opts.contents = true,
            "--no-show-raw-insn" => opts.raw_insn = false,
            "--show-raw-insn" => opts.raw_insn = true,
            "-M" => match args.next() {
                Some(v) => disassembler_option(&mut opts, &v)?,
                None => return Err("-M needs an argument".to_string()),
            },
            _ if a.starts_with("--disassembler-options=") => {
                disassembler_option(&mut opts, &a["--disassembler-options=".len()..])?
            }
            _ if a.starts_with("-M") => disassembler_option(&mut opts, &a[2..])?,
            _ if a.starts_with("--") => return Err(format!("unknown option {a}")),
            _ if a.starts_with('-') && a.len() > 1 => {
                for c in a[1..].chars() {
                    match c {
                        'd' => opts.disassemble = true,
                        'r' => opts.relocs = true,
                        't' => opts.symbols = true,
                        'h' => opts.section_headers = true,
                        'x' => opts.set_all_headers(),
                        'f' => opts.file_headers = true,
                        'p' => opts.private_headers = true,
                        's' => opts.contents = true,
                        _ => return Err(format!("unknown option -{c}")),
                    }
                }
            }
            _ => files.push(a),
        }
    }
    if !opts.any() {
        return Err("no command specified (try -d, -h, -t, -r, -x or -s)".to_string());
    }
    if files.is_empty() {
        return Err("no input file".to_string());
    }
    Ok(Command::Dump { opts, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    fn opts(args: &[&str]) -> Options {
        match parse(args) {
            Ok(Command::Dump { opts, .. }) => opts,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn short_options_combine() {
        let o = opts(&["-dr", "a.o"]);
        assert!(o.disassemble && o.relocs && !o.symbols);
        let o = opts(&["-x", "a.o"]);
        assert!(o.file_headers && o.private_headers && o.section_headers && o.symbols && o.relocs);
        assert!(!o.disassemble && !o.contents);
        let o = opts(&["-h", "-t", "-s", "a.o"]);
        assert!(o.section_headers && o.symbols && o.contents);
    }

    #[test]
    fn long_options() {
        let o = opts(&[
            "--disassemble",
            "--reloc",
            "--no-show-raw-insn",
            "-M",
            "no-aliases",
            "a.o",
        ]);
        assert!(o.disassemble && o.relocs && !o.raw_insn && !o.aliases);
        assert!(!opts(&["-d", "-Mno-aliases", "a.o"]).aliases);
        assert!(!opts(&["-d", "--disassembler-options=no-aliases", "a.o"]).aliases);
        assert!(opts(&["--headers", "a.o"]).section_headers);
    }

    #[test]
    fn files_in_order() {
        match parse(&["a.o", "-d", "b.elf"]).unwrap() {
            Command::Dump { files, .. } => assert_eq!(files, ["a.o", "b.elf"]),
            Command::Help => panic!(),
        }
    }

    #[test]
    fn errors() {
        assert_eq!(parse(&["--help", "-q"]), Ok(Command::Help));
        assert!(parse(&["-q", "a.o"]).unwrap_err().contains("-q"));
        assert!(parse(&["--bogus", "a.o"]).unwrap_err().contains("--bogus"));
        assert!(parse(&["a.o"]).unwrap_err().contains("no command"));
        assert!(parse(&["-d"]).unwrap_err().contains("no input"));
        assert!(parse(&["-d", "-M"]).unwrap_err().contains("needs an argument"));
        assert!(parse(&["-d", "-M", "att", "a.o"]).unwrap_err().contains("att"));
    }
}
