//! The pages and the JSON API behind them (`docs/pendragon.md` lists every route).
//!
//! Each API request is one or two Avalon protocol sessions through `avalon-client`,
//! the layer Round Table uses too: the same operations, the same parsers for what
//! people type (`--ram`, `--clock`, disk sources and sizes) and the same formatting
//! for what they read. Errors carry the protocol's error code and the server's
//! message, with an HTTP status chosen from the code.

use crate::base64;
use crate::console::Hub;
use crate::http::{Request, Response};
use crate::json::Json;
use avalon_client::{
    format_hz, format_size, holds_disks, lake_disks, parse_clock, parse_disk_size, parse_disk_source,
    parse_ram, Client, Timeouts, VmAction,
};
use avalon_proto::{
    validate_name, DeviceSpec, DiskInfo, DiskSpec, ErrorCode, HardwareProfile, VmDetails, VmRef, VmState,
    VmSummary, SECTOR_SIZE,
};
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Most body bytes of an API request, but for creating a VM.
pub const MAX_BODY: usize = 64 * 1024;

/// Most body bytes of a request to create a VM: its program, in base64, fits in one
/// protocol frame (16 MiB) with room to spare.
pub const MAX_UPLOAD: usize = 24 * 1024 * 1024;

/// Longest a console read waits for output.
pub const MAX_WAIT: Duration = Duration::from_secs(20);

/// How long a detach waits for Lake to see the disk released.
const RELEASE_WAIT: Duration = Duration::from_secs(5);

/// Time limits for the health checks of the overview and the attachment column of the
/// disk list, which should not hold the page up for long when a server is away.
const QUICK: Timeouts = Timeouts {
    connect: Duration::from_secs(3),
    reply: Duration::from_secs(10),
};

const INDEX_HTML: &str = include_str!("ui/index.html");
const APP_JS: &str = include_str!("ui/app.js");
const STYLE_CSS: &str = include_str!("ui/style.css");

/// What the API serves, and where it finds the servers.
pub struct App {
    avalon: String,
    lake: String,
    /// Time limits of every session with the servers.
    timeouts: Timeouts,
    /// Avalon's RAM budget, for the overview (the protocol does not report it).
    max_ram: u64,
    pub(crate) hub: Hub,
}

/// A failed request: the HTTP status, the message for the page, and the protocol's
/// error code when a server refused.
#[derive(Debug)]
struct Fail {
    status: u16,
    message: String,
    code: Option<ErrorCode>,
}

impl Fail {
    fn new(status: u16, message: impl Into<String>) -> Fail {
        Fail {
            status,
            message: message.into(),
            code: None,
        }
    }

    fn bad(message: impl Into<String>) -> Fail {
        Fail::new(400, message)
    }

    fn response(&self) -> Response {
        let code = self.code.map(|c| c.to_string());
        error_response(self.status, &self.message, code.as_deref())
    }
}

/// A JSON error body: `{"error": message, "code": protocol error or null}`.
pub fn error_response(status: u16, message: &str, code: Option<&str>) -> Response {
    json(
        status,
        Json::object([
            ("error", message.into()),
            ("code", code.map(str::to_string).into()),
        ]),
    )
}

/// What a server's refusal (or absence) is in HTTP.
fn status_of(code: Option<ErrorCode>) -> u16 {
    match code {
        Some(ErrorCode::NoSuchVm | ErrorCode::NoSuchDisk) => 404,
        Some(
            ErrorCode::AlreadyExists | ErrorCode::Busy | ErrorCode::InvalidState | ErrorCode::OverBudget,
        ) => 409,
        Some(ErrorCode::InvalidName | ErrorCode::InvalidProfile | ErrorCode::InvalidProgram) => 422,
        // The server failed, misunderstood, or could not be reached.
        _ => 502,
    }
}

/// `what` names the server for an unreachable one ("Avalon", "Lake").
fn client_fail(what: &str, e: avalon_client::Error) -> Fail {
    let code = e.code();
    let message = match &e {
        avalon_client::Error::Connect { addr, cause } if code.is_none() => {
            format!("cannot connect to {what} at {addr}: {cause}")
        }
        _ => e.to_string(),
    };
    Fail {
        status: status_of(code),
        message,
        code,
    }
}

fn json(status: u16, value: Json) -> Response {
    Response::new(status, "application/json", value.encode())
}

type Reply = Result<Response, Fail>;

