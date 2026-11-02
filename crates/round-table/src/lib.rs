//! Round Table, the control CLI of the Avalon platform: the commands (`create`, `list`,
//! `info`, `start`, ..., `disk create`, ...) parsed from the command line and carried
//! out through [`avalon_client`], with their output as text.
//!
//! The `round-table` binary is [`run`] with the process's arguments and stdout; the
//! tests call [`run`] the same way against servers running in process.

use avalon_client::{
    device_name, disk_source_text, format_hz, format_size, parse_addr, parse_clock, parse_disk_size,
    parse_disk_source, parse_ram, Client, VmAction,
};
use avalon_proto::{
    validate_name, DeviceSpec, DiskInfo, DiskSpec, HardwareProfile, VmDetails, VmRef, VmSummary,
    DEFAULT_AVALON_ADDR, DEFAULT_LAKE_ADDR, SECTOR_SIZE,
};
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

/// The name Round Table gives in its `Hello`.
pub const AGENT: &str = "round-table";

pub const USAGE: &str = "usage: round-table [--server ADDR] <command> [arguments]
       round-table [--lake ADDR] disk <command> [arguments]

Gives orders to an Avalon hypervisor server: creates virtual machines and
controls them. The disk commands manage the disk images of a Lake server.

VM commands, sent to Avalon:
  create --name NAME [options] <program.elf>
                  create a VM that runs program.elf (an Excalibur ELF
                  executable, read here and sent to Avalon), in state created
  list            every VM: id, name, state, RAM, clock and disks
  info VM         one VM in detail: hardware, disks, steps run, devices
  start VM        start a created VM
  pause VM        pause a running VM; it keeps its state
  resume VM       go on running a paused VM
  stop VM         stop a VM for good, releasing its disks; a stopped VM
                  cannot be started again
  destroy VM      remove a VM in any state, freeing its RAM and its name
                  (and its disks, if it still holds them)

  VM is a VM's id (all digits) or its name.

create options:
  --name NAME     the VM's name (required): 1 to 64 lowercase letters,
                  digits, '.', '_' and '-', not all digits
  --ram MiB       RAM size in MiB, 1 to 2048 (default 128)
  --clock FREQ    real time: run FREQ instructions per second. FREQ is a
                  number with an optional unit, Hz, kHz or MHz: 10MHz, 500kHz,
                  1.5MHz, 5Hz. Without it the VM runs at full speed and
                  reports a nominal 10 MHz clock
  --disk SRC      attach a disk, read-write: the path of an image file on
                  Avalon's host (relative to Avalon's working directory), or
                  lake://ADDR/NAME for the image NAME on a Lake server.
                  Repeat for more disks, in order
  --disk-ro SRC   attach a disk read-only
  --start         start the VM as soon as it is created

Disk commands, sent to Lake:
  disk create NAME --size SIZE
                  a new zero-filled image. SIZE is a whole number of 512-byte
                  sectors: 64MiB, 512K, 1G, 2048s (sectors); no unit is bytes
  disk list       every image: its size and how it is attached
  disk delete NAME
                  delete an image nobody has attached

A disk is attached to a VM when the VM is created (--disk, --disk-ro), and
detached when the VM's machine ends: when the VM is stopped, its program
exits, or it is destroyed. A stopped or exited VM still lists its disks
(list, info) but no longer holds them. Lake allows one writer per image, or
any number of readers.

Options:
  --server ADDR   Avalon's address, for the VM commands: host:port, or a
                  host alone for port 7400 (default 127.0.0.1:7400)
  --lake ADDR     Lake's address, for the disk commands: host:port, or a
                  host alone for port 7401 (default 127.0.0.1:7401)
  -h, --help      show this text

Exit status: 0 on success, 1 when a server refuses the request or cannot be
reached, 2 for a mistake on the command line.";

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Avalon's address, `host:port`.
    pub server: String,
    /// Lake's address, `host:port`.
    pub lake: String,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Create(Create),
    List,
    Info(VmRef),
    Action(VmAction, VmRef),
    DiskCreate { name: String, sectors: u64 },
    DiskList,
    DiskDelete { name: String },
}

