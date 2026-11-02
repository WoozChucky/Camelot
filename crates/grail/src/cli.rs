//! The `grail` command line: its options, parsed into the machine they describe. In the
//! library so that other programs can start a machine from the same arguments
//! (`merlin-dbg -- grail-args...` runs one in its own process).

use crate::config::DiskSource;
use crate::net::Hub;
use crate::{parse_clock, parse_harts, parse_ram_mib, DiskConfig, Machine, MachineConfig, NetConfig};
use std::net::SocketAddr;
use std::path::PathBuf;

pub const USAGE: &str = "usage: grail [options] [program.elf]

Runs the program. Without one, boots from disk 0 (--disk): the boot ROM loads
its boot block (its first 1 KiB) and runs it, as camelot-fs mkfs --boot makes
for the Camelot kernel (spec section 7, \"Boot from disk\").

  --trace         print every instruction before it runs (with its hart's
                  number, h0, h1, ..., on a machine of several harts)
  --max-steps N   stop after N steps (exit code 124)
  --ram MiB       RAM size in MiB, 1 to 2048 (default 4)
  --clock FREQ    real time: run FREQ instructions per second, and mtime ticks at FREQ.
                  FREQ is a number with an optional unit, Hz, kHz or MHz (any case;
                  no unit = Hz); fractions are fine if the result is whole Hz:
                  --clock 10MHz, --clock 500kHz, --clock 1.5MHz, --clock 5Hz.
                  Without it Grail runs at full speed, deterministically, and reports
                  a nominal 10 MHz clock to software
  --harts N       N harts (cores), 1 to 8 (default 1). They share memory and
                  devices and take turns, one instruction each per step, so
                  with --clock each runs at FREQ. Hart 0 starts the program;
                  the others wait in the boot ROM until an IPI (spec section 7)
  --disk IMG      attach the image file IMG as the next disk (spec section 9.6),
                  read-write: the program's writes change the file. IMG must be a
                  whole number of 512-byte sectors. Repeat for more disks (up to 8).
                  IMG may also be lake://host:port/name, image 'name' on a Lake
                  storage server (port 7401 if left out)
  --disk-ro IMG   the same, read-only
  --net hub[,mac=ADDR]
                  add a network card (spec section 9.8) on a port of this process's
                  Ethernet hub; every card on the hub sees the frames the others send.
                  Repeat for more cards (up to 4). The MAC address is
                  02:ca:00:00:00:NN for port NN unless mac= sets one
  --net user[,hostfwd=RULE...][,mac=ADDR]
                  add a network card with user networking: a NAT gateway in Grail
                  gives it 10.0.2.15 by DHCP, DNS at 10.0.2.3, and TCP and UDP to the
                  internet; 10.0.2.2 is the host's 127.0.0.1. Each hostfwd forwards a
                  host port to the guest: [tcp|udp]:[HOSTADDR]:HOSTPORT-[GUESTADDR]:GUESTPORT,
                  e.g. hostfwd=tcp::8080-:80 (HOSTADDR 127.0.0.1, GUESTADDR 10.0.2.15
                  unless given). MAC 52:54:00:12:34:56 unless mac= sets one
  --pcap FILE     write every frame the card of the last --net sends or receives to
                  FILE, a pcap capture (Wireshark, tcpdump)
  --fb            show the framebuffer in a window (Windows only)
  --headless      no window, even with --fb (the default)
  --fb-dump DIR   write DIR/frame-NNNN.ppm on every FB_PRESENT and at exit
  --gdb ADDR      run under a GDB remote stub listening on ADDR, for GDB, LLDB or
                  merlin-dbg (docs/merlin-dbg.md): HOST:PORT, a host (port 1234) or
                  a port (on 127.0.0.1). The machine runs and a client that connects
                  stops it. Anyone who can connect controls the machine: keep ADDR on
                  127.0.0.1 unless the network is trusted. No --fb, --fb-dump or
                  --max-steps with it
  --gdb-wait      with --gdb (127.0.0.1:1234 if not given): start stopped at the
                  first instruction, waiting for a client";

/// Where `--gdb-wait` alone listens.
pub const GDB_DEFAULT: &str = "127.0.0.1:1234";

/// What the command line asks for.
pub struct Options {
    pub trace: bool,
    pub max_steps: Option<u64>,
    pub config: MachineConfig,
    /// The program to run; `None` boots from disk 0.
    pub program: Option<String>,
    pub fb: bool,
    pub headless: bool,
    pub dump_dir: Option<PathBuf>,
    /// `--gdb`'s address (or the default one, with `--gdb-wait` alone).
    pub gdb: Option<SocketAddr>,
    pub gdb_wait: bool,
}

/// Why the command line was not run.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// `-h`/`--help`.
    Help,
    /// A mistake: the message, and whether the usage should follow it.
    Usage(String, bool),
}

