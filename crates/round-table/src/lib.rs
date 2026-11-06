//! Round Table, the control CLI of the Avalon platform: the commands (`create`, `list`,
//! `info`, `start`, ..., `switch create`, ..., `disk create`, ...) parsed from the
//! command line and carried out through [`avalon_client`], with their output as text.
//!
//! The `round-table` binary is [`run`] with the process's arguments and stdout; the
//! tests call [`run`] the same way against servers running in process.

use avalon_client::{
    arch_text, create_camelot_vm, device_name, disk_source_text, format_hz, format_size, mac_text,
    parse_addr, parse_clock, parse_disk_size, parse_disk_source, parse_harts, parse_hostfwd, parse_net,
    parse_paging, parse_preset, parse_ram, preset, vm_disk, Client, Preset, VmAction, PRESETS,
};
use avalon_proto::{
    validate_name, Bios, DeviceSpec, DiskInfo, DiskSpec, HardwareProfile, NatConfig, NetSpec, PortOwner,
    SwitchInfo, VmDetails, VmRef, VmSummary, DEFAULT_AVALON_ADDR, DEFAULT_LAKE_ADDR, SECTOR_SIZE,
};
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

/// The name Round Table gives in its `Hello`.
pub const AGENT: &str = "round-table";

pub const USAGE: &str = "usage: round-table [--server ADDR] <command> [arguments]
       round-table [--server ADDR] switch <command> [arguments]
       round-table [--lake ADDR] disk <command> [arguments]

Gives orders to an Avalon hypervisor server: creates virtual machines and
controls them, and the virtual switches their network cards plug into. The
disk commands manage the disk images of a Lake server.

VM commands, sent to Avalon:
  create --name NAME [options] [program.elf]
                  create a VM that runs program.elf (an Excalibur ELF
                  executable, read here and sent to Avalon), in state created.
                  Without a program the VM boots from its first --disk, as
                  grail --disk IMG does
  create --name NAME --profile PROFILE [--lake ADDR] [options]
                  create a VM that boots Camelot from a disk of its own:
                  the profile's Lake image (cargo xtask up puts them there)
                  is copied to NAME-disk0, which becomes the VM's first
                  disk. PROFILE is server (image os-base) or desktop
                  (os-desktop); --camelot is --profile server
  profiles        the hardware profiles of Camelot VMs: image, RAM, firmware
  list            every VM: id, name, state, RAM, clock, disks and cards
  info VM         one VM in detail: hardware, disks, network cards, steps
                  run, devices
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
  --paging MODE   the VM's paging: sv48 (Sv48 and Sv39, the default) or sv39
                  (Sv39 alone, as most RV64 boards). Every VM is RV64, as
                  QEMU's qemu-system-riscv64: an ELF32 program does not run
  --bios FW       the VM's firmware, as QEMU's -bios: none (the default: the
                  program runs in M-mode), default (Camelot's firmware, which
                  starts the program, or the disk's boot block, in S-mode)
                  or the path of a firmware file on Avalon's host. A Camelot
                  VM (--profile) has default unless this says otherwise
  --ram SIZE      RAM size: MiB, or a number with a unit, M/MiB, G/GiB or
                  T/TiB (--ram 64, --ram 16G); 1 MiB to 1 TiB (default 4 MiB;
                  a Camelot VM has its profile's: 6 MiB for a server, whose
                  firmware keeps 2 MiB, 64 MiB for a desktop)
  --clock FREQ    real time: run FREQ instructions per second. FREQ is a
                  number with an optional unit, Hz, kHz or MHz: 10MHz, 500kHz,
                  1.5MHz, 5Hz. Without it the VM runs at full speed and
                  reports a nominal 10 MHz clock
  --harts N       harts (cores), 1 to 8 (default 1); each runs at the clock
  --disk SRC      attach a disk, read-write: the path of an image file on
                  Avalon's host (relative to Avalon's working directory), or
                  lake://ADDR/NAME for the image NAME on a Lake server.
                  Repeat for more disks, in order
  --disk-ro SRC   attach a disk read-only
  --net SWITCH[,mac=ADDR]
                  a network card plugged into the switch SWITCH on Avalon
                  ('-' for none: the card starts unplugged), with the MAC
                  address ADDR (02:00:00:00:00:01; without one Avalon makes
                  one up from the VM's name). Repeat for more cards, up to 4
  --start         start the VM as soon as it is created
  --profile PROFILE
                  a Camelot VM of that hardware profile (round-table
                  profiles lists them), booting from NAME-disk0, a copy of
                  the profile's image made on the Lake at --lake ADDR (no
                  program); Avalon reaches the disk at that address too.
                  --ram, --bios and the rest change the profile's machine
  --camelot       --profile server

