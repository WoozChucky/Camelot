//! `grail [options] <program.elf>`

use grail::console;
use grail::display::{self, DisplayOptions, Outcome};
use grail::net::Hub;
use grail::{DiskConfig, Machine, MachineConfig, NetConfig, RunResult};
use std::process::ExitCode;

const USAGE: &str = "usage: grail [options] <program.elf>

  --trace         print every instruction before it runs (with its hart's
                  number, h0, h1, ..., on a machine of several harts)
  --max-steps N   stop after N steps (exit code 124)
  --ram MiB       RAM size in MiB, 1 to 2048 (default 128)
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
  --fb-dump DIR   write DIR/frame-NNNN.ppm on every FB_PRESENT and at exit";

fn main() -> ExitCode {
    let mut trace = false;
    let mut max_steps = None;
    let mut config = MachineConfig::default();
    let mut path = None;
    let mut fb = false;
    let mut headless = false;
    let mut dump_dir = None;
    // The hub every `--net hub` card plugs into.
    let hub = Hub::new();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--trace" => trace = true,
            "--fb" => fb = true,
            "--headless" => headless = true,
            "--fb-dump" => match args.next() {
                Some(dir) => dump_dir = Some(dir.into()),
                None => {
                    eprintln!("grail: --fb-dump needs a directory");
                    return ExitCode::from(2);
                }
            },
            "--max-steps" => {
                let parse = |s: &str| s.parse().map_err(|_| format!("'{s}' is not a number"));
                match value(&mut args, "--max-steps", parse) {
                    Some(n) => max_steps = Some(n),
                    None => return ExitCode::from(2),
                }
            }
            "--ram" => match value(&mut args, "--ram", grail::parse_ram_mib) {
                Some(size) => config.ram_size = size,
                None => return ExitCode::from(2),
            },
            "--clock" => match value(&mut args, "--clock", grail::parse_clock) {
                Some(hz) => config.clock_hz = Some(hz),
                None => return ExitCode::from(2),
            },
            "--harts" => match value(&mut args, "--harts", grail::parse_harts) {
                Some(n) => config.harts = n,
                None => return ExitCode::from(2),
            },
            "--disk" | "--disk-ro" => match value(&mut args, &a, DiskConfig::parse) {
                Some(disk) => config.disks.push(disk.with_read_only(a == "--disk-ro")),
                None => return ExitCode::from(2),
            },
            "--net" => match value(&mut args, "--net", |s| NetConfig::parse(s, &hub)) {
                Some(net) => config.nets.push(net),
                None => return ExitCode::from(2),
            },
            "--pcap" => match (args.next(), config.nets.last_mut()) {
                (Some(file), Some(net)) if net.pcap.is_none() => net.pcap = Some(file.into()),
                (Some(_), Some(_)) => {
                    eprintln!("grail: --pcap: that card is already captured");
                    return ExitCode::from(2);
                }
                (Some(_), None) => {
                    eprintln!("grail: --pcap needs a --net before it");
                    return ExitCode::from(2);
                }
                (None, _) => {
                    eprintln!("grail: --pcap needs a file");
                    return ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if a.starts_with('-') => {
                eprintln!("grail: unknown option {a}\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => path = Some(a),
        }
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let file = match std::fs::read(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("grail: cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut m = match Machine::new(config) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("grail: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = m.load_elf(&file) {
        eprintln!("grail: {path}: {e}");
        return ExitCode::from(2);
    }
    // The machine's console is this process's terminal. An interactive one is switched
    // to raw mode (each key goes to the UART as typed, unechoed) until Grail exits:
    // before the reader thread starts, so its first read is already a raw one.
    let raw = console::RawMode::enable();
    m.set_uart_input(match &raw {
        Some(raw) => raw.stdin_channel(),
        None => console::stdin_channel(),
    });
    m.set_uart_output(std::io::stdout());
    m.set_debug_output(std::io::stderr());

    if fb && !headless && !display::WINDOW_SUPPORTED {
        eprintln!("grail: no framebuffer window on this platform, running headless");
    }
    let several_harts = m.config().harts > 1;
    let name = std::path::Path::new(&path)
        .file_name()
        .map_or(path.clone(), |n| n.to_string_lossy().into());
    let opts = DisplayOptions {
        window: fb && !headless && display::WINDOW_SUPPORTED,
        dump_dir,
        title: format!("Grail - {name}"),
    };
    let result = display::run(
        &mut m,
        max_steps,
        |cpu| {
            if trace {
                let word = cpu.bus.peek(cpu.pc);
                let text = word.map_or("<unmapped>".to_string(), |w| excalibur::decode(w).to_string());
                if several_harts {
                    eprintln!("h{} {:?} {:08x}: {}", cpu.id, cpu.mode, cpu.pc, text);
                } else {
                    eprintln!("{:?} {:08x}: {}", cpu.mode, cpu.pc, text);
                }
            }
        },
        &opts,
    );
    match result {
        Ok(Outcome::Program(RunResult::Exited(code))) => ExitCode::from(code.min(255) as u8),
        Ok(Outcome::Program(RunResult::StepLimit)) => {
            eprintln!("grail: step limit reached at pc={:#010x}", m.cpu.pc);
            ExitCode::from(124)
        }
        // Closing the window is a normal way to stop the machine.
        Ok(Outcome::WindowClosed) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("grail: {e}");
            ExitCode::from(2)
        }
    }
}

/// The value of option `name`, parsed with `parse`. Prints why it is missing or wrong.
fn value<T>(
    args: &mut impl Iterator<Item = String>,
    name: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Option<T> {
    let Some(v) = args.next() else {
        eprintln!("grail: {name} needs a value\n{USAGE}");
        return None;
    };
    parse(&v).map_err(|e| eprintln!("grail: {name}: {e}")).ok()
}