fn usage(msg: impl Into<String>) -> Error {
    Error::Usage(msg.into(), true)
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Usage(msg.into(), false)
}

/// Parses `grail`'s arguments (without the program name).
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, Error> {
    let mut o = Options {
        trace: false,
        max_steps: None,
        config: MachineConfig::default(),
        program: None,
        fb: false,
        headless: false,
        dump_dir: None,
        gdb: None,
        gdb_wait: false,
    };
    // The hub every `--net hub` card plugs into.
    let hub = Hub::new();
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| usage(format!("{name} needs a value")));
        let config = &mut o.config;
        match a.as_str() {
            "--trace" => o.trace = true,
            "--fb" => o.fb = true,
            "--headless" => o.headless = true,
            "--fb-dump" => {
                let dir = value("--fb-dump").map_err(|_| bad("--fb-dump needs a directory"))?;
                o.dump_dir = Some(dir.into());
            }
            "--max-steps" => {
                let s = value("--max-steps")?;
                let n = s
                    .parse()
                    .map_err(|_| bad(format!("--max-steps: '{s}' is not a number")))?;
                o.max_steps = Some(n);
            }
            "--ram" => config.ram_size = parsed("--ram", value("--ram")?, parse_ram_mib)?,
            "--clock" => config.clock_hz = Some(parsed("--clock", value("--clock")?, parse_clock)?),
            "--harts" => config.harts = parsed("--harts", value("--harts")?, parse_harts)?,
            "--disk" | "--disk-ro" => {
                let disk = parsed(&a, value(&a)?, DiskConfig::parse)?;
                config.disks.push(disk.with_read_only(a == "--disk-ro"));
            }
            "--net" => {
                let net = parsed("--net", value("--net")?, |s| NetConfig::parse(s, &hub))?;
                config.nets.push(net);
            }
            "--pcap" => match (args.next(), config.nets.last_mut()) {
                (Some(file), Some(net)) if net.pcap.is_none() => net.pcap = Some(file.into()),
                (Some(_), Some(_)) => return Err(bad("--pcap: that card is already captured")),
                (Some(_), None) => return Err(bad("--pcap needs a --net before it")),
                (None, _) => return Err(bad("--pcap needs a file")),
            },
            "--gdb" => o.gdb = Some(parsed("--gdb", value("--gdb")?, parse_gdb_addr)?),
            "--gdb-wait" => o.gdb_wait = true,
            "-h" | "--help" => return Err(Error::Help),
            _ if a.starts_with('-') => return Err(usage(format!("unknown option {a}"))),
            _ => o.program = Some(a),
        }
    }
    if o.gdb_wait && o.gdb.is_none() {
        o.gdb = Some(parse_gdb_addr(GDB_DEFAULT).expect("the default address parses"));
    }
    if o.gdb.is_some() && (o.fb || o.dump_dir.is_some() || o.max_steps.is_some()) {
        return Err(bad("--gdb cannot be used with --fb, --fb-dump or --max-steps"));
    }
    if o.program.is_none() && o.config.disks.is_empty() {
        return Err(usage("give a program to run, or a disk to boot from"));
    }
    Ok(o)
}