Switch commands, sent to Avalon:
  switch create NAME [--nat [--hostfwd RULE]...]
                  a new virtual Ethernet switch, with no cards plugged in.
                  --nat gives it a NAT gateway: its VMs get addresses by DHCP
                  on 10.0.2.0/24 and reach the internet and Avalon's host
                  (10.0.2.2 is the host's 127.0.0.1). --hostfwd forwards a
                  port of Avalon's host to a VM (repeatable). RULE is
                  [tcp|udp]:[HOSTADDR]:HOSTPORT-[GUESTADDR]:GUESTPORT, as
                  QEMU's hostfwd: tcp::8080-:80 forwards 127.0.0.1:8080 to
                  port 80 of 10.0.2.15, the first VM to ask for an address
                  (the second gets 10.0.2.16: tcp::2222-10.0.2.16:22)
  switch nat NAME [--hostfwd RULE]...
                  give a switch a NAT gateway
  switch nat NAME --off
                  take a switch's NAT gateway away
  switch list     every switch: its ports, NAT and capture
  switch show NAME
                  one switch's ports, with frames and bytes in and out and
                  frames dropped for want of room; its gateway's forwarded
                  ports and addresses given out
  switch attach SWITCH VM [--card N]
                  plug network card N (default 0) of VM into SWITCH
  switch detach VM [--card N]
                  unplug network card N (default 0) of VM
  switch capture SWITCH FILE
                  write every frame that enters SWITCH to FILE, a pcap file
                  on Avalon's host (Wireshark reads it)
  switch capture SWITCH --stop
                  stop the capture
  switch delete NAME
                  delete a switch, unplugging every card from it

Disk commands, sent to Lake:
  disk create NAME --size SIZE
                  a new zero-filled image. SIZE is a whole number of 512-byte
                  sectors: 64MiB, 512K, 1G, 2048s (sectors); no unit is bytes
  disk create NAME --from SRC [--size SIZE]
                  a new image that is a copy of the image SRC (which must not
                  have a writer), as big as SRC or SIZE (no smaller than SRC;
                  zeros after its end)
  disk list       every image: its size and how it is attached
  disk delete NAME
                  delete an image nobody has attached

A disk is attached to a VM when the VM is created (--disk, --disk-ro), and
detached when the VM's machine ends: when the VM is stopped, its program
exits, or it is destroyed. A stopped or exited VM still lists its disks
(list, info) but no longer holds them. Lake allows one writer per image, or
any number of readers. A VM's network cards are unplugged when its machine
ends, too.

Options:
  --server ADDR   Avalon's address, for the VM commands: host:port, or a
                  host alone for port 7400 (default 127.0.0.1:7400)
  --lake ADDR     Lake's address, for the disk commands and create --profile:
                  host:port, or a host alone for port 7401 (default
                  127.0.0.1:7401)
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
    Profiles,
    List,
    Info(VmRef),
    Action(VmAction, VmRef),
    DiskCreate {
        name: String,
        sectors: u64,
    },
    /// A copy of image `source`, as big as it (`None`) or `sectors`.
    DiskClone {
        name: String,
        source: String,
        sectors: Option<u64>,
    },
    DiskList,
    DiskDelete {
        name: String,
    },
    SwitchCreate {
        name: String,
        nat: Option<NatConfig>,
    },
    /// Give a switch a NAT gateway, or take it away (`None`).
    SwitchNat {
        name: String,
        nat: Option<NatConfig>,
    },
    SwitchList,
    SwitchShow {
        name: String,
    },
    SwitchAttach {
        switch: String,
        vm: VmRef,
        card: u8,
    },
    SwitchDetach {
        vm: VmRef,
        card: u8,
    },
    /// Start capturing into a file on Avalon's host, or stop (`None`).
    SwitchCapture {
        switch: String,
        path: Option<String>,
    },
    SwitchDelete {
        name: String,
    },
}

/// `create`'s arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create {
    pub name: String,
    pub profile: HardwareProfile,
    /// `None`: boot from the first disk.
    pub program: Option<PathBuf>,
    /// Boot Camelot as this hardware profile, from a copy of its Lake image made
    /// first (`--profile`; `--camelot` is the server's).
    pub camelot: Option<&'static Preset>,
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
const OPTIONS: [(&str, bool); 21] = [
    ("--server", true),
    ("--lake", true),
    ("--name", true),
    ("--paging", true),
    ("--bios", true),
    ("--ram", true),
    ("--clock", true),
    ("--harts", true),
    ("--disk", true),
    ("--disk-ro", true),
    ("--start", false),
    ("--size", true),
    ("--net", true),
    ("--card", true),
    ("--stop", false),
    ("--nat", false),
    ("--hostfwd", true),
    ("--off", false),
    ("--from", true),
    ("--camelot", false),
    ("--profile", true),
];

/// Options that may be given more than once.
const REPEATABLE: [&str; 4] = ["--disk", "--disk-ro", "--net", "--hostfwd"];

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
                "--paging",
                "--bios",
                "--ram",
                "--clock",
                "--harts",
                "--disk",
                "--disk-ro",
                "--net",
                "--start",
                "--camelot",
                "--profile",
                "--lake",
            ],
        ),
        "profiles" => {
            no_arguments(word, rest)?;
            (Command::Profiles, &[])
        }
        "list" => {
            no_arguments(word, rest)?;
            (Command::List, &["--server"])
        }
        "info" => (Command::Info(vm_argument(word, rest)?), &["--server"]),
        "disk" => parse_disk(rest, &options)?,
        "switch" => parse_switch(rest, &options)?,
        _ => match VmAction::parse(word) {
            Some(action) => (Command::Action(action, vm_argument(word, rest)?), &["--server"]),
            None => return Err(format!("unknown command '{word}'")),
        },
    };
    let command_name = match &command {
        Command::DiskCreate { .. } | Command::DiskClone { .. } => "disk create",
        Command::DiskList => "disk list",
        Command::DiskDelete { .. } => "disk delete",
        Command::SwitchCreate { .. } => "switch create",
        Command::SwitchNat { .. } => "switch nat",
        Command::SwitchList => "switch list",
        Command::SwitchShow { .. } => "switch show",
        Command::SwitchAttach { .. } => "switch attach",
        Command::SwitchDetach { .. } => "switch detach",
        Command::SwitchCapture { .. } => "switch capture",
        Command::SwitchDelete { .. } => "switch delete",
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
        [vm] if !vm.is_empty() => parse_vm(vm),
        [] | [_] => Err(format!("'{command}' needs a VM (its id or its name)")),
        [_, extra, ..] => Err(format!("'{command}' takes one VM (got '{extra}' as well)")),
    }
}