/// `create`'s arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create {
    pub name: String,
    pub profile: HardwareProfile,
    pub program: PathBuf,
    pub start: bool,
}

/// Why a command failed, and the exit status for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The command line is wrong: exit status 2.
    Usage(String),
    /// The request failed (refused, server unreachable, unreadable program): 1.
    Failed(String),
}

impl Failure {
    pub fn exit_code(&self) -> u8 {
        match self {
            Failure::Usage(_) => 2,
            Failure::Failed(_) => 1,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Failure::Usage(why) => write!(f, "{why}\nRun 'round-table --help' for the commands and options."),
            Failure::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// Parses `args` (without the program name) and carries the command out, writing what
/// it has to say to `out`.
pub fn run(args: &[String], out: &mut dyn Write) -> Result<(), Failure> {
    let invocation = parse_args(args).map_err(Failure::Usage)?;
    execute(&invocation, out)
}

// ---- Parsing ----

/// Every option, and whether it takes a value.
const OPTIONS: [(&str, bool); 9] = [
    ("--server", true),
    ("--lake", true),
    ("--name", true),
    ("--ram", true),
    ("--clock", true),
    ("--disk", true),
    ("--disk-ro", true),
    ("--start", false),
    ("--size", true),
];

/// Options that may be given more than once.
const REPEATABLE: [&str; 2] = ["--disk", "--disk-ro"];

/// Parses a command line (without the program name). Options may come before or after
/// the command, as `--opt value` or `--opt=value`; `--` ends the options.
pub fn parse_args(args: &[String]) -> Result<Invocation, String> {
    let mut options: Vec<(&'static str, String)> = Vec::new();
    let mut words: Vec<&str> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            words.extend(iter.by_ref().map(String::as_str));
            break;
        }
        if arg == "-h" || arg == "--help" {
            return Ok(Invocation {
                server: DEFAULT_AVALON_ADDR.into(),
                lake: DEFAULT_LAKE_ADDR.into(),
                command: Command::Help,
            });
        }
        if !arg.starts_with('-') || arg == "-" {
            words.push(arg);
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        if flag == "-h" || flag == "--help" {
            return Err(format!("{flag} takes no value"));
        }
        let Some(&(flag, takes_value)) = OPTIONS.iter().find(|(name, _)| *name == flag) else {
            return Err(format!("unknown option '{flag}'"));
        };
        let value = match (takes_value, inline) {
            (true, Some(value)) => value,
            (true, None) => iter
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))?,
            (false, None) => String::new(),
            (false, Some(_)) => return Err(format!("{flag} takes no value")),
        };
        if !REPEATABLE.contains(&flag) && options.iter().any(|(f, _)| *f == flag) {
            return Err(format!("{flag} is given twice"));
        }
        options.push((flag, value));
    }

    let option = |flag: &str| options.iter().find(|(f, _)| *f == flag).map(|(_, v)| v.as_str());
    let server = match option("--server") {
        Some(s) => parse_addr(s, port_of(DEFAULT_AVALON_ADDR)).map_err(|e| format!("--server: {e}"))?,
        None => DEFAULT_AVALON_ADDR.to_string(),
    };
    let lake = match option("--lake") {
        Some(s) => parse_addr(s, port_of(DEFAULT_LAKE_ADDR)).map_err(|e| format!("--lake: {e}"))?,
        None => DEFAULT_LAKE_ADDR.to_string(),
    };

    let Some((&word, rest)) = words.split_first() else {
        return Err("no command given".into());
    };
    // The options this command takes: --server for the VM commands (they talk to
    // Avalon), --lake for the disk commands (they talk to Lake), and their own.
    let (command, allowed): (Command, &[&str]) = match word {
        "create" => (
            Command::Create(parse_create(rest, &options)?),
            &[
                "--server",
                "--name",
                "--ram",
                "--clock",
                "--disk",
                "--disk-ro",
                "--start",
            ],
        ),
        "list" => {
            no_arguments(word, rest)?;
            (Command::List, &["--server"])
        }
        "info" => (Command::Info(vm_argument(word, rest)?), &["--server"]),
        "disk" => parse_disk(rest, &options)?,
        _ => match VmAction::parse(word) {
            Some(action) => (Command::Action(action, vm_argument(word, rest)?), &["--server"]),
            None => return Err(format!("unknown command '{word}'")),
        },
    };
    let command_name = match &command {
        Command::DiskCreate { .. } => "disk create",
        Command::DiskList => "disk list",
        Command::DiskDelete { .. } => "disk delete",
        _ => word,
    };
    for (flag, _) in &options {
        if !allowed.contains(flag) {
            let why = match *flag {
                "--server" => " (the disk commands talk to Lake: --lake ADDR)",
                "--lake" => " (the VM commands talk to Avalon: --server ADDR)",
                _ => "",
            };
            return Err(format!("'{command_name}' does not take {flag}{why}"));
        }
    }
    Ok(Invocation {
        server,
        lake,
        command,
    })
}

fn port_of(default_addr: &str) -> u16 {
    avalon_client::parse::default_port(default_addr)
}

fn no_arguments(command: &str, rest: &[&str]) -> Result<(), String> {
    match rest.first() {
        None => Ok(()),
        Some(extra) => Err(format!("'{command}' takes no arguments (got '{extra}')")),
    }
}

/// The single VM argument of `info`, `start`, ...: an id (all digits) or a name.
fn vm_argument(command: &str, rest: &[&str]) -> Result<VmRef, String> {
    match rest {
        [vm] if !vm.is_empty() => match VmRef::parse(vm) {
            // No name is all digits, so an id too big for a u32 can never match a VM.
            VmRef::Name(_) if vm.bytes().all(|b| b.is_ascii_digit()) => {
                Err(format!("'{vm}' is not a VM id (ids are at most {})", u32::MAX))
            }
            vm => Ok(vm),
        },
        [] | [_] => Err(format!("'{command}' needs a VM (its id or its name)")),
        [_, extra, ..] => Err(format!("'{command}' takes one VM (got '{extra}' as well)")),
    }
}

/// The single disk name of `disk create` and `disk delete`.
fn disk_argument(command: &str, rest: &[&str]) -> Result<String, String> {
    match rest {
        [name] => Ok(name.to_string()),
        [] => Err(format!("'disk {command}' needs a disk name")),
        [_, extra, ..] => Err(format!(
            "'disk {command}' takes one disk name (got '{extra}' as well)"
        )),
    }
}

fn parse_create(rest: &[&str], options: &[(&'static str, String)]) -> Result<Create, String> {
    let program = match rest {
        [program] => PathBuf::from(program),
        [] => return Err("'create' needs the program to run (an ELF file)".into()),
        [_, extra, ..] => return Err(format!("'create' takes one program (got '{extra}' as well)")),
    };
    let mut name = None;
    let mut profile = HardwareProfile::default();
    let mut start = false;
    for (flag, value) in options {
        let with = |e: String| format!("{flag}: {e}");
        match *flag {
            "--name" => {
                validate_name(value).map_err(with)?;
                name = Some(value.clone());
            }
            "--ram" => profile.ram_size = parse_ram(value).map_err(with)?,
            "--clock" => profile.clock_hz = Some(parse_clock(value).map_err(with)?),
            "--disk" | "--disk-ro" => profile.devices.push(DeviceSpec::Disk(DiskSpec {
                source: parse_disk_source(value).map_err(with)?,
                read_only: *flag == "--disk-ro",
            })),
            "--start" => start = true,
            _ => {}
        }
    }
    let name = name.ok_or("'create' needs --name NAME")?;
    Ok(Create {
        name,
        profile,
        program,
        start,
    })
}

fn parse_disk(
    rest: &[&str],
    options: &[(&'static str, String)],
) -> Result<(Command, &'static [&'static str]), String> {
    let Some((&word, rest)) = rest.split_first() else {
        return Err("'disk' needs a command: create, list or delete".into());
    };
    match word {
        "create" => {
            let name = disk_argument(word, rest)?;
            validate_name(&name)?;
            let size = options
                .iter()
                .find(|(f, _)| *f == "--size")
                .map(|(_, v)| v)
                .ok_or("'disk create' needs --size SIZE")?;
            let sectors = parse_disk_size(size).map_err(|e| format!("--size: {e}"))?;
            Ok((Command::DiskCreate { name, sectors }, &["--lake", "--size"]))
        }
        "list" => {
            no_arguments("disk list", rest)?;
            Ok((Command::DiskList, &["--lake"]))
        }
        "delete" => Ok((
            Command::DiskDelete {
                name: disk_argument(word, rest)?,
            },
            &["--lake"],
        )),
        _ => Err(format!("unknown disk command '{word}' (create, list or delete)")),
    }
}

// ---- Carrying commands out ----

/// Carries out a parsed command.
pub fn execute(invocation: &Invocation, out: &mut dyn Write) -> Result<(), Failure> {
    let text = match &invocation.command {
        Command::Help => format!("{USAGE}\n"),
        Command::Create(create) => {
            let program = std::fs::read(&create.program)
                .map_err(|e| Failure::Failed(format!("cannot read {}: {e}", create.program.display())))?;
            let mut avalon = connect("Avalon", &invocation.server)?;
            let id = avalon
                .create_vm(&create.name, create.profile.clone(), program)
                .map_err(failed)?;
            let mut text = format!("created VM {id} ({})\n", create.name);
            if create.start {
                if let Err(e) = avalon.start_vm(&VmRef::Id(id)) {
                    // Say what did happen before the error.
                    write_out(out, &text)?;
                    return Err(Failure::Failed(format!(
                        "VM {id} ({}) was created, but did not start: {e}",
                        create.name
                    )));
                }
                text += &format!("started VM {id} ({})\n", create.name);
            }
            text
        }
        Command::List => vm_table(
            &connect("Avalon", &invocation.server)?
                .list_vms()
                .map_err(failed)?,
        ),
        Command::Info(vm) => vm_details(
            &connect("Avalon", &invocation.server)?
                .vm_info(vm)
                .map_err(failed)?,
        ),
        Command::Action(action, vm) => {
            connect("Avalon", &invocation.server)?
                .vm_action(*action, vm)
                .map_err(failed)?;
            format!("{} VM {vm}\n", action.done())
        }
        Command::DiskCreate { name, sectors } => {
            connect("Lake", &invocation.lake)?
                .create_disk(name, *sectors)
                .map_err(failed)?;
            format!("created disk {name} ({})\n", sectors_text(*sectors))
        }
        Command::DiskList => disk_table(&connect("Lake", &invocation.lake)?.list_disks().map_err(failed)?),
        Command::DiskDelete { name } => {
            connect("Lake", &invocation.lake)?
                .delete_disk(name)
                .map_err(failed)?;
            format!("deleted disk {name}\n")
        }
    };
    write_out(out, &text)
}

fn connect(what: &str, addr: &str) -> Result<Client, Failure> {
    Client::connect(addr, AGENT).map_err(|e| match e {
        avalon_client::Error::Connect { addr, cause } => {
            Failure::Failed(format!("cannot connect to {what} at {addr}: {cause}"))
        }
        other => failed(other),
    })
}

fn failed(e: avalon_client::Error) -> Failure {
    Failure::Failed(e.to_string())
}

fn write_out(out: &mut dyn Write, text: &str) -> Result<(), Failure> {
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| Failure::Failed(format!("cannot write the output: {e}")))
}

// ---- Output ----

/// Rows under a header, each column as wide as its widest cell, two spaces apart.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let header: Vec<String> = header.iter().map(|h| h.to_string()).collect();
    let mut text = String::new();
    for row in std::iter::once(&header).chain(rows) {
        let mut line = String::new();
        for (i, (cell, w)) in row.iter().zip(&widths).enumerate() {
            if i > 0 {
                line += "  ";
            }
            line += &format!("{cell:w$}");
        }
        text += line.trim_end();
        text += "\n";
    }
    text
}

/// A VM's clock: its real-time frequency, or full speed.
fn clock_text(profile: &HardwareProfile) -> String {
    match profile.clock_hz {
        Some(hz) => format_hz(hz),
        None => "full speed".into(),
    }
}

fn disks(profile: &HardwareProfile) -> impl Iterator<Item = &DiskSpec> {
    profile.devices.iter().map(|DeviceSpec::Disk(disk)| disk)
}

/// `list`: one row per VM.
pub fn vm_table(vms: &[VmSummary]) -> String {
    if vms.is_empty() {
        return "no VMs\n".into();
    }
    let rows: Vec<Vec<String>> = vms
        .iter()
        .map(|vm| {
            vec![
                vm.id.to_string(),
                vm.name.clone(),
                vm.state.to_string(),
                format_size(vm.profile.ram_size),
                clock_text(&vm.profile),
                disks(&vm.profile).count().to_string(),
            ]
        })
        .collect();
    table(&["ID", "NAME", "STATE", "RAM", "CLOCK", "DISKS"], &rows)
}

/// `info`: one VM, a field per line.
pub fn vm_details(details: &VmDetails) -> String {
    let VmDetails { vm, devices, steps } = details;
    let clock = match vm.profile.clock_hz {
        Some(hz) => format!("{} (real time)", format_hz(hz)),
        None => format!(
            "full speed (reports {} to the guest)",
            format_hz(vm.profile.nominal_hz)
        ),
    };
    let mut text = format!(
        "id       {}\nname     {}\nstate    {}\nRAM      {}\nclock    {clock}\nsteps    {steps}\n",
        vm.id,
        vm.name,
        vm.state,
        format_size(vm.profile.ram_size),
    );
    // One line per disk, the label on the first.
    let lines: Vec<String> = disks(&vm.profile)
        .map(|disk| {
            let mode = if disk.read_only { "read-only" } else { "read-write" };
            format!("{} ({mode})", disk_source_text(&disk.source))
        })
        .collect();
    if lines.is_empty() {
        text += "disks    none\n";
    }
    for (i, line) in lines.iter().enumerate() {
        text += if i == 0 { "disks    " } else { "         " };
        text += line;
        text += "\n";
    }
    text += &format!("devices  {}\n", devices.len());
    for d in devices {
        let name = device_name(d.kind).map_or_else(|| format!("type {}", d.kind), str::to_string);
        let irq = if d.irq != 0 {
            format!("  irq {}", d.irq)
        } else {
            String::new()
        };
        text += &format!("  {name:<9} {:#010x} + {:#010x}{irq}\n", d.base, d.size);
    }
    text
}

/// A disk's size, in bytes and sectors.
fn sectors_text(sectors: u64) -> String {
    let bytes = sectors.saturating_mul(SECTOR_SIZE as u64);
    let unit = if sectors == 1 { "sector" } else { "sectors" };
    format!("{}, {sectors} {unit}", format_size(bytes))
}

/// `disk list`: one row per image.
pub fn disk_table(disks: &[DiskInfo]) -> String {
    if disks.is_empty() {
        return "no disks\n".into();
    }
    let rows: Vec<Vec<String>> = disks
        .iter()
        .map(|d| {
            let mut attached = Vec::new();
            if d.writer {
                attached.push("read-write".to_string());
            }
            match d.readers {
                0 => {}
                1 => attached.push("1 reader".into()),
                n => attached.push(format!("{n} readers")),
            }
            vec![
                d.name.clone(),
                format_size(d.sectors.saturating_mul(SECTOR_SIZE as u64)),
                d.sectors.to_string(),
                if attached.is_empty() {
                    "no".into()
                } else {
                    attached.join(", ")
                },
            ]
        })
        .collect();
    table(&["NAME", "SIZE", "SECTORS", "ATTACHED"], &rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use avalon_proto::{DeviceEntry, DiskSource, VmState};

    fn parse(args: &str) -> Result<Invocation, String> {
        let args: Vec<String> = args.split_whitespace().map(str::to_string).collect();
        parse_args(&args)
    }

    fn command(args: &str) -> Command {
        parse(args).unwrap_or_else(|e| panic!("{args}: {e}")).command
    }

    #[test]
    fn addresses() {
        let inv = parse("list").unwrap();
        assert_eq!(inv.server, "127.0.0.1:7400");
        assert_eq!(inv.lake, "127.0.0.1:7401");
        let inv = parse("--server 10.0.0.2 list").unwrap();
        assert_eq!(inv.server, "10.0.0.2:7400");
        let inv = parse("--lake=storage disk list").unwrap();
        assert_eq!(inv.lake, "storage:7401");
        // Options may come after the command too.
        let inv = parse("disk list --lake 127.0.0.1:9000").unwrap();
        assert_eq!(inv.lake, "127.0.0.1:9000");
        assert_eq!(inv.command, Command::DiskList);
        assert!(parse("--server bad:addr:x list")
            .unwrap_err()
            .starts_with("--server: "));
        assert!(parse("--lake").unwrap_err().contains("needs a value"));
        assert!(parse("--server 127.0.0.1:0 list")
            .unwrap_err()
            .starts_with("--server: "));
    }

    #[test]
    fn vm_commands() {
        assert_eq!(command("list"), Command::List);
        assert_eq!(command("info 3"), Command::Info(VmRef::Id(3)));
        assert_eq!(command("info web"), Command::Info(VmRef::Name("web".into())));
        assert_eq!(command("start 7"), Command::Action(VmAction::Start, VmRef::Id(7)));
        assert_eq!(
            command("pause a"),
            Command::Action(VmAction::Pause, VmRef::Name("a".into()))
        );
        assert_eq!(
            command("resume 1"),
            Command::Action(VmAction::Resume, VmRef::Id(1))
        );
        assert_eq!(command("stop 1"), Command::Action(VmAction::Stop, VmRef::Id(1)));
        assert_eq!(
            command("destroy 1"),
            Command::Action(VmAction::Destroy, VmRef::Id(1))
        );
        assert_eq!(command("--help"), Command::Help);
        assert_eq!(command("start 1 -h"), Command::Help);
        assert_eq!(command("info 007"), Command::Info(VmRef::Id(7)));
        assert_eq!(command("info 4294967295"), Command::Info(VmRef::Id(u32::MAX)));
    }

    #[test]
    fn create() {
        let Command::Create(c) = command(
            "create --name web --ram 64 --clock 10MHz --disk lake://lake:7401/data --disk-ro boot.img --start prog.elf",
        ) else {
            panic!()
        };
        assert_eq!(c.name, "web");
        assert_eq!(c.program, PathBuf::from("prog.elf"));
        assert!(c.start);
        assert_eq!(c.profile.ram_size, 64 << 20);
        assert_eq!(c.profile.clock_hz, Some(10_000_000));
        assert_eq!(c.profile.nominal_hz, avalon_proto::DEFAULT_NOMINAL_HZ);
        assert_eq!(
            c.profile.devices,
            vec![
                DeviceSpec::Disk(DiskSpec {
                    source: DiskSource::Lake {
                        addr: "lake:7401".into(),
                        name: "data".into()
                    },
                    read_only: false
                }),
                DeviceSpec::Disk(DiskSpec {
                    source: DiskSource::File("boot.img".into()),
                    read_only: true
                }),
            ]
        );
        // Defaults: Grail's machine.
        let Command::Create(c) = command("create prog.elf --name=web") else {
            panic!()
        };
        assert_eq!(c.profile, HardwareProfile::default());
        assert!(!c.start);
        // A program whose name starts with '-', after '--'.
        let Command::Create(c) = command("create --name web -- -prog.elf") else {
            panic!()
        };
        assert_eq!(c.program, PathBuf::from("-prog.elf"));
    }

    #[test]
    fn disk_commands() {
        assert_eq!(
            command("disk create data --size 64MiB"),
            Command::DiskCreate {
                name: "data".into(),
                sectors: 131_072
            }
        );
        assert_eq!(command("disk list"), Command::DiskList);
        assert_eq!(
            command("disk delete data"),
            Command::DiskDelete { name: "data".into() }
        );
    }

    #[test]
    fn mistakes() {
        for (args, why) in [
            ("", "no command given"),
            ("frobnicate", "unknown command 'frobnicate'"),
            ("list --bogus", "unknown option '--bogus'"),
            ("list extra", "'list' takes no arguments (got 'extra')"),
            ("list --ram 64", "'list' does not take --ram"),
            ("start", "'start' needs a VM (its id or its name)"),
            ("info 1 2", "'info' takes one VM (got '2' as well)"),
            (
                "create --name a",
                "'create' needs the program to run (an ELF file)",
            ),
            (
                "create a.elf b.elf --name a",
                "'create' takes one program (got 'b.elf' as well)",
            ),
            ("create a.elf", "'create' needs --name NAME"),
            ("create a.elf --name a --name b", "--name is given twice"),
            ("create a.elf --name a --start=yes", "--start takes no value"),
            ("create a.elf --name a --size 1M", "'create' does not take --size"),
            ("create a.elf --name Web", "--name: 'Web' is not a valid name"),
            ("create a.elf --name 42", "--name: '42' is not a valid name"),
            ("create a.elf --name a --ram 0", "--ram: 0 MiB is out of range"),
            (
                "create a.elf --name a --ram 64MiB",
                "--ram: '64MiB' is not a whole number of MiB",
            ),
            (
                "create a.elf --name a --clock fast",
                "--clock: 'fast' is not a frequency",
            ),
            ("create a.elf --name a --disk lake://x/", "--disk: 'lake://x/'"),
            ("create a.elf --name a --disk", "--disk needs a value"),
            ("disk", "'disk' needs a command"),
            ("disk resize a", "unknown disk command 'resize'"),
            ("disk create a", "'disk create' needs --size SIZE"),
            ("disk create --size 1M", "'disk create' needs a disk name"),
            (
                "disk create a b --size 1M",
                "'disk create' takes one disk name (got 'b' as well)",
            ),
            ("disk create con --size 1M", "'con' is not a valid name"),
            (
                "disk create a --size 100",
                "--size: 100 is not a whole number of 512-byte sectors",
            ),
            ("disk list --size 1M", "'disk list' does not take --size"),
            ("disk delete", "'disk delete' needs a disk name"),
            ("disk delete a --start", "'disk delete' does not take --start"),
            (
                "info 4294967296",
                "'4294967296' is not a VM id (ids are at most 4294967295)",
            ),
            (
                "stop 99999999999",
                "'99999999999' is not a VM id (ids are at most 4294967295)",
            ),
            ("--help=x", "--help takes no value"),
            ("-h=", "-h takes no value"),
            // Each address goes with the commands that talk to its server.
            (
                "--server 10.0.0.2 disk list",
                "'disk list' does not take --server (the disk commands talk to Lake: --lake ADDR)",
            ),
            (
                "disk create a --size 1M --server x",
                "'disk create' does not take --server",
            ),
            (
                "list --lake x",
                "'list' does not take --lake (the VM commands talk to Avalon: --server ADDR)",
            ),
            ("create a.elf --name a --lake x", "'create' does not take --lake"),
            ("start 1 --lake x", "'start' does not take --lake"),
        ] {
            match parse(args) {
                Err(e) => assert!(e.starts_with(why), "{args}: {e}"),
                Ok(inv) => panic!("{args}: parsed as {inv:?}"),
            }
        }
        // An empty VM argument (split_whitespace cannot make one).
        for action in ["info", "start", "destroy"] {
            let args = [action.to_string(), String::new()];
            assert_eq!(
                parse_args(&args),
                Err(format!("'{action}' needs a VM (its id or its name)"))
            );
            let failure = run(&args, &mut Vec::new()).unwrap_err();
            assert_eq!(failure.exit_code(), 2, "{failure}");
        }
        let usage = run(&[], &mut Vec::new()).unwrap_err();
        assert_eq!(usage.exit_code(), 2);
        assert!(usage
            .to_string()
            .ends_with("Run 'round-table --help' for the commands and options."));
    }

    #[test]
    fn help() {
        let mut out = Vec::new();
        run(&["--help".to_string()], &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("{USAGE}\n"));
        // The usage text fits a terminal.
        for line in USAGE.lines() {
            assert!(line.chars().count() <= 80, "{line}");
        }
    }

    #[test]
    fn tables() {
        let rows = vec![
            vec!["1".to_string(), "a".into(), "x".into()],
            vec!["10".to_string(), "longer".into(), "".into()],
        ];
        assert_eq!(
            table(&["ID", "NAME", "Z"], &rows),
            "ID  NAME    Z\n1   a       x\n10  longer\n"
        );
        assert_eq!(vm_table(&[]), "no VMs\n");
        assert_eq!(disk_table(&[]), "no disks\n");
    }

    fn summary(id: u32, name: &str, state: VmState, profile: HardwareProfile) -> VmSummary {
        VmSummary {
            id,
            name: name.into(),
            state,
            profile,
        }
    }

    #[test]
    fn vm_output() {
        let disk = |source: &str, read_only| {
            DeviceSpec::Disk(DiskSpec {
                source: avalon_client::parse_disk_source(source).unwrap(),
                read_only,
            })
        };
        let web = HardwareProfile {
            ram_size: 64 << 20,
            clock_hz: Some(1_500_000),
            nominal_hz: 10_000_000,
            devices: vec![disk("lake://10.0.0.9:7401/data", false), disk("boot.img", true)],
        };
        let vms = [
            summary(1, "web", VmState::Running, web.clone()),
            summary(12, "batch-job", VmState::Exited(3), HardwareProfile::default()),
        ];
        assert_eq!(
            vm_table(&vms),
            "\
ID  NAME       STATE      RAM      CLOCK       DISKS
1   web        running    64 MiB   1.5 MHz     2
12  batch-job  exited(3)  128 MiB  full speed  0
"
        );
        let details = VmDetails {
            vm: vms[0].clone(),
            devices: vec![
                DeviceEntry {
                    kind: 3,
                    base: 0x1000_0000,
                    size: 0x100,
                    irq: 1,
                },
                DeviceEntry {
                    kind: 99,
                    base: 0x4000_0000,
                    size: 0x1000,
                    irq: 0,
                },
            ],
            steps: 12345,
        };
        assert_eq!(
            vm_details(&details),
            "\
id       1
name     web
state    running
RAM      64 MiB
clock    1.5 MHz (real time)
steps    12345
disks    lake://10.0.0.9:7401/data (read-write)
         boot.img (read-only)
devices  2
  UART      0x10000000 + 0x00000100  irq 1
  type 99   0x40000000 + 0x00001000
"
        );
        let details = VmDetails {
            vm: vms[1].clone(),
            devices: vec![],
            steps: 0,
        };
        let text = vm_details(&details);
        assert!(
            text.contains("\nclock    full speed (reports 10 MHz to the guest)\n"),
            "{text}"
        );
        assert!(text.contains("\ndisks    none\ndevices  0\n"), "{text}");
    }

    #[test]
    fn disk_output() {
        let disk = |name: &str, sectors, readers, writer| DiskInfo {
            name: name.into(),
            sectors,
            readers,
            writer,
        };
        assert_eq!(
            disk_table(&[
                disk("boot", 8, 2, false),
                disk("data", 131_072, 0, true),
                disk("one", 1, 1, false),
                disk("spare", 3, 0, false),
            ]),
            "\
NAME   SIZE       SECTORS  ATTACHED
boot   4 KiB      8        2 readers
data   64 MiB     131072   read-write
one    512 bytes  1        1 reader
spare  1.5 KiB    3        no
"
        );
        assert_eq!(sectors_text(1), "512 bytes, 1 sector");
        assert_eq!(sectors_text(131_072), "64 MiB, 131072 sectors");
    }
}
