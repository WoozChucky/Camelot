//! The listener: one thread per connection, one request per connection.

use crate::api::{error_response, App};
use crate::guard::{Guard, SECURITY_HEADERS};
use crate::http::{read_request, Deadline, Limits, Request, Response};
use crate::random;
use avalon_client::Timeouts;
use avalon_proto::{DEFAULT_AVALON_ADDR, DEFAULT_LAKE_ADDR};
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Where Pendragon listens by default: loopback, next to Avalon (7400) and Lake (7401).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7402";

/// Avalon's RAM budget when nobody says otherwise: Avalon's own default (`avalon
/// --max-ram`), 4096 MiB. The protocol does not report the budget, so the overview
/// measures the RAM in use against this.
pub const DEFAULT_AVALON_MAX_RAM: u64 = 4096 * 1024 * 1024;

/// Most connections served at once; more are answered 503 at once.
pub const MAX_CONNECTIONS: usize = 64;

/// How long writing a response may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How Pendragon runs.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to serve the pages. Loopback by default.
    pub listen: SocketAddr,
    /// Avalon's address, `host:port`.
    pub avalon: String,
    /// Lake's address, `host:port`.
    pub lake: String,
    /// Avalon's RAM budget in bytes, for the overview.
    pub avalon_max_ram: u64,
    /// More names the pages may be reached by (`Host` values; see [`crate::guard`]).
    pub allow_hosts: Vec<String>,
    pub limits: Limits,
    /// Time limits of Pendragon's sessions with Avalon and Lake: how long to try to
    /// connect, and to wait for a reply (a console waits for output without limit).
    pub timeouts: Timeouts,
    /// Say where it listens on stderr.
    pub log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen: DEFAULT_LISTEN.parse().expect("the default address parses"),
            avalon: DEFAULT_AVALON_ADDR.into(),
            lake: DEFAULT_LAKE_ADDR.into(),
            avalon_max_ram: DEFAULT_AVALON_MAX_RAM,
            allow_hosts: Vec::new(),
            limits: Limits::default(),
            timeouts: Timeouts::default(),
            log: true,
        }
    }
}

/// A bound server, ready to [`Server::run`].
pub struct Server {
    listener: TcpListener,
    shared: Arc<Shared>,
}

/// Stops a running server from another thread.
#[derive(Clone)]
pub struct ShutdownHandle {
    shared: Arc<Shared>,
}

/// A server running on a thread of its own ([`Server::spawn`]).
pub struct ServerThread {
    addr: SocketAddr,
    url: String,
    handle: ShutdownHandle,
    thread: JoinHandle<io::Result<()>>,
}

struct Shared {
    guard: Guard,
    app: App,
    limits: Limits,
    addr: SocketAddr,
    log: bool,
    stopping: AtomicBool,
    connections: AtomicUsize,
}