/// A VM argument: an id (all digits) or a name.
fn parse_vm(vm: &str) -> Result<VmRef, String> {
    match VmRef::parse(vm) {
        // No name is all digits, so an id too big for a u32 can never match a VM.
        VmRef::Name(_) if vm.bytes().all(|b| b.is_ascii_digit()) => {
            Err(format!("'{vm}' is not a VM id (ids are at most {})", u32::MAX))
        }
        vm => Ok(vm),
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
        [program] => Some(PathBuf::from(program)),
        [] => None,
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
            "--paging" => profile.paging = parse_paging(value).map_err(with)?,
            "--bios" => profile.bios = Bios::parse(value).map_err(with)?,
            "--ram" => profile.ram_size = parse_ram(value).map_err(with)?,
            "--clock" => profile.clock_hz = Some(parse_clock(value).map_err(with)?),
            "--harts" => profile.harts = parse_harts(value).map_err(with)?,
            "--disk" | "--disk-ro" => profile.devices.push(DeviceSpec::Disk(DiskSpec {
                source: parse_disk_source(value).map_err(with)?,
                read_only: *flag == "--disk-ro",
            })),
            "--net" => profile
                .devices
                .push(DeviceSpec::Net(parse_net(value).map_err(with)?)),
            "--start" => start = true,
            _ => {}
        }
    }
    let name = name.ok_or("'create' needs --name NAME")?;
    let given = |flag: &str| options.iter().find(|(f, _)| *f == flag).map(|(_, v)| v.as_str());
    let camelot = match (given("--profile"), given("--camelot")) {
        (Some(_), Some(_)) => return Err("--camelot is --profile server: give one of them".into()),
        (Some(p), None) => Some(parse_preset(p).map_err(|e| format!("--profile: {e}"))?),
        (None, Some(_)) => preset("server"),
        (None, None) => None,
    };
    if camelot.is_some() && program.is_some() {
        return Err("'create --profile' boots Camelot from its disk, and takes no program".into());
    }
    if camelot.is_some() && validate_name(&vm_disk(&name)).is_err() {
        return Err(format!(
            "--name: '{name}' is too long for a Camelot VM, whose disk is named {}",
            vm_disk(&name)
        ));
    }
    if camelot.is_none() && given("--lake").is_some() {
        return Err("'create' takes --lake only with --profile (the Lake that copies the disk)".into());
    }
    // A Camelot VM's defaults, its profile's: Camelot's firmware, and its RAM (a
    // server's, the firmware's 2 MiB on top of the platform's 4).
    if let Some(preset) = camelot {
        let machine = preset.profile();
        if given("--bios").is_none() {
            profile.bios = machine.bios;
        }
        if given("--ram").is_none() {
            profile.ram_size = machine.ram_size;
        }
    }
    let has_disk = profile.devices.iter().any(|d| matches!(d, DeviceSpec::Disk(_)));
    if program.is_none() && !has_disk && camelot.is_none() {
        return Err(
            "'create' needs the program to run (an ELF file), a --disk to boot from, or --profile".into(),
        );
    }
    Ok(Create {
        name,
        profile,
        program,
        camelot,
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
            let option = |flag: &str| options.iter().find(|(f, _)| *f == flag).map(|(_, v)| v);
            let sectors = option("--size")
                .map(|size| parse_disk_size(size).map_err(|e| format!("--size: {e}")))
                .transpose()?;
            let allowed: &[&str] = &["--lake", "--size", "--from"];
            match (option("--from"), sectors) {
                (Some(source), sectors) => {
                    validate_name(source).map_err(|e| format!("--from: {e}"))?;
                    let source = source.clone();
                    Ok((
                        Command::DiskClone {
                            name,
                            source,
                            sectors,
                        },
                        allowed,
                    ))
                }
                (None, Some(sectors)) => Ok((Command::DiskCreate { name, sectors }, allowed)),
                (None, None) => Err("'disk create' needs --size SIZE, or --from SRC to copy an image".into()),
            }
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

/// The single switch name of `switch create`, `show` and `delete`.
fn switch_argument(command: &str, rest: &[&str]) -> Result<String, String> {
    match rest {
        [name] => Ok(name.to_string()),
        [] => Err(format!("'switch {command}' needs a switch name")),
        [_, extra, ..] => Err(format!(
            "'switch {command}' takes one switch name (got '{extra}' as well)"
        )),
    }
}

fn parse_switch(
    rest: &[&str],
    options: &[(&'static str, String)],
) -> Result<(Command, &'static [&'static str]), String> {
    const WORDS: &str = "create, nat, list, show, attach, detach, capture or delete";
    let Some((&word, rest)) = rest.split_first() else {
        return Err(format!("'switch' needs a command: {WORDS}"));
    };
    let option = |flag: &str| options.iter().find(|(f, _)| *f == flag).map(|(_, v)| v.as_str());
    let card = || -> Result<u8, String> {
        match option("--card") {
            None => Ok(0),
            Some(n) => match n.parse::<u8>() {
                Ok(card) if n.bytes().all(|b| b.is_ascii_digit()) => Ok(card),
                _ => Err(format!("--card: '{n}' is not a card number (0, 1, 2 or 3)")),
            },
        }
    };
    // The --hostfwd rules, in order, as a gateway's configuration.
    let gateway = || -> Result<NatConfig, String> {
        let hostfwd = options
            .iter()
            .filter(|(f, _)| *f == "--hostfwd")
            .map(|(_, rule)| parse_hostfwd(rule).map_err(|e| format!("--hostfwd: {e}")))
            .collect::<Result<_, _>>()?;
        Ok(NatConfig { hostfwd })
    };
    let forwards = options.iter().any(|(f, _)| *f == "--hostfwd");
    match word {
        "create" => {
            let name = switch_argument(word, rest)?;
            validate_name(&name)?;
            let nat = match (option("--nat").is_some(), forwards) {
                (true, _) => Some(gateway()?),
                (false, true) => {
                    return Err("'switch create' takes --hostfwd only with --nat (ports are \
                                forwarded by the switch's NAT gateway)"
                        .into())
                }
                (false, false) => None,
            };
            Ok((
                Command::SwitchCreate { name, nat },
                &["--server", "--nat", "--hostfwd"],
            ))
        }
        "nat" => {
            let name = switch_argument(word, rest)?;
            let nat = match (option("--off").is_some(), forwards) {
                (false, _) => Some(gateway()?),
                (true, false) => None,
                (true, true) => {
                    return Err("'switch nat' takes --hostfwd or --off, not both".into());
                }
            };
            Ok((
                Command::SwitchNat { name, nat },
                &["--server", "--hostfwd", "--off"],
            ))
        }
        "list" => {
            no_arguments("switch list", rest)?;
            Ok((Command::SwitchList, &["--server"]))
        }
        "show" => Ok((
            Command::SwitchShow {
                name: switch_argument(word, rest)?,
            },
            &["--server"],
        )),
        "delete" => Ok((
            Command::SwitchDelete {
                name: switch_argument(word, rest)?,
            },
            &["--server"],
        )),
        "attach" => match rest {
            [switch, vm] if !vm.is_empty() => Ok((
                Command::SwitchAttach {
                    switch: switch.to_string(),
                    vm: parse_vm(vm)?,
                    card: card()?,
                },
                &["--server", "--card"],
            )),
            [_, _, extra, ..] => Err(format!(
                "'switch attach' takes a switch and a VM (got '{extra}' as well)"
            )),
            _ => Err("'switch attach' needs a switch and a VM".into()),
        },
        "detach" => Ok((
            Command::SwitchDetach {
                vm: vm_argument("switch detach", rest)?,
                card: card()?,
            },
            &["--server", "--card"],
        )),
        "capture" => {
            let stop = option("--stop").is_some();
            let (switch, path) = match (rest, stop) {
                ([switch, path], false) => (switch, Some(path.to_string())),
                ([switch], true) => (switch, None),
                ([], _) => return Err("'switch capture' needs a switch".into()),
                ([_], false) => return Err("'switch capture' needs a file, or --stop".into()),
                ([_, _], true) => return Err("'switch capture' takes a file or --stop, not both".into()),
                ([_, _, extra, ..], _) => {
                    return Err(format!(
                        "'switch capture' takes a switch and a file (got '{extra}' as well)"
                    ))
                }
            };
            Ok((
                Command::SwitchCapture {
                    switch: switch.to_string(),
                    path,
                },
                &["--server", "--stop"],
            ))
        }
        _ => Err(format!("unknown switch command '{word}' ({WORDS})")),
    }
}

// ---- Carrying commands out ----

/// Carries out a parsed command.
pub fn execute(invocation: &Invocation, out: &mut dyn Write) -> Result<(), Failure> {
    let text = match &invocation.command {
        Command::Help => format!("{USAGE}\n"),
        Command::Create(create) => {
            // No program: an empty one, which Avalon boots from the VM's disk 0.
            let program = match &create.program {
                Some(path) => std::fs::read(path)
                    .map_err(|e| Failure::Failed(format!("cannot read {}: {e}", path.display())))?,
                None => Vec::new(),
            };
            let mut avalon = connect("Avalon", &invocation.server)?;
            let (id, mut text) = if let Some(preset) = create.camelot {
                let mut lake = connect("Lake", &invocation.lake)?;
                let (id, disk) = create_camelot_vm(
                    &mut avalon,
                    &mut lake,
                    &invocation.lake,
                    &create.name,
                    preset.image,
                    create.profile.clone(),
                )
                .map_err(|e| Failure::Failed(e.to_string()))?;
                (id, format!("created disk {disk} (a copy of {})\n", preset.image))
            } else {
                let id = avalon
                    .create_vm(&create.name, create.profile.clone(), program)
                    .map_err(failed)?;
                (id, String::new())
            };
            text += &format!("created VM {id} ({})\n", create.name);
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
        Command::Profiles => profile_table(&PRESETS),
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
        Command::DiskClone {
            name,
            source,
            sectors,
        } => {
            let mut lake = connect("Lake", &invocation.lake)?;
            lake.clone_disk(name, source, *sectors).map_err(failed)?;
            let size = lake
                .list_disks()
                .map_err(failed)?
                .into_iter()
                .find(|d| d.name == *name)
                .map_or_else(String::new, |d| format!(", {}", sectors_text(d.sectors)));
            format!("created disk {name} (a copy of {source}{size})\n")
        }
        Command::DiskList => disk_table(&connect("Lake", &invocation.lake)?.list_disks().map_err(failed)?),
        Command::DiskDelete { name } => {
            connect("Lake", &invocation.lake)?
                .delete_disk(name)
                .map_err(failed)?;
            format!("deleted disk {name}\n")
        }
        Command::SwitchCreate { name, nat } => {
            let mut avalon = connect("Avalon", &invocation.server)?;
            avalon.create_switch_with(name, nat.clone()).map_err(failed)?;
            match nat {
                None => format!("created switch {name}\n"),
                Some(_) => format!(
                    "created switch {name} with a NAT gateway\n{}",
                    forwarded(&mut avalon, name)?
                ),
            }
        }
        Command::SwitchNat { name, nat } => {
            let mut avalon = connect("Avalon", &invocation.server)?;
            avalon.set_switch_nat(name, nat.clone()).map_err(failed)?;
            match nat {
                None => format!("took the NAT gateway of switch {name} away\n"),
                Some(_) => format!(
                    "gave switch {name} a NAT gateway\n{}",
                    forwarded(&mut avalon, name)?
                ),
            }
        }
        Command::SwitchList => switch_table(
            &connect("Avalon", &invocation.server)?
                .list_switches()
                .map_err(failed)?,
        ),
        Command::SwitchShow { name } => {
            let switches = connect("Avalon", &invocation.server)?
                .list_switches()
                .map_err(failed)?;
            match switches.iter().find(|s| s.name == *name) {
                Some(switch) => switch_details(switch),
                None => {
                    return Err(Failure::Failed(format!(
                        "no such switch: no switch named '{name}'"
                    )))
                }
            }
        }
        Command::SwitchAttach { switch, vm, card } => {
            connect("Avalon", &invocation.server)?
                .attach_card(vm, *card, switch)
                .map_err(failed)?;
            format!("plugged card {card} of VM {vm} into switch {switch}\n")
        }
        Command::SwitchDetach { vm, card } => {
            connect("Avalon", &invocation.server)?
                .detach_card(vm, *card)
                .map_err(failed)?;
            format!("unplugged card {card} of VM {vm}\n")
        }
        Command::SwitchCapture { switch, path } => {
            connect("Avalon", &invocation.server)?
                .capture_switch(switch, path.as_deref())
                .map_err(failed)?;
            match path {
                Some(path) => format!("capturing switch {switch} into {path}\n"),
                None => format!("stopped capturing switch {switch}\n"),
            }
        }
        Command::SwitchDelete { name } => {
            connect("Avalon", &invocation.server)?
                .delete_switch(name)
                .map_err(failed)?;
            format!("deleted switch {name}\n")
        }
    };
    write_out(out, &text)
}

/// The ports the gateway of switch `name` forwards, one line each, as bound (a host
/// port 0 is the one the system picked).
fn forwarded(avalon: &mut Client, name: &str) -> Result<String, Failure> {
    let switches = avalon.list_switches().map_err(failed)?;
    let rules = switches
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.nat.as_ref())
        .map(|nat| nat.hostfwd.as_slice())
        .unwrap_or_default();
    Ok(rules.iter().map(|r| format!("forwarding {r}\n")).collect())
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
    profile.devices.iter().filter_map(|d| match d {
        DeviceSpec::Disk(disk) => Some(disk),
        _ => None,
    })
}

fn nets(profile: &HardwareProfile) -> impl Iterator<Item = &NetSpec> {
    profile.devices.iter().filter_map(|d| match d {
        DeviceSpec::Net(net) => Some(net),
        _ => None,
    })
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
                nets(&vm.profile).count().to_string(),
            ]
        })
        .collect();
    table(&["ID", "NAME", "STATE", "RAM", "CLOCK", "DISKS", "NETS"], &rows)
}