impl App {
    pub fn new(avalon: &str, lake: &str, max_ram: u64, timeouts: Timeouts) -> App {
        App {
            avalon: avalon.to_string(),
            lake: lake.to_string(),
            timeouts,
            max_ram,
            hub: Hub::new(avalon, timeouts),
        }
    }

    pub fn avalon_addr(&self) -> &str {
        &self.avalon
    }

    pub fn lake_addr(&self) -> &str {
        &self.lake
    }

    /// The most body bytes a request may have.
    pub fn body_limit(&self, method: &str, path: &str) -> usize {
        if method == "POST" && path == "/api/vms" {
            MAX_UPLOAD
        } else {
            MAX_BODY
        }
    }

    /// Answers a request that passed the [`crate::guard`] checks.
    pub fn handle(&self, req: &Request) -> Response {
        let segments: Vec<&str> = req.segments.iter().map(String::as_str).collect();
        // HEAD is GET without the body (the server leaves it out).
        let method = if req.method == "HEAD" {
            "GET"
        } else {
            req.method.as_str()
        };
        match segments.as_slice() {
            ["api", rest @ ..] => self.api(method, rest, req).unwrap_or_else(|f| f.response()),
            page => {
                let (content_type, body) = match page {
                    [""] => ("text/html; charset=utf-8", INDEX_HTML),
                    ["app.js"] => ("text/javascript; charset=utf-8", APP_JS),
                    ["style.css"] => ("text/css; charset=utf-8", STYLE_CSS),
                    _ => return Response::text(404, "no such page"),
                };
                if method != "GET" {
                    return Response::text(405, "only GET").with_header("Allow", "GET, HEAD");
                }
                Response::new(200, content_type, body)
            }
        }
    }

    fn api(&self, method: &str, path: &[&str], req: &Request) -> Reply {
        let allow = |methods: &str| Err(Fail::new(405, format!("{} takes {methods}", req.path)));
        match (path, method) {
            (["overview"], "GET") => Ok(self.overview()),
            (["overview"], _) => allow("GET"),
            (["vms"], "GET") => self.list_vms(),
            (["vms"], "POST") => self.create_vm(req),
            (["vms"], _) => allow("GET, POST"),
            (["vms", vm], "GET") => self.vm_info(vm_ref(vm)?),
            (["vms", _], _) => allow("GET"),
            (["vms", vm, "console"], "GET") => self.console_read(vm_ref(vm)?, req),
            (["vms", vm, "console"], "POST") => self.console_write(vm_ref(vm)?, req),
            (["vms", _, "console"], _) => allow("GET, POST"),
            (["vms", vm, action], "POST") => match VmAction::parse(action) {
                Some(action) => self.vm_action(action, vm_ref(vm)?),
                None => Err(Fail::new(
                    404,
                    format!("no action '{action}' (start, pause, resume, stop, destroy)"),
                )),
            },
            (["vms", _, _], _) => allow("POST"),
            (["disks"], "GET") => self.list_disks(),
            (["disks"], "POST") => self.create_disk(req),
            (["disks"], _) => allow("GET, POST"),
            (["disks", name], "DELETE") => self.delete_disk(name),
            (["disks", _], _) => allow("DELETE"),
            (["disks", name, "detach"], "POST") => self.detach_disk(name),
            (["disks", _, "detach"], _) => allow("POST"),
            _ => Err(Fail::new(404, format!("no such API: {}", req.path))),
        }
    }

    /// [`QUICK`], or shorter where the configured limits are.
    fn quick(&self) -> Timeouts {
        Timeouts {
            connect: QUICK.connect.min(self.timeouts.connect),
            reply: QUICK.reply.min(self.timeouts.reply),
        }
    }

    fn avalon(&self) -> Result<Client, Fail> {
        Client::connect_with(&self.avalon, crate::AGENT, self.timeouts).map_err(|e| client_fail("Avalon", e))
    }

    fn lake(&self) -> Result<Client, Fail> {
        Client::connect_with(&self.lake, crate::AGENT, self.timeouts).map_err(|e| client_fail("Lake", e))
    }

    // ---- Overview ----