impl Server {
    /// Binds the listening socket (port 0 for an ephemeral port) and makes this
    /// launch's token from the OS random source.
    pub fn bind(config: &Config) -> io::Result<Server> {
        let token = random::token()?;
        let listener = TcpListener::bind(config.listen)?;
        let addr = listener.local_addr()?;
        Ok(Server {
            listener,
            shared: Arc::new(Shared {
                guard: Guard::new(addr, &config.allow_hosts, token),
                app: App::new(
                    &config.avalon,
                    &config.lake,
                    config.avalon_max_ram,
                    config.timeouts,
                ),
                limits: config.limits,
                addr,
                log: config.log,
                stopping: AtomicBool::new(false),
                connections: AtomicUsize::new(0),
            }),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.shared.addr
    }

    /// This launch's token.
    pub fn token(&self) -> &str {
        self.shared.guard.token()
    }

    /// The URL to open: the pages, with the token.
    pub fn url(&self) -> String {
        self.shared.url()
    }

    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Serves until [`ShutdownHandle::shutdown`].
    pub fn run(self) -> io::Result<()> {
        let shared = &self.shared;
        if shared.log {
            eprintln!(
                "pendragon: Avalon at {}, Lake at {}",
                shared.app.avalon_addr(),
                shared.app.lake_addr()
            );
            if !shared.addr.ip().is_loopback() {
                eprintln!(
                    "pendragon: warning: {} is not a loopback address: anyone who can reach it and \
                     knows the token controls the VMs and disks",
                    shared.addr
                );
            }
        }
        for stream in self.listener.incoming() {
            if shared.stopping.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else {
                // Out of sockets or similar: give it a moment rather than spin.
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                shared.connections.fetch_sub(1, Ordering::SeqCst);
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                let busy = with_security_headers(Response::text(503, "too many connections; try again"));
                let _ = busy.write_to(&mut &stream, false);
                finish(&stream);
                continue;
            }
            let shared = Arc::clone(shared);
            let spawned = std::thread::Builder::new()
                .name("pendragon-http".into())
                .spawn(move || {
                    shared.serve(&stream);
                    shared.connections.fetch_sub(1, Ordering::SeqCst);
                });
            if spawned.is_err() {
                self.shared.connections.fetch_sub(1, Ordering::SeqCst);
            }
        }
        shared.app.hub.close_all();
        Ok(())
    }

    /// Runs the server on a new thread.
    pub fn spawn(self) -> io::Result<ServerThread> {
        let addr = self.local_addr();
        let url = self.url();
        let handle = self.shutdown_handle();
        let thread = std::thread::Builder::new()
            .name("pendragon".into())
            .spawn(move || self.run())?;
        Ok(ServerThread {
            addr,
            url,
            handle,
            thread,
        })
    }
}

impl ShutdownHandle {
    /// Asks the server to stop accepting connections. Requests being served finish.
    pub fn shutdown(&self) {
        if self.shared.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        // Wake the accept loop with a connection of our own.
        let _ = TcpStream::connect_timeout(&self.shared.reachable_addr(), Duration::from_secs(1));
    }
}

impl ServerThread {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The URL to open, with the token.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The token, from the URL.
    pub fn token(&self) -> &str {
        self.url.rsplit_once("token=").map_or("", |(_, t)| t)
    }

    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.handle.clone()
    }

    /// Shuts the server down and waits for its accept loop to end.
    pub fn stop(self) -> io::Result<()> {
        self.handle.shutdown();
        self.thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the server thread panicked")))
    }
}

impl Shared {
    /// The listening address, with a loopback address for an unspecified one.
    fn reachable_addr(&self) -> SocketAddr {
        let mut addr = self.addr;
        if addr.ip().is_unspecified() {
            addr.set_ip(match addr {
                SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            });
        }
        addr
    }

    fn url(&self) -> String {
        let addr = self.reachable_addr();
        format!("http://{addr}/?token={}", self.guard.token())
    }

    fn serve(&self, stream: &TcpStream) {
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let request = {
            let mut reader = Deadline::new(stream, self.limits.timeout);
            read_request(&mut reader, &self.limits, |method, path| {
                self.app.body_limit(method, path)
            })
        };
        let (response, head_only) = match request {
            Ok(req) => (self.respond(&req), req.method == "HEAD"),
            Err(e) => match e.status() {
                Some(status) => (Response::text(status, &e.to_string()), false),
                None => return,
            },
        };
        let _ = with_security_headers(response).write_to(&mut &*stream, head_only);
        finish(stream);
    }

    /// The guard's checks, then the page or the API.
    fn respond(&self, req: &Request) -> Response {
        let api = req.segments.first().is_some_and(|s| s == "api");
        let refuse = |why: String| {
            if api {
                error_response(403, &why, None)
            } else {
                Response::text(403, &why)
            }
        };
        if let Err(r) = self.guard.check_host(req) {
            return refuse(r.0);
        }
        if api {
            if let Err(r) = self.guard.check_token(req) {
                return refuse(r.0);
            }
            if !matches!(req.method.as_str(), "GET" | "HEAD") {
                if let Err(r) = self.guard.check_origin(req) {
                    return refuse(r.0);
                }
            }
        }
        self.app.handle(req)
    }
}

fn with_security_headers(mut response: Response) -> Response {
    for (name, value) in SECURITY_HEADERS {
        response = response.with_header(name, value);
    }
    response
}

/// Ends a connection gently: says no more is coming, then reads (and drops) what the
/// client still sends for a moment, so that closing does not reset the connection
/// before the client has read the response (which a reset can discard).
fn finish(stream: &TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut sink = [0u8; 8192];
    let mut drained = 0;
    while drained < 4 * 1024 * 1024 {
        match (&mut &*stream).read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
}
