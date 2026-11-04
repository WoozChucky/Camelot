//! Pendragon: the web console of the Avalon platform. One browser page for the VMs on
//! an Avalon server and the disk images on a Lake server: list, create, start, pause,
//! resume, stop and destroy VMs, watch and type into their consoles, create, delete,
//! attach and detach disks, and see how both servers are doing.
//!
//! Pendragon is a client of both servers, like Round Table, and goes through the same
//! client layer ([`avalon_client`]) for every operation; Avalon and Lake stay headless.
//! Everything else is std only and written here:
//!
//! - [`http`]: a strict HTTP/1.1 request parser and response writer, with limits on
//!   size and time.
//! - [`json`]: the API's JSON, and [`base64`] for bytes inside it.
//! - [`guard`]: the checks that keep other web pages out (`Host`, a per-launch token
//!   from the OS random source in [`random`], `Origin`), and the security headers.
//! - `api.rs`: the routes, pages and API; `console.rs`: live consoles for the page.
//! - [`Server`] (`server.rs`): the listener, a thread per connection.
//! - `ui/`: the page, its script and its style, plain HTML, JavaScript and CSS built
//!   into the binary (`include_str!`), with no build step.
//!
//! `docs/pendragon.md` describes the API and the security model. The `pendragon`
//! binary is [`parse_args`] and [`Server`]; the tests run a server in process.

pub mod base64;
pub mod guard;
pub mod http;
pub mod json;
pub mod random;
pub mod sha1;
pub mod websocket;

mod api;
mod console;
pub mod screen;
mod server;

pub use api::{MAX_BODY, MAX_UPLOAD, MAX_WAIT};
pub use console::{ConsoleLimits, IDLE as CONSOLE_IDLE, KEEP as CONSOLE_KEEP, MAX_TAPS};
pub use guard::{TOKEN_HEADER, TOKEN_PROTOCOL};
pub use screen::{ScreenLimits, MAX_SCREENS};
pub use server::{
    Config, Server, ServerThread, ShutdownHandle, DEFAULT_AVALON_MAX_RAM, DEFAULT_LISTEN, MAX_CONNECTIONS,
};

use avalon_client::parse_addr;
use avalon_proto::{default_port, DEFAULT_AVALON_ADDR, DEFAULT_LAKE_ADDR};

/// The name Pendragon gives in its `Hello`.
pub const AGENT: &str = "pendragon";

/// Pendragon's version, shown on the overview.
pub const VERSION: &str = concat!("pendragon ", env!("CARGO_PKG_VERSION"));

pub const USAGE: &str = "usage: pendragon [--listen ADDR] [--avalon ADDR] [--lake ADDR] [options]

Serves a web console for the Avalon platform: the VMs of an Avalon server and
the disk images of a Lake server, in a browser. Prints the URL to open, which
carries this launch's token.

  --listen ADDR   where to serve the pages: host:port, or a host or IP address
                  for port 7402 (default 127.0.0.1:7402)
  --avalon ADDR   Avalon's address: host:port, or a host alone for port 7400
                  (default 127.0.0.1:7400)
  --lake ADDR     Lake's address: host:port, or a host alone for port 7401
                  (default 127.0.0.1:7401)
  --avalon-max-ram SIZE
                  Avalon's RAM budget (its --max-ram), shown against the RAM
                  in use: MiB, or with M/MiB, G/GiB or T/TiB (default 4096)
  --allow-host NAME
                  another name the pages may be reached by (repeatable), for
                  --listen on an interface other than loopback: camelot.lan,
                  192.168.1.5, camelot:8080. Requests naming any other host
                  are refused, which stops DNS rebinding
  -h, --help      show this text

Only pages with the token can use the API. Ctrl-C stops Pendragon; the VMs and
disks carry on.";