    fn overview(&self) -> Response {
        let server = |what: &str, addr: &str, result: &Result<String, Fail>| {
            Json::object([
                ("name", what.into()),
                ("addr", addr.into()),
                ("ok", result.is_ok().into()),
                ("server", result.as_ref().ok().cloned().into()),
                ("error", result.as_ref().err().map(|f| f.message.clone()).into()),
            ])
        };
        let mut vms = None;
        let avalon = Client::connect_with(&self.avalon, crate::AGENT, self.quick())
            .and_then(|mut c| {
                vms = Some(c.list_vms()?);
                Ok(c.server().to_string())
            })
            .map_err(|e| client_fail("Avalon", e));
        let mut disks = None;
        let lake = Client::connect_with(&self.lake, crate::AGENT, self.quick())
            .and_then(|mut c| {
                disks = Some(c.list_disks()?);
                Ok(c.server().to_string())
            })
            .map_err(|e| client_fail("Lake", e));

        let vm_stats = vms.as_ref().map(|vms| {
            let count = |f: fn(VmState) -> bool| vms.iter().filter(|v| f(v.state)).count();
            Json::object([
                ("total", vms.len().into()),
                ("created", count(|s| s == VmState::Created).into()),
                ("running", count(|s| s == VmState::Running).into()),
                ("paused", count(|s| s == VmState::Paused).into()),
                (
                    "ended",
                    count(|s| matches!(s, VmState::Stopped | VmState::Exited(_))).into(),
                ),
            ])
        });
        // Avalon holds a VM's RAM from its creation to its destruction.
        let ram = vms.as_ref().map(|vms| {
            let used: u64 = vms.iter().map(|v| v.profile.ram_size).sum();
            Json::object([
                ("used", used.into()),
                ("budget", self.max_ram.into()),
                ("used_text", format_size(used).into()),
                ("budget_text", format_size(self.max_ram).into()),
            ])
        });
        let disk_stats = disks.as_ref().map(|disks| {
            let bytes: u64 = disks
                .iter()
                .map(|d| d.sectors.saturating_mul(SECTOR_SIZE as u64))
                .sum();
            Json::object([
                ("total", disks.len().into()),
                (
                    "attached",
                    disks.iter().filter(|d| d.writer || d.readers > 0).count().into(),
                ),
                ("bytes", bytes.into()),
                ("bytes_text", format_size(bytes).into()),
            ])
        });
        json(
            200,
            Json::object([
                ("pendragon", crate::VERSION.into()),
                ("avalon", server("Avalon", &self.avalon, &avalon)),
                ("lake", server("Lake", &self.lake, &lake)),
                ("vms", vm_stats.into()),
                ("ram", ram.into()),
                ("disks", disk_stats.into()),
            ]),
        )
    }

    // ---- VMs ----

    fn list_vms(&self) -> Reply {
        let vms = self.avalon()?.list_vms().map_err(|e| client_fail("Avalon", e))?;
        let vms: Vec<Json> = vms.iter().map(vm_json).collect();
        Ok(json(
            200,
            Json::object([("avalon", self.avalon.as_str().into()), ("vms", Json::Array(vms))]),
        ))
    }

    fn vm_info(&self, vm: VmRef) -> Reply {
        let details = self
            .avalon()?
            .vm_info(&vm)
            .map_err(|e| client_fail("Avalon", e))?;
        Ok(json(200, vm_details_json(&details)))
    }

    fn vm_action(&self, action: VmAction, vm: VmRef) -> Reply {
        self.avalon()?
            .vm_action(action, &vm)
            .map_err(|e| client_fail("Avalon", e))?;
        Ok(json(
            200,
            Json::object([("done", action.done().into()), ("vm", vm.to_string().into())]),
        ))
    }

    fn create_vm(&self, req: &Request) -> Reply {
        let create = parse_create(&body_json(req)?)?;
        let mut avalon = self.avalon()?;
        let id = avalon
            .create_vm(&create.name, create.profile, create.program)
            .map_err(|e| client_fail("Avalon", e))?;
        if create.start {
            if let Err(e) = avalon.start_vm(&VmRef::Id(id)) {
                // Say what did happen before the error, as Round Table does.
                let mut fail = client_fail("Avalon", e);
                fail.message = format!(
                    "VM {id} ({}) was created, but did not start: {}",
                    create.name, fail.message
                );
                return Err(fail);
            }
        }
        Ok(json(
            201,
            Json::object([
                ("id", id.into()),
                ("name", create.name.into()),
                ("started", create.start.into()),
            ]),
        ))
    }