/// A field of `info` that lists several values: one per line, the label on the first,
/// `none` for none.
fn field_lines(label: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return format!("{label:<9}none\n");
    }
    let mut text = String::new();
    for (i, line) in lines.iter().enumerate() {
        text += &format!("{:<9}{line}\n", if i == 0 { label } else { "" });
    }
    text
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
    // The firmware, for a VM that has one (QEMU's -bios): none is the default.
    let firmware = match &vm.profile.bios {
        Bios::None => String::new(),
        bios => format!("firmware {bios}\n"),
    };
    let mut text = format!(
        "id       {}\nname     {}\nstate    {}\narch     {}\n{firmware}RAM      {}\nclock    {clock}\n\
         harts    {}\nsteps    {steps}\n",
        vm.id,
        vm.name,
        vm.state,
        arch_text(vm.profile.paging),
        format_size(vm.profile.ram_size),
        vm.profile.harts,
    );
    let lines: Vec<String> = disks(&vm.profile)
        .map(|disk| {
            let mode = if disk.read_only { "read-only" } else { "read-write" };
            format!("{} ({mode})", disk_source_text(&disk.source))
        })
        .collect();
    text += &field_lines("disks", &lines);
    // A server always says each card's address; a missing one shows as such.
    let lines: Vec<String> = nets(&vm.profile)
        .map(|net| {
            let mac = net.mac.map_or_else(|| "no address".into(), mac_text);
            match &net.switch {
                Some(switch) => format!("{mac} on switch {switch}"),
                None => format!("{mac} unplugged"),
            }
        })
        .collect();
    text += &field_lines("nets", &lines);
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