/// Parses the command line (without the program name): a configuration, or `None`
/// for `--help`.
pub fn parse_args(args: &[String]) -> Result<Option<Config>, String> {
    let mut config = Config::default();
    let mut seen: Vec<&str> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        if flag == "-h" || flag == "--help" {
            if inline.is_some() {
                return Err(format!("{flag} takes no value"));
            }
            return Ok(None);
        }
        let flag = match flag {
            "--listen" => "--listen",
            "--avalon" => "--avalon",
            "--lake" => "--lake",
            "--avalon-max-ram" => "--avalon-max-ram",
            "--allow-host" => "--allow-host",
            other if other.starts_with('-') => return Err(format!("unknown option '{other}'")),
            other => {
                return Err(format!(
                    "unexpected argument '{other}' (pendragon takes only options)"
                ))
            }
        };
        let value = match inline {
            Some(value) => value,
            None => iter
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))?,
        };
        if flag != "--allow-host" && seen.contains(&flag) {
            return Err(format!("{flag} is given twice"));
        }
        seen.push(flag);
        let with = |e: String| format!("{flag}: {e}");
        match flag {
            "--listen" => {
                config.listen =
                    avalon_proto::parse_listen(&value, default_port(DEFAULT_LISTEN)).map_err(with)?
            }
            "--avalon" => {
                config.avalon = parse_addr(&value, default_port(DEFAULT_AVALON_ADDR)).map_err(with)?
            }
            "--lake" => config.lake = parse_addr(&value, default_port(DEFAULT_LAKE_ADDR)).map_err(with)?,
            "--avalon-max-ram" => {
                config.avalon_max_ram = avalon_client::parse::parse_budget(&value).map_err(with)?;
            }
            _ => config
                .allow_hosts
                .push(guard::parse_allow_host(&value).map_err(with)?),
        }
    }
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &str) -> Result<Option<Config>, String> {
        let args: Vec<String> = args.split_whitespace().map(str::to_string).collect();
        parse_args(&args)
    }

    #[test]
    fn options() {
        let c = parse("").unwrap().unwrap();
        assert_eq!(c.listen, "127.0.0.1:7402".parse().unwrap());
        assert_eq!(c.avalon, "127.0.0.1:7400");
        assert_eq!(c.lake, "127.0.0.1:7401");
        assert_eq!(c.avalon_max_ram, 4096 << 20);
        assert!(c.allow_hosts.is_empty());
        let c = parse(
            "--listen 0.0.0.0 --avalon 10.0.0.2 --lake=store:9000 --avalon-max-ram 512 \
             --allow-host Camelot.lan --allow-host 10.0.0.5:7402",
        )
        .unwrap()
        .unwrap();
        assert_eq!(c.listen, "0.0.0.0:7402".parse().unwrap());
        assert_eq!(c.avalon, "10.0.0.2:7400");
        assert_eq!(c.lake, "store:9000");
        assert_eq!(c.avalon_max_ram, 512 << 20);
        assert_eq!(c.allow_hosts, ["camelot.lan", "10.0.0.5:7402"]);
        assert!(parse("--help").unwrap().is_none());
        assert!(parse("--lake x -h").unwrap().is_none());
    }

    #[test]
    fn mistakes() {
        for (args, why) in [
            ("--bogus", "unknown option '--bogus'"),
            ("serve", "unexpected argument 'serve'"),
            ("--listen", "--listen needs a value"),
            ("--listen nowhere:x", "--listen: "),
            ("--avalon a:0", "--avalon: "),
            ("--lake a b", "unexpected argument 'b'"),
            ("--lake a --lake b", "--lake is given twice"),
            (
                "--avalon-max-ram 0",
                "--avalon-max-ram: 0 is too small (at least 1 MiB)",
            ),
            ("--avalon-max-ram 1.5", "--avalon-max-ram: '1.5'"),
            ("--avalon-max-ram 99999999999999999", "--avalon-max-ram: "),
            ("--allow-host a/b", "--allow-host: 'a/b' is not a host name"),
            ("--help=1", "--help takes no value"),
        ] {
            match parse(args) {
                Err(e) => assert!(e.starts_with(why), "{args}: {e}"),
                Ok(c) => panic!("{args}: parsed as {c:?}"),
            }
        }
    }

    #[test]
    fn usage_fits_a_terminal() {
        for line in USAGE.lines() {
            assert!(line.chars().count() <= 80, "{line}");
        }
    }
}