    fn console_read(&self, vm: VmRef, req: &Request) -> Reply {
        let number = |name: &str| -> Result<Option<u64>, Fail> {
            req.query(name)
                .map(|v| {
                    v.parse::<u64>()
                        .ok()
                        .filter(|_| v.bytes().all(|b| b.is_ascii_digit()))
                        .ok_or_else(|| Fail::bad(format!("{name}: '{v}' is not a whole number")))
                })
                .transpose()
        };
        let from = number("from")?.unwrap_or(0);
        let tap_id = number("tap")?;
        let wait = Duration::from_millis(number("wait")?.unwrap_or(0)).min(MAX_WAIT);
        let tap = self.hub.tap(&vm).map_err(|e| client_fail("Avalon", e))?;
        let chunk = tap.read(tap_id, from, wait);
        Ok(json(
            200,
            Json::object([
                ("vm", tap.vm.into()),
                ("tap", chunk.tap.into()),
                ("from", chunk.from.into()),
                ("next", chunk.next.into()),
                ("data", base64::encode(&chunk.data).into()),
                ("reset", chunk.reset.into()),
                ("ended", chunk.ended.into()),
            ]),
        ))
    }

    fn console_write(&self, vm: VmRef, req: &Request) -> Reply {
        let body = body_json(req)?;
        only_keys(&body, &["data"])?;
        let data = body
            .get("data")
            .and_then(Json::as_str)
            .ok_or_else(|| Fail::bad("data: the text to send is missing"))?;
        let tap = self.hub.tap(&vm).map_err(|e| client_fail("Avalon", e))?;
        tap.input(data.as_bytes()).map_err(|e| Fail::new(409, e))?;
        Ok(json(200, Json::object([("sent", data.len().into())])))
    }

    // ---- Disks ----

    fn list_disks(&self) -> Reply {
        let disks = self.lake()?.list_disks().map_err(|e| client_fail("Lake", e))?;
        // Which VMs hold each image comes from Avalon; the list stands without it.
        let vms = Client::connect_with(&self.avalon, crate::AGENT, self.quick())
            .and_then(|mut c| c.list_vms())
            .map_err(|e| client_fail("Avalon", e));
        let mut same = SameServer::new(&self.lake);
        let disks: Vec<Json> = disks
            .iter()
            .map(|disk| {
                let holders = vms.as_ref().ok().map(|vms| {
                    holders(vms, &disk.name, &mut same)
                        .into_iter()
                        .map(|(vm, read_only)| {
                            Json::object([
                                ("id", vm.id.into()),
                                ("name", vm.name.as_str().into()),
                                ("read_only", read_only.into()),
                            ])
                        })
                        .collect::<Vec<_>>()
                });
                disk_json(disk, holders.map(Json::Array))
            })
            .collect();
        Ok(json(
            200,
            Json::object([
                ("lake", self.lake.as_str().into()),
                ("disks", Json::Array(disks)),
                ("vms_error", vms.err().map(|f| f.message).into()),
            ]),
        ))
    }

    fn create_disk(&self, req: &Request) -> Reply {
        let body = body_json(req)?;
        only_keys(&body, &["name", "size"])?;
        let name = string(&body, "name")?.ok_or_else(|| Fail::bad("name: a disk needs a name"))?;
        validate_name(name).map_err(|e| Fail::bad(format!("name: {e}")))?;
        let size = string(&body, "size")?.ok_or_else(|| Fail::bad("size: a disk needs a size"))?;
        let sectors = parse_disk_size(size).map_err(|e| Fail::bad(format!("size: {e}")))?;
        self.lake()?
            .create_disk(name, sectors)
            .map_err(|e| client_fail("Lake", e))?;
        Ok(json(
            201,
            Json::object([
                ("name", name.into()),
                ("sectors", sectors.into()),
                ("size_text", format_size(sectors * SECTOR_SIZE as u64).into()),
            ]),
        ))
    }

    fn delete_disk(&self, name: &str) -> Reply {
        self.lake()?
            .delete_disk(name)
            .map_err(|e| client_fail("Lake", e))?;
        Ok(json(200, Json::object([("deleted", name.into())])))
    }