/// The hardware profiles of Camelot VMs, as `profiles` prints them.
pub fn profile_table(presets: &[Preset]) -> String {
    let rows: Vec<Vec<String>> = presets
        .iter()
        .map(|p| {
            let machine = p.profile();
            vec![
                p.name.to_string(),
                p.image.to_string(),
                format_size(machine.ram_size),
                machine.bios.to_string(),
                p.about.to_string(),
            ]
        })
        .collect();
    table(&["NAME", "IMAGE", "RAM", "BIOS", "ABOUT"], &rows)
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

/// `switch list`: one row per switch.
pub fn switch_table(switches: &[SwitchInfo]) -> String {
    if switches.is_empty() {
        return "no switches\n".into();
    }
    let rows: Vec<Vec<String>> = switches
        .iter()
        .map(|s| {
            vec![
                s.name.clone(),
                s.ports.len().to_string(),
                if s.nat.is_some() { "yes" } else { "no" }.into(),
                s.capture.clone().unwrap_or_else(|| "no".into()),
            ]
        })
        .collect();
    table(&["NAME", "PORTS", "NAT", "CAPTURE"], &rows)
}

/// `switch show`: a switch, its gateway, and its ports' counters.
pub fn switch_details(switch: &SwitchInfo) -> String {
    let mut text = format!(
        "switch   {}\ncapture  {}\n",
        switch.name,
        switch.capture.as_deref().unwrap_or("no")
    );
    match &switch.nat {
        None => text += "nat      no\n",
        Some(nat) => {
            text += "nat      10.0.2.0/24, gateway and host 10.0.2.2, DNS 10.0.2.3\n";
            for rule in &nat.hostfwd {
                text += &format!("forward  {rule}\n");
            }
            for lease in &nat.leases {
                text += &format!("lease    {} {}\n", lease.addr, mac_text(lease.mac));
            }
        }
    }
    if switch.ports.is_empty() {
        return text + "ports    none\n";
    }
    text += &format!("ports    {}\n", switch.ports.len());
    let rows: Vec<Vec<String>> = switch
        .ports
        .iter()
        .map(|p| {
            let (vm, card) = match p.owner {
                PortOwner::Card { vm, card } => (vm.to_string(), card.to_string()),
                PortOwner::Gateway => ("gateway".into(), "-".into()),
            };
            vec![
                vm,
                card,
                mac_text(p.mac),
                p.in_frames.to_string(),
                p.in_bytes.to_string(),
                p.out_frames.to_string(),
                p.out_bytes.to_string(),
                p.dropped.to_string(),
            ]
        })
        .collect();
    text + &table(
        &[
            "VM",
            "CARD",
            "MAC",
            "IN",
            "IN-BYTES",
            "OUT",
            "OUT-BYTES",
            "DROPPED",
        ],
        &rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use avalon_proto::{DeviceEntry, DiskSource, Paging, SwitchPort, VmState};

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
            "create --name web --ram 64 --clock 10MHz --harts 4 --disk lake://lake:7401/data --disk-ro boot.img --start prog.elf",
        ) else {
            panic!()
        };
        assert_eq!(c.name, "web");
        assert_eq!(c.program, Some(PathBuf::from("prog.elf")));
        assert!(c.start);
        assert_eq!(c.profile.ram_size, 64 << 20);
        assert_eq!(c.profile.clock_hz, Some(10_000_000));
        assert_eq!(c.profile.nominal_hz, avalon_proto::DEFAULT_NOMINAL_HZ);
        assert_eq!(c.profile.harts, 4);
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
        // 4 MiB of RAM, as the usage says.
        assert_eq!(c.profile.ram_size, 4 << 20);
        assert_eq!(avalon_proto::DEFAULT_RAM_SIZE, 4 << 20);
        let mib = avalon_proto::DEFAULT_RAM_SIZE >> 20;
        assert!(USAGE.contains(&format!(
            "(default {mib} MiB;\n                  a Camelot VM has its profile's"
        )));
        assert!(!c.start);
        // No program: the VM boots from its disk.
        let Command::Create(c) = command("create --name os --disk root.img") else {
            panic!()
        };
        assert_eq!(c.program, None);
        assert_eq!(c.profile.devices.len(), 1);
        // A program whose name starts with '-', after '--'.
        let Command::Create(c) = command("create --name web -- -prog.elf") else {
            panic!()
        };
        assert_eq!(c.program, Some(PathBuf::from("-prog.elf")));
        assert!(c.camelot.is_none());
        // Camelot from a copy of Lake's image: no program, no disk needed; Lake's
        // address is the invocation's.
        let inv = parse("create --name os --camelot --lake store --net lan --start").unwrap();
        assert_eq!(inv.lake, "store:7401");
        let Command::Create(c) = inv.command else { panic!() };
        assert_eq!(c.camelot, preset("server"));
        assert!(c.start);
        assert_eq!((c.name.as_str(), c.program), ("os", None));
        assert_eq!(c.profile.devices.len(), 1, "the card; the disk comes from Lake");
        let Command::Create(c) = command("create --camelot --name os --disk-ro data.img") else {
            panic!()
        };
        assert_eq!(c.camelot.map(|p| p.image), Some(avalon_client::BASE_IMAGE));
        assert_eq!(c.profile.devices.len(), 1);
        // A profile by name: the desktop's image and RAM.
        let Command::Create(c) = command("create --name os --profile desktop") else {
            panic!()
        };
        assert_eq!(c.camelot.map(|p| p.image), Some(avalon_client::DESKTOP_IMAGE));
        assert_eq!(
            (c.profile.bios, c.profile.ram_size),
            (Bios::Default, avalon_client::DESKTOP_RAM)
        );
        let Command::Create(c) = command("create --name os --profile=server") else {
            panic!()
        };
        assert_eq!(c.camelot, preset("server"));
        // Sv39 alone; up to 1 TiB of RAM.
        let Command::Create(c) = command("create --name os --camelot --paging sv39 --ram 1T") else {
            panic!()
        };
        assert_eq!((c.profile.paging, c.profile.ram_size), (Paging::Sv39, 1 << 40));
        let Command::Create(c) = command("create --name os --paging=sv48 --disk a.img") else {
            panic!()
        };
        assert_eq!(c.profile.paging, Paging::Sv48);
        // The firmware: none unless asked, as QEMU's -bios; a Camelot VM has
        // Camelot's, and the firmware's 2 MiB of RAM on top of 4, unless told.
        assert_eq!(c.profile.bios, Bios::None);
        let bios = |line: &str| match command(line) {
            Command::Create(c) => (c.profile.bios, c.profile.ram_size >> 20),
            _ => panic!(),
        };
        assert_eq!(bios("create --name os --camelot"), (Bios::Default, 6));
        assert_eq!(
            bios("create --name os --camelot --bios none --ram 16"),
            (Bios::None, 16)
        );
        assert_eq!(
            bios("create --name os --profile desktop --ram 32"),
            (Bios::Default, 32)
        );
        // The usage gives each profile's RAM.
        assert!(USAGE.contains(&format!(
            "{} MiB for a server, whose\n                  firmware keeps 2 MiB, {} MiB for a desktop)",
            avalon_client::CAMELOT_RAM >> 20,
            avalon_client::DESKTOP_RAM >> 20
        )));
        assert_eq!(
            bios("create --name os --bios fw_jump.bin --disk a.img"),
            (Bios::File("fw_jump.bin".into()), 4)
        );
        assert_eq!(bios("create --name os --bios default p.elf"), (Bios::Default, 4));
        // There is no width to choose: --xlen is no option.
        assert!(parse("create --name os --xlen 64 p.elf").is_err());
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
        assert_eq!(
            command("disk create web-disk0 --from os-base"),
            Command::DiskClone {
                name: "web-disk0".into(),
                source: "os-base".into(),
                sectors: None
            }
        );
        assert_eq!(
            command("disk create big --size 1G --from=os-base"),
            Command::DiskClone {
                name: "big".into(),
                source: "os-base".into(),
                sectors: Some(2_097_152)
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
                "'create' needs the program to run (an ELF file), a --disk to boot from, or --profile",
            ),
            (
                "create --name a --camelot a.elf",
                "'create --profile' boots Camelot from its disk, and takes no program",
            ),
            ("create --name a --camelot=yes", "--camelot takes no value"),
            (
                "--server x disk create a --from b",
                "'disk create' does not take --server",
            ),
            ("disk create a --from B", "--from: 'B' is not a valid name"),
            (
                "disk create a --from b --size 1",
                "--size: 1 is not a whole number",
            ),
            ("disk create a --from b --from c", "--from is given twice"),
            ("disk list --from b", "'disk list' does not take --from"),
            ("list --camelot", "'list' does not take --camelot"),
            (
                "create --name a --profile laptop",
                "--profile: no profile 'laptop' (server or desktop)",
            ),
            (
                "create --name a --camelot --profile server",
                "--camelot is --profile server: give one of them",
            ),
            ("create --name a --profile", "--profile needs a value"),
            ("profiles all", "'profiles' takes no arguments (got 'all')"),
            ("profiles --ram 2", "'profiles' does not take --ram"),
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
            ("create a.elf --name a --ram 0", "--ram: 0 is out of range"),
            (
                "create a.elf --name a --ram 64MB",
                "--ram: '64MB' is not a RAM size",
            ),
            (
                "create a.elf --name a --clock fast",
                "--clock: 'fast' is not a frequency",
            ),
            (
                "create a.elf --name a --harts 9",
                "--harts: 9 harts is out of range (1 to 8)",
            ),
            ("list --harts 2", "'list' does not take --harts"),
            ("create a.elf --name a --disk lake://x/", "--disk: 'lake://x/'"),
            ("create a.elf --name a --disk", "--disk needs a value"),
            ("disk", "'disk' needs a command"),
            ("disk resize a", "unknown disk command 'resize'"),
            (
                "disk create a",
                "'disk create' needs --size SIZE, or --from SRC to copy an image",
            ),
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
            (
                "create a.elf --name a --lake x",
                "'create' takes --lake only with --profile",
            ),
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
            harts: 2,
            paging: Paging::Sv48,
            bios: avalon_proto::Bios::Default,
            devices: vec![
                disk("lake://10.0.0.9:7401/data", false),
                DeviceSpec::Net(parse_net("lan,mac=02:ca:00:00:00:01").unwrap()),
                disk("boot.img", true),
                DeviceSpec::Net(parse_net("-,mac=02:ca:00:00:00:02").unwrap()),
            ],
        };
        let vms = [
            summary(1, "web", VmState::Running, web.clone()),
            summary(12, "batch-job", VmState::Exited(3), HardwareProfile::default()),
        ];
        assert_eq!(
            vm_table(&vms),
            "\
ID  NAME       STATE      RAM     CLOCK       DISKS  NETS
1   web        running    64 MiB  1.5 MHz     2      2
12  batch-job  exited(3)  4 MiB   full speed  0      0
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
arch     RV64 (Sv48 and Sv39 paging)
firmware default
RAM      64 MiB
clock    1.5 MHz (real time)
harts    2
steps    12345
disks    lake://10.0.0.9:7401/data (read-write)
         boot.img (read-only)
nets     02:ca:00:00:00:01 on switch lan
         02:ca:00:00:00:02 unplugged
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
        assert!(
            text.contains("\narch     RV64 (Sv48 and Sv39 paging)\n"),
            "{text}"
        );
        assert!(
            text.contains("\ndisks    none\nnets     none\ndevices  0\n"),
            "{text}"
        );
    }

    #[test]
    fn switch_commands() {
        let name = |n: &str| n.to_string();
        assert_eq!(
            command("switch create lan"),
            Command::SwitchCreate {
                name: name("lan"),
                nat: None
            }
        );
        let rule = |text: &str| avalon_client::parse_hostfwd(text).unwrap();
        assert_eq!(
            command("switch create wan --nat"),
            Command::SwitchCreate {
                name: name("wan"),
                nat: Some(NatConfig::default())
            }
        );
        assert_eq!(
            command("switch create wan --nat --hostfwd tcp::8080-:80 --hostfwd=udp::5353-10.0.2.16:53"),
            Command::SwitchCreate {
                name: name("wan"),
                nat: Some(NatConfig {
                    hostfwd: vec![rule("tcp::8080-:80"), rule("udp::5353-10.0.2.16:53")]
                })
            }
        );
        assert_eq!(
            command("switch nat wan --hostfwd 2222-10.0.2.16:22"),
            Command::SwitchNat {
                name: name("wan"),
                nat: Some(NatConfig {
                    hostfwd: vec![rule("tcp:127.0.0.1:2222-10.0.2.16:22")]
                })
            }
        );
        assert_eq!(
            command("switch nat wan"),
            Command::SwitchNat {
                name: name("wan"),
                nat: Some(NatConfig::default())
            }
        );
        assert_eq!(
            command("switch nat wan --off"),
            Command::SwitchNat {
                name: name("wan"),
                nat: None
            }
        );
        assert_eq!(command("switch list"), Command::SwitchList);
        assert_eq!(
            command("switch show lan"),
            Command::SwitchShow { name: name("lan") }
        );
        assert_eq!(
            command("switch delete lan"),
            Command::SwitchDelete { name: name("lan") }
        );
        assert_eq!(
            command("switch attach lan web"),
            Command::SwitchAttach {
                switch: name("lan"),
                vm: VmRef::Name(name("web")),
                card: 0
            }
        );
        assert_eq!(
            command("switch attach lan 3 --card 2"),
            Command::SwitchAttach {
                switch: name("lan"),
                vm: VmRef::Id(3),
                card: 2
            }
        );
        assert_eq!(
            command("switch detach --card=1 web"),
            Command::SwitchDetach {
                vm: VmRef::Name(name("web")),
                card: 1
            }
        );
        assert_eq!(
            command("switch capture lan lan.pcap"),
            Command::SwitchCapture {
                switch: name("lan"),
                path: Some(name("lan.pcap"))
            }
        );
        assert_eq!(
            command("switch capture lan --stop"),
            Command::SwitchCapture {
                switch: name("lan"),
                path: None
            }
        );
        assert_eq!(
            parse("--server 10.0.0.2 switch list").unwrap().server,
            "10.0.0.2:7400"
        );
        // Cards for create.
        let Command::Create(c) = command("create p.elf --name web --net lan --net -,mac=02:00:00:00:00:07")
        else {
            panic!()
        };
        assert_eq!(
            c.profile.devices,
            [
                DeviceSpec::Net(NetSpec {
                    switch: Some(name("lan")),
                    mac: None
                }),
                DeviceSpec::Net(NetSpec {
                    switch: None,
                    mac: Some([2, 0, 0, 0, 0, 7])
                }),
            ]
        );
        for (args, why) in [
            ("switch", "'switch' needs a command: create, nat, list, show"),
            ("switch flip lan", "unknown switch command 'flip'"),
            ("switch create", "'switch create' needs a switch name"),
            ("switch create Lan", "'Lan' is not a valid name"),
            (
                "switch create a b",
                "'switch create' takes one switch name (got 'b' as well)",
            ),
            ("switch list x", "'switch list' takes no arguments (got 'x')"),
            ("switch show", "'switch show' needs a switch name"),
            ("switch attach lan", "'switch attach' needs a switch and a VM"),
            (
                "switch attach lan a b",
                "'switch attach' takes a switch and a VM (got 'b' as well)",
            ),
            ("switch attach lan 99999999999", "'99999999999' is not a VM id"),
            ("switch attach lan a --card x", "--card: 'x' is not a card number"),
            (
                "switch attach lan a --card 256",
                "--card: '256' is not a card number",
            ),
            (
                "switch attach lan a --card +1",
                "--card: '+1' is not a card number",
            ),
            ("switch detach", "'switch detach' needs a VM"),
            ("switch capture", "'switch capture' needs a switch"),
            ("switch capture lan", "'switch capture' needs a file, or --stop"),
            (
                "switch capture lan f --stop",
                "'switch capture' takes a file or --stop, not both",
            ),
            (
                "switch capture lan f g",
                "'switch capture' takes a switch and a file (got 'g' as well)",
            ),
            ("switch list --lake x", "'switch list' does not take --lake"),
            ("switch show lan --card 1", "'switch show' does not take --card"),
            ("switch detach a --stop", "'switch detach' does not take --stop"),
            (
                "switch create wan --hostfwd tcp::1-:2",
                "'switch create' takes --hostfwd only with --nat",
            ),
            (
                "switch create wan --nat --hostfwd tcp::1-:0",
                "--hostfwd: 'tcp::1-:0' is not a forwarding rule (the guest port cannot be 0",
            ),
            (
                "switch create wan --nat --hostfwd 8080",
                "--hostfwd: '8080' is not a forwarding rule (no '-'",
            ),
            ("switch create wan --off", "'switch create' does not take --off"),
            ("switch create wan --nat --nat", "--nat is given twice"),
            ("switch nat", "'switch nat' needs a switch name"),
            (
                "switch nat wan --off --hostfwd 1-:2",
                "'switch nat' takes --hostfwd or --off, not both",
            ),
            ("switch nat wan --nat", "'switch nat' does not take --nat"),
            (
                "switch show wan --hostfwd 1-:2",
                "'switch show' does not take --hostfwd",
            ),
            (
                "create a.elf --name a --hostfwd 1-:2",
                "'create' does not take --hostfwd",
            ),
            ("list --card 1", "'list' does not take --card"),
            (
                "create a.elf --name a --net LAN",
                "--net: 'LAN': switch 'LAN' is not a valid name",
            ),
            (
                "create a.elf --name a --net lan,mac=01:00:00:00:00:01",
                "--net: 01:00:00:00:00:01 is a group",
            ),
        ] {
            match parse(args) {
                Err(e) => assert!(e.starts_with(why), "{args}: {e}"),
                Ok(inv) => panic!("{args}: parsed as {inv:?}"),
            }
        }
    }

    #[test]
    fn switch_output() {
        assert_eq!(switch_table(&[]), "no switches\n");
        let port = |vm, card, last, n: u64| SwitchPort {
            owner: PortOwner::Card { vm, card },
            mac: [0x02, 0xca, 0, 0, 0, last],
            in_frames: n,
            in_bytes: 60 * n,
            out_frames: n + 1,
            out_bytes: 60 * (n + 1),
            dropped: n / 2,
        };
        let lan = SwitchInfo {
            name: "lan".into(),
            capture: Some("lan.pcap".into()),
            nat: None,
            ports: vec![port(1, 0, 1, 3), port(12, 1, 2, 1000)],
        };
        let dmz = SwitchInfo {
            name: "dmz".into(),
            capture: None,
            nat: None,
            ports: Vec::new(),
        };
        let wan = SwitchInfo {
            name: "wan".into(),
            capture: None,
            nat: Some(avalon_proto::NatInfo {
                hostfwd: vec![avalon_client::parse_hostfwd("tcp::5555-10.0.2.16:7").unwrap()],
                leases: vec![
                    avalon_proto::Lease {
                        mac: [0x02, 0xca, 0, 0, 0, 1],
                        addr: "10.0.2.15".parse().unwrap(),
                    },
                    avalon_proto::Lease {
                        mac: [0x02, 0xca, 0, 0, 0, 2],
                        addr: "10.0.2.16".parse().unwrap(),
                    },
                ],
            }),
            ports: vec![
                SwitchPort {
                    owner: PortOwner::Gateway,
                    mac: [0x52, 0x55, 0x0a, 0, 2, 2],
                    ..port(0, 0, 0, 7)
                },
                port(3, 0, 1, 5),
            ],
        };
        assert_eq!(
            switch_table(&[dmz.clone(), lan.clone(), wan.clone()]),
            "\
NAME  PORTS  NAT  CAPTURE
dmz   0      no   no
lan   2      no   lan.pcap
wan   2      yes  no
"
        );
        assert_eq!(
            switch_details(&lan),
            "\
switch   lan
capture  lan.pcap
nat      no
ports    2
VM  CARD  MAC                IN    IN-BYTES  OUT   OUT-BYTES  DROPPED
1   0     02:ca:00:00:00:01  3     180       4     240        1
12  1     02:ca:00:00:00:02  1000  60000     1001  60060      500
"
        );
        assert_eq!(
            switch_details(&dmz),
            "switch   dmz\ncapture  no\nnat      no\nports    none\n"
        );
        assert_eq!(
            switch_details(&wan),
            "\
switch   wan
capture  no
nat      10.0.2.0/24, gateway and host 10.0.2.2, DNS 10.0.2.3
forward  tcp:127.0.0.1:5555-10.0.2.16:7
lease    10.0.2.15 02:ca:00:00:00:01
lease    10.0.2.16 02:ca:00:00:00:02
ports    2
VM       CARD  MAC                IN  IN-BYTES  OUT  OUT-BYTES  DROPPED
gateway  -     52:55:0a:00:02:02  7   420       8    480        3
3        0     02:ca:00:00:00:01  5   300       6    360        2
"
        );
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
