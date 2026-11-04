//! The listener: one thread per connection, one request per connection.

use crate::api::{error_response, App, RELEASE_WAIT};
use crate::console::ConsoleLimits;
use crate::guard::{Guard, Refusal, SECURITY_HEADERS};
use crate::http::{read_request_head, Deadline, Limits, Request, Response};
use crate::random;
use avalon_client::Timeouts;
use avalon_proto::{DEFAULT_AVALON_ADDR, DEFAULT_LAKE_ADDR};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
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

/// Most connections being turned away with a 503 at once; more are closed at once.
const MAX_TURNING_AWAY: usize = 16;

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
    /// How many consoles may be open, how much output each keeps, and when an
    /// unused one is closed.
    pub consoles: ConsoleLimits,
    /// How long detaching a disk waits for Lake to see it released.
    pub release_wait: Duration,
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
            consoles: ConsoleLimits::default(),
            release_wait: RELEASE_WAIT,
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
    /// Connections being answered 503 ([`Shared::turn_away`]).
    turning_away: AtomicUsize,
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
                app: App::new(config),
                limits: config.limits,
                addr,
                log: config.log,
                stopping: AtomicBool::new(false),
                connections: AtomicUsize::new(0),
                turning_away: AtomicUsize::new(0),
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
        // Closes unused consoles on time, whether or not anyone opens another. It
        // ends when the accept loop does (close_all wakes it).
        let sweeper = Arc::clone(shared);
        std::thread::Builder::new()
            .name("pendragon-consoles".into())
            .spawn(move || sweeper.app.hub.sweep_until_closed())?;
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
                shared.turn_away(stream);
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
        let mut reader = Deadline::new(stream, self.limits.timeout);
        // The guard judges the head before the body is read: a request it refuses
        // costs its head, not the up to MAX_UPLOAD bytes it may say it will send.
        let answer = read_request_head(&mut reader, &self.limits, |method, path| {
            self.app.body_limit(method, path)
        })
        .and_then(|head| {
            let head_only = head.request.method == "HEAD";
            match self.refuse(&head.request) {
                Some(refusal) => Ok((refusal, head_only)),
                None => Ok((self.app.handle(&head.read_body(&mut reader)?), head_only)),
            }
        });
        let (response, head_only) = match answer {
            Ok(answer) => answer,
            Err(e) => match e.status() {
                Some(status) => (Response::text(status, &e.to_string()), false),
                None => return,
            },
        };
        let _ = with_security_headers(response).write_to(&mut &*stream, head_only);
        finish(stream);
    }

    /// The guard's checks, on the request's head: a 403, or `None` to go on to the
    /// page or the API.
    fn refuse(&self, req: &Request) -> Option<Response> {
        let api = req.segments.first().is_some_and(|s| s == "api");
        let check = || {
            self.guard.check_host(req)?;
            if api {
                self.guard.check_token(req)?;
                if !matches!(req.method.as_str(), "GET" | "HEAD") {
                    self.guard.check_origin(req)?;
                }
            }
            Ok(())
        };
        check().err().map(|Refusal(why)| {
            if api {
                error_response(403, &why, None)
            } else {
                Response::text(403, &why)
            }
        })
    }

    /// Answers a connection over [`MAX_CONNECTIONS`] with a 503, on a thread of its
    /// own so that a client that dawdles over it cannot hold up the accept loop. A few
    /// at a time: past [`MAX_TURNING_AWAY`], connections are closed without a word.
    fn turn_away(self: &Arc<Self>, stream: TcpStream) {
        if self.turning_away.fetch_add(1, Ordering::SeqCst) >= MAX_TURNING_AWAY {
            self.turning_away.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        let shared = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("pendragon-busy".into())
            .spawn(move || {
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                let busy = with_security_headers(Response::text(503, "too many connections; try again"));
                let _ = busy.write_to(&mut &stream, false);
                finish(&stream);
                shared.turning_away.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.turning_away.fetch_sub(1, Ordering::SeqCst);
        }
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
/// before the client has read the response (which a reset can discard). The moment is
/// [`avalon_proto::LINGER`] all told, however the client spreads its bytes, and at most
/// [`avalon_proto::LINGER_BYTES`]: a client that keeps sending cannot keep the
/// connection. Avalon and Lake close their connections the same way.
fn finish(stream: &TcpStream) {
    avalon_proto::close_gracefully(stream, || false);
}