    /// Detaches an image from the VMs on our Avalon that hold it. A VM's disks are
    /// part of its hardware and cannot be unplugged while it runs (protocol doc,
    /// section 6.6): a VM lets go of them when it stops, so detaching stops it.
    fn detach_disk(&self, name: &str) -> Reply {
        validate_name(name).map_err(|e| Fail::new(404, e))?;
        let mut avalon = self.avalon()?;
        let vms = avalon.list_vms().map_err(|e| client_fail("Avalon", e))?;
        let mut same = SameServer::new(&self.lake);
        let holders = holders(&vms, name, &mut same);
        if holders.is_empty() {
            return Err(Fail::new(
                409,
                format!("no VM on Avalon at {} holds disk '{name}'", self.avalon),
            ));
        }
        let mut stopped = Vec::new();
        for (vm, _) in holders {
            match avalon.stop_vm(&VmRef::Id(vm.id)) {
                Ok(()) => {}
                // It stopped, exited or went away meanwhile: it let go all the same.
                Err(e) if matches!(e.code(), Some(ErrorCode::InvalidState | ErrorCode::NoSuchVm)) => {}
                Err(e) => return Err(client_fail("Avalon", e)),
            }
            stopped.push(Json::object([
                ("id", vm.id.into()),
                ("name", vm.name.as_str().into()),
            ]));
        }
        // Avalon releases a stopped VM's disks just after the state change: wait for
        // Lake to see it, so the page shows the disk free (or says it is not yet).
        let released = self.wait_released(name);
        Ok(json(
            200,
            Json::object([("stopped", Json::Array(stopped)), ("released", released.into())]),
        ))
    }

    fn wait_released(&self, name: &str) -> bool {
        let start = Instant::now();
        let Ok(mut lake) = self.lake() else { return false };
        loop {
            match lake.list_disks() {
                Ok(disks) => match disks.iter().find(|d| d.name == name) {
                    Some(d) if d.writer || d.readers > 0 => {}
                    _ => return true,
                },
                Err(_) => return false,
            }
            if start.elapsed() >= RELEASE_WAIT {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A VM named in a path: all digits is an id, anything else a name.
fn vm_ref(segment: &str) -> Result<VmRef, Fail> {
    if segment.is_empty() {
        return Err(Fail::new(404, "no VM named"));
    }
    match VmRef::parse(segment) {
        VmRef::Name(_) if segment.bytes().all(|b| b.is_ascii_digit()) => Err(Fail {
            status: 404,
            message: format!("no VM with id {segment}"),
            code: Some(ErrorCode::NoSuchVm),
        }),
        vm => Ok(vm),
    }
}

/// The body as a JSON object.
fn body_json(req: &Request) -> Result<Json, Fail> {
    let text = std::str::from_utf8(&req.body).map_err(|_| Fail::bad("the body is not UTF-8"))?;
    match Json::parse(text).map_err(Fail::bad)? {
        body @ Json::Object(_) => Ok(body),
        _ => Err(Fail::bad("the body is not a JSON object")),
    }
}

/// Refuses members other than `keys`, so a misspelt field is an error rather than a
/// silent default.
fn only_keys(body: &Json, keys: &[&str]) -> Result<(), Fail> {
    if let Json::Object(members) = body {
        if let Some((key, _)) = members.iter().find(|(k, _)| !keys.contains(&k.as_str())) {
            return Err(Fail::bad(format!(
                "unknown field '{key}' (fields: {})",
                keys.join(", ")
            )));
        }
    }
    Ok(())
}

/// A string member; absent and `null` are `None`.
fn string<'a>(body: &'a Json, key: &str) -> Result<Option<&'a str>, Fail> {
    match body.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) => Ok(Some(s)),
        Some(_) => Err(Fail::bad(format!("{key}: not a string"))),
    }
}

/// A boolean member; absent and `null` are `false`.
fn flag(body: &Json, key: &str) -> Result<bool, Fail> {
    match body.get(key) {
        None | Some(Json::Null) => Ok(false),
        Some(Json::Bool(b)) => Ok(*b),
        Some(_) => Err(Fail::bad(format!("{key}: not true or false"))),
    }
}

/// A VM to create, from the page's form.
#[derive(Debug)]
struct Create {
    name: String,
    profile: HardwareProfile,
    program: Vec<u8>,
    start: bool,
}