/// `value` parsed by `parse`, or an error naming the option.
fn parsed<T>(name: &str, value: String, parse: impl Fn(&str) -> Result<T, String>) -> Result<T, Error> {
    parse(&value).map_err(|e| bad(format!("{name}: {e}")))
}

/// `--gdb`'s address: `host:port`, a host or IP address (port 1234), or a port alone
/// (on 127.0.0.1).
pub fn parse_gdb_addr(s: &str) -> Result<SocketAddr, String> {
    if let Ok(port) = s.parse::<u16>() {
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    avalon_proto::parse_listen(s, 1234)
}

impl Options {
    /// What the machine is called: the program, or the disk it boots from.
    pub fn shown(&self) -> String {
        match &self.program {
            Some(p) => p.clone(),
            None => disk_name(&self.config.disks[0].source),
        }
    }

    /// Builds the machine (consuming the config: its disks and cards are opened) and
    /// loads the program into it, or points it at disk 0. Errors are ready to print
    /// after `grail: `.
    pub fn machine(&mut self) -> Result<Machine, String> {
        let file = match &self.program {
            Some(path) => Some(std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?),
            None => None,
        };
        let shown = self.shown();
        let mut m = Machine::new(std::mem::take(&mut self.config))?;
        match &file {
            Some(file) => m.load_elf(file),
            None => m.boot_from_disk(),
        }
        .map_err(|e| format!("{shown}: {e}"))?;
        Ok(m)
    }
}

/// A disk as the command line named it.
fn disk_name(source: &DiskSource) -> String {
    match source {
        DiskSource::File(path) => path.display().to_string(),
        DiskSource::Lake { addr, name } => format!("lake://{addr}/{name}"),
        DiskSource::Memory(_) => "disk 0".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn gdb_options() {
        let o = parse(args("--gdb 4000 p.elf")).unwrap();
        assert_eq!(o.gdb, Some("127.0.0.1:4000".parse().unwrap()));
        assert!(!o.gdb_wait);
        let o = parse(args("--gdb 0.0.0.0:5000 --gdb-wait p.elf")).unwrap();
        assert_eq!(o.gdb, Some("0.0.0.0:5000".parse().unwrap()));
        assert!(o.gdb_wait);
        let o = parse(args("--gdb 127.0.0.1 p.elf")).unwrap();
        assert_eq!(o.gdb, Some("127.0.0.1:1234".parse().unwrap()));
        let o = parse(args("--gdb-wait p.elf")).unwrap();
        assert_eq!(o.gdb, Some("127.0.0.1:1234".parse().unwrap()));
        assert!(matches!(parse(args("--gdb")), Err(Error::Usage(m, true)) if m == "--gdb needs a value"));
        assert!(parse(args("--gdb 1234 --max-steps 5 p.elf")).is_err());
        assert!(parse(args("--gdb 1234 --fb p.elf")).is_err());
        assert!(parse(args("--gdb 'not an address' p.elf")).is_err());
    }

    #[test]
    fn programs_and_errors() {
        let o = parse(args("--harts 2 --ram 8 --trace p.elf")).unwrap();
        assert_eq!(o.program.as_deref(), Some("p.elf"));
        assert_eq!((o.config.harts, o.config.ram_size, o.trace), (2, 8 << 20, true));
        assert_eq!(parse(args("-h")).err(), Some(Error::Help));
        assert_eq!(
            parse(args("")).err(),
            Some(usage("give a program to run, or a disk to boot from"))
        );
        assert_eq!(
            parse(args("--bogus")).err(),
            Some(usage("unknown option --bogus"))
        );
        assert_eq!(
            parse(args("--pcap x p.elf")).err(),
            Some(bad("--pcap needs a --net before it"))
        );
    }
}