/// `{"name", "ram", "clock", "disks": [{"source", "read_only"}], "program", "start"}`:
/// `ram` and `clock` as Round Table's `--ram` and `--clock` take them (a number of MiB,
/// a frequency with Hz/kHz/MHz; an empty clock is full speed), each disk source a path
/// on Avalon's host or `lake://ADDR/NAME`, the program an ELF file in base64.
fn parse_create(body: &Json) -> Result<Create, Fail> {
    only_keys(body, &["name", "ram", "clock", "disks", "program", "start"])?;
    let name = string(body, "name")?.ok_or_else(|| Fail::bad("name: a VM needs a name"))?;
    validate_name(name).map_err(|e| Fail::bad(format!("name: {e}")))?;
    let mut profile = HardwareProfile::default();
    if let Some(ram) = string(body, "ram")?.filter(|s| !s.trim().is_empty()) {
        profile.ram_size = parse_ram(ram.trim()).map_err(|e| Fail::bad(format!("ram: {e}")))?;
    }
    if let Some(clock) = string(body, "clock")?.filter(|s| !s.trim().is_empty()) {
        profile.clock_hz = Some(parse_clock(clock.trim()).map_err(|e| Fail::bad(format!("clock: {e}")))?);
    }
    match body.get("disks") {
        None | Some(Json::Null) => {}
        Some(Json::Array(disks)) => {
            for (i, disk) in disks.iter().enumerate() {
                let at = |e: String| Fail::bad(format!("disk {}: {e}", i + 1));
                if !matches!(disk, Json::Object(_)) {
                    return Err(at("not an object".into()));
                }
                only_keys(disk, &["source", "read_only"]).map_err(|f| at(f.message))?;
                let source = string(disk, "source")
                    .map_err(|f| at(f.message))?
                    .ok_or_else(|| at("source: missing".into()))?;
                profile.devices.push(DeviceSpec::Disk(DiskSpec {
                    source: parse_disk_source(source.trim()).map_err(at)?,
                    read_only: flag(disk, "read_only").map_err(|f| at(f.message))?,
                }));
            }
        }
        Some(_) => return Err(Fail::bad("disks: not a list")),
    }
    let program = string(body, "program")?.ok_or_else(|| Fail::bad("program: choose an ELF file to run"))?;
    let program = base64::decode(program).map_err(|e| Fail::bad(format!("program: {e}")))?;
    if program.is_empty() {
        return Err(Fail::bad("program: the file is empty"));
    }
    Ok(Create {
        name: name.to_string(),
        profile,
        program,
        start: flag(body, "start")?,
    })
}

/// The one-word state, for the page to style and decide by.
fn state_word(state: VmState) -> &'static str {
    match state {
        VmState::Created => "created",
        VmState::Running => "running",
        VmState::Paused => "paused",
        VmState::Stopped => "stopped",
        VmState::Exited(_) => "exited",
    }
}

fn vm_json(vm: &VmSummary) -> Json {
    let profile = &vm.profile;
    let held = holds_disks(vm.state);
    let disks: Vec<Json> = profile
        .devices
        .iter()
        .map(|DeviceSpec::Disk(disk)| {
            let lake = match &disk.source {
                avalon_proto::DiskSource::Lake { addr, name } => {
                    Json::object([("addr", addr.as_str().into()), ("name", name.as_str().into())])
                }
                avalon_proto::DiskSource::File(_) => Json::Null,
            };
            Json::object([
                ("source", disk.source.to_string().into()),
                ("read_only", disk.read_only.into()),
                ("lake", lake),
                ("held", held.into()),
            ])
        })
        .collect();
    let actions: Vec<&str> = VmAction::ALL
        .into_iter()
        .filter(|a| a.allowed_in(vm.state))
        .map(VmAction::name)
        .collect();
    Json::object([
        ("id", vm.id.into()),
        ("name", vm.name.as_str().into()),
        ("state", state_word(vm.state).into()),
        ("state_text", vm.state.to_string().into()),
        (
            "exit_code",
            match vm.state {
                VmState::Exited(code) => Some(code),
                _ => None,
            }
            .into(),
        ),
        ("ram", profile.ram_size.into()),
        ("ram_text", format_size(profile.ram_size).into()),
        ("clock_hz", profile.clock_hz.into()),
        (
            "clock_text",
            match profile.clock_hz {
                Some(hz) => format_hz(hz),
                None => "full speed".into(),
            }
            .into(),
        ),
        ("nominal_hz", profile.nominal_hz.into()),
        ("nominal_text", format_hz(profile.nominal_hz).into()),
        ("disks", Json::Array(disks)),
        ("actions", actions.into()),
    ])
}

fn vm_details_json(details: &VmDetails) -> Json {
    let Json::Object(mut members) = vm_json(&details.vm) else {
        unreachable!("vm_json makes an object")
    };
    let devices: Vec<Json> = details
        .devices
        .iter()
        .map(|d| {
            let name =
                avalon_client::device_name(d.kind).map_or_else(|| format!("type {}", d.kind), str::to_string);
            Json::object([
                ("name", name.into()),
                ("kind", d.kind.into()),
                ("base", format!("{:#010x}", d.base).into()),
                ("size", format!("{:#010x}", d.size).into()),
                ("irq", d.irq.into()),
            ])
        })
        .collect();
    members.push(("steps".into(), details.steps.into()));
    members.push(("devices".into(), Json::Array(devices)));
    Json::Object(members)
}

fn disk_json(disk: &DiskInfo, attached_to: Option<Json>) -> Json {
    let bytes = disk.sectors.saturating_mul(SECTOR_SIZE as u64);
    Json::object([
        ("name", disk.name.as_str().into()),
        ("sectors", disk.sectors.into()),
        ("bytes", bytes.into()),
        ("size_text", format_size(bytes).into()),
        ("readers", disk.readers.into()),
        ("writer", disk.writer.into()),
        ("attached", (disk.writer || disk.readers > 0).into()),
        ("attached_to", attached_to.into()),
    ])
}

/// The VMs whose machines hold Lake image `name` right now (with whether read-only):
/// those still created, running or paused whose profile names it on our Lake.
fn holders<'a>(vms: &'a [VmSummary], name: &str, same: &mut SameServer) -> Vec<(&'a VmSummary, bool)> {
    let mut found = Vec::new();
    for vm in vms.iter().filter(|vm| holds_disks(vm.state)) {
        for (addr, disk, read_only) in lake_disks(&vm.profile) {
            if disk == name && same.is_lake(addr) {
                found.push((vm, read_only));
            }
        }
    }
    found
}

/// Whether an address in a VM's profile is our Lake server: the same text, or a name
/// that reaches the same socket address (`localhost:7401` and `127.0.0.1:7401`).
/// Lookups are remembered for one request.
struct SameServer {
    lake: String,
    resolved: HashMap<String, Vec<SocketAddr>>,
}

impl SameServer {
    fn new(lake: &str) -> SameServer {
        SameServer {
            lake: lake.to_string(),
            resolved: HashMap::new(),
        }
    }

    fn lookup(&mut self, addr: &str) -> Vec<SocketAddr> {
        self.resolved
            .entry(addr.to_string())
            .or_insert_with(|| addr.to_socket_addrs().map(Iterator::collect).unwrap_or_default())
            .clone()
    }

    fn is_lake(&mut self, addr: &str) -> bool {
        if addr.eq_ignore_ascii_case(&self.lake) {
            return true;
        }
        let ours = self.lookup(&self.lake.clone());
        let theirs = self.lookup(addr);
        theirs.iter().any(|a| ours.contains(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(text: &str) -> Result<Create, String> {
        parse_create(&Json::parse(text).unwrap()).map_err(|f| f.message)
    }

    #[test]
    fn create_requests() {
        let c = create(
            r#"{"name": "web", "ram": "64", "clock": "1.5MHz", "start": true, "program": "AQID",
                "disks": [{"source": "lake://127.0.0.1:7401/data"}, {"source": "boot.img", "read_only": true}]}"#,
        )
        .unwrap();
        assert_eq!(c.name, "web");
        assert_eq!(c.profile.ram_size, 64 << 20);
        assert_eq!(c.profile.clock_hz, Some(1_500_000));
        assert_eq!(c.program, [1, 2, 3]);
        assert!(c.start);
        assert_eq!(c.profile.devices.len(), 2);
        assert_eq!(
            c.profile.devices[1],
            DeviceSpec::Disk(DiskSpec {
                source: avalon_proto::DiskSource::File("boot.img".into()),
                read_only: true
            })
        );
        // Defaults: Grail's machine, not started.
        let c = create(r#"{"name": "a", "program": "AA==", "ram": "", "clock": " ", "disks": []}"#).unwrap();
        assert_eq!(c.profile, HardwareProfile::default());
        assert!(!c.start);

        for (text, why) in [
            (r#"{"program": "AA=="}"#, "name: a VM needs a name"),
            (
                r#"{"name": "Web", "program": "AA=="}"#,
                "name: 'Web' is not a valid name",
            ),
            (r#"{"name": 7, "program": "AA=="}"#, "name: not a string"),
            (r#"{"name": "a"}"#, "program: choose an ELF file"),
            (r#"{"name": "a", "program": ""}"#, "program: the file is empty"),
            (r#"{"name": "a", "program": "A"}"#, "program: not base64"),
            (
                r#"{"name": "a", "program": "AA==", "ram": "0"}"#,
                "ram: 0 MiB is out of range",
            ),
            (
                r#"{"name": "a", "program": "AA==", "ram": 64}"#,
                "ram: not a string",
            ),
            (
                r#"{"name": "a", "program": "AA==", "clock": "fast"}"#,
                "clock: 'fast' is not a frequency",
            ),
            (
                r#"{"name": "a", "program": "AA==", "disks": "x"}"#,
                "disks: not a list",
            ),
            (
                r#"{"name": "a", "program": "AA==", "disks": [1]}"#,
                "disk 1: not an object",
            ),
            (
                r#"{"name": "a", "program": "AA==", "disks": [{}]}"#,
                "disk 1: source: missing",
            ),
            (
                r#"{"name": "a", "program": "AA==", "disks": [{"source": "lake://x/"}]}"#,
                "disk 1: 'lake://x/'",
            ),
            (
                r#"{"name": "a", "program": "AA==", "disks": [{"source": "a"}, {"source": "b", "ro": true}]}"#,
                "disk 2: unknown field 'ro'",
            ),
            (
                r#"{"name": "a", "program": "AA==", "disks": [{"source": "a", "read_only": "yes"}]}"#,
                "disk 1: read_only: not true or false",
            ),
            (
                r#"{"name": "a", "program": "AA==", "start": 1}"#,
                "start: not true or false",
            ),
            (
                r#"{"name": "a", "program": "AA==", "rma": "64"}"#,
                "unknown field 'rma'",
            ),
        ] {
            match create(text) {
                Err(e) => assert!(e.starts_with(why), "{text}: {e}"),
                Ok(c) => panic!("{text}: {c:?}"),
            }
        }
    }

    #[test]
    fn statuses() {
        assert_eq!(status_of(Some(ErrorCode::NoSuchVm)), 404);
        assert_eq!(status_of(Some(ErrorCode::NoSuchDisk)), 404);
        assert_eq!(status_of(Some(ErrorCode::Busy)), 409);
        assert_eq!(status_of(Some(ErrorCode::InvalidState)), 409);
        assert_eq!(status_of(Some(ErrorCode::OverBudget)), 409);
        assert_eq!(status_of(Some(ErrorCode::AlreadyExists)), 409);
        assert_eq!(status_of(Some(ErrorCode::InvalidProfile)), 422);
        assert_eq!(status_of(Some(ErrorCode::Internal)), 502);
        assert_eq!(status_of(None), 502);
    }

    #[test]
    fn vm_refs() {
        assert_eq!(vm_ref("7").unwrap(), VmRef::Id(7));
        assert_eq!(vm_ref("web").unwrap(), VmRef::Name("web".into()));
        assert_eq!(vm_ref("99999999999").unwrap_err().status, 404);
        assert_eq!(vm_ref("").unwrap_err().status, 404);
    }

    #[test]
    fn same_server() {
        let mut same = SameServer::new("127.0.0.1:7401");
        assert!(same.is_lake("127.0.0.1:7401"));
        assert!(same.is_lake("localhost:7401"));
        assert!(!same.is_lake("127.0.0.1:7402"));
        assert!(!same.is_lake("no-such-host.invalid:7401"));
    }

    #[test]
    fn vm_output() {
        let vm = VmSummary {
            id: 3,
            name: "web".into(),
            state: VmState::Exited(2),
            profile: HardwareProfile {
                clock_hz: Some(1_500_000),
                devices: vec![DeviceSpec::Disk(DiskSpec {
                    source: parse_disk_source("lake://127.0.0.1:7401/data").unwrap(),
                    read_only: true,
                })],
                ..HardwareProfile::with_ram(64 << 20)
            },
        };
        assert_eq!(
            vm_json(&vm).encode(),
            r#"{"id":3,"name":"web","state":"exited","state_text":"exited(2)","exit_code":2,"ram":67108864,"ram_text":"64 MiB","clock_hz":1500000,"clock_text":"1.5 MHz","nominal_hz":10000000,"nominal_text":"10 MHz","disks":[{"source":"lake://127.0.0.1:7401/data","read_only":true,"lake":{"addr":"127.0.0.1:7401","name":"data"},"held":false}],"actions":["destroy"]}"#
        );
    }
}
