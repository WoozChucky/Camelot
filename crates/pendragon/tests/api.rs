//! Every VM and disk operation of the pages, over HTTP, against Avalon and Lake in
//! process: what the browser does, without a browser.

mod common;

use common::*;
use pendragon::json::Json;

/// A VM's JSON in the list, by name.
fn vm_in(list: &Json, name: &str) -> Json {
    list_of(list, "vms")
        .into_iter()
        .find(|vm| s(vm, "name") == name)
        .unwrap_or_else(|| panic!("no VM {name} in {list}"))
}

#[test]
fn a_quiet_console_stays_attached() {
    // A console waits for output however long the guest is silent: the time limit of
    // Pendragon's sessions applies to replies, not to a console's output.
    let mut platform = Platform::start("quiet", 64);
    let short = pendragon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        avalon: platform.avalon.addr().to_string(),
        lake: platform.lake_addr(),
        timeouts: avalon_client::Timeouts {
            connect: std::time::Duration::from_secs(5),
            reply: std::time::Duration::from_millis(200),
        },
        log: false,
        ..pendragon::Config::default()
    };
    let old = std::mem::replace(&mut platform.pendragon, pendragon_with(short));
    old.stop().unwrap();
    let api = platform.api();
    let r = api.expect(
        201,
        api.post(
            "vms",
            &obj([
                ("name", "quiet".into()),
                ("ram", "8".into()),
                ("program", program("uart-echo")),
                ("start", true.into()),
            ]),
        ),
    );
    let id = n(&r, "id");
    let tap = n(&api.expect(200, api.get(&format!("vms/{id}/console"))), "tap");
    // Well past the reply time limit, with nothing to read.
    let r = api.expect(200, api.get(&format!("vms/{id}/console?tap={tap}&wait=1000")));
    assert_eq!(r.get("ended"), Some(&Json::Null), "{r}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    api.expect(
        200,
        api.post(
            &format!("vms/{id}/console"),
            &obj([("data", "still here".into())]),
        ),
    );
    let (same, _, _) = read_console_until(&api, id, Some(tap), 0, "still here");
    assert_eq!(same, tap);
    platform.stop();
}

/// Replaces the platform's Pendragon with one configured by `change`.
fn reconfigure(platform: &mut Platform, change: impl FnOnce(&mut pendragon::Config)) {
    let mut config = pendragon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        avalon: platform.avalon.addr().to_string(),
        lake: platform.lake_addr(),
        log: false,
        ..pendragon::Config::default()
    };
    change(&mut config);
    let old = std::mem::replace(&mut platform.pendragon, pendragon_with(config));
    old.stop().unwrap();
}

/// Creates and starts a VM running uart-echo; returns its id.
fn echo_vm(api: &Api, name: &str, disks: Vec<Json>) -> u64 {
    let r = api.expect(
        201,
        api.post(
            "vms",
            &obj([
                ("name", name.into()),
                ("ram", "8".into()),
                ("program", program("uart-echo")),
                ("disks", Json::Array(disks)),
                ("start", true.into()),
            ]),
        ),
    );
    n(&r, "id")
}

#[test]
fn consoles_are_limited_and_closed_when_unused() {
    let mut platform = Platform::start("consoles", 64);
    reconfigure(&mut platform, |c| {
        c.consoles = pendragon::ConsoleLimits {
            max_taps: 1,
            idle: std::time::Duration::from_millis(300),
            ..pendragon::ConsoleLimits::default()
        }
    });
    let api = platform.api();
    let one = echo_vm(&api, "one", vec![]);
    let two = echo_vm(&api, "two", vec![]);
    let consoles = |api: &Api| n(&api.expect(200, api.get("overview")), "consoles");
    assert_eq!(consoles(&api), 0);
    let tap = n(&api.expect(200, api.get(&format!("vms/{one}/console"))), "tap");
    assert_eq!(consoles(&api), 1);
    // A second console is over Pendragon's budget.
    let r = api.get(&format!("vms/{two}/console"));
    assert_eq!(r.status, 409);
    assert_eq!(r.json().get("code").and_then(Json::as_str), Some("over budget"));
    assert!(r.error().contains("1 consoles open already"), "{}", r.error());
    // Nobody reads the first: it is closed on time, with no other console request
    // to notice (the overview does not touch consoles).
    wait_for("the unused console to close", || {
        (consoles(&api) == 0).then_some(())
    });
    api.expect(200, api.get(&format!("vms/{two}/console")));
    assert_eq!(consoles(&api), 1);
    wait_for("the second console to close", || {
        (consoles(&api) == 0).then_some(())
    });
    // The first VM's page comes back to a new tap, and starts over.
    let r = api.expect(200, api.get(&format!("vms/{one}/console?tap={tap}&from=5")));
    assert_ne!(n(&r, "tap"), tap);
    assert_eq!((r.get("reset"), n(&r, "from")), (Some(&Json::Bool(true)), 0));
    platform.stop();
}

#[test]
fn a_detach_says_when_the_disk_is_still_held() {
    // Two Avalons on one Lake, each with a VM reading the same image: detaching
    // through Pendragon stops the VM on its own Avalon, but the image stays held.
    let mut platform = Platform::start("held", 64);
    reconfigure(&mut platform, |c| {
        c.release_wait = std::time::Duration::from_millis(300)
    });
    let other_avalon = avalon::Server::bind(&avalon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        max_ram: Some(64 * MIB),
        ..avalon::Config::default()
    })
    .unwrap()
    .spawn()
    .unwrap();
    let other = pendragon_for(&other_avalon.addr().to_string(), &platform.lake_addr(), 64 * MIB);
    let api = platform.api();
    let other_api = Api::new(&other);
    api.expect(
        201,
        api.post("disks", &obj([("name", "shared".into()), ("size", "8s".into())])),
    );
    let disk = || {
        vec![obj([
            ("source", format!("lake://{}/shared", platform.lake_addr()).into()),
            ("read_only", true.into()),
        ])]
    };
    echo_vm(&api, "here", disk());
    echo_vm(&other_api, "there", disk());
    let r = api.expect(200, api.action("disks/shared/detach"));
    assert_eq!(list(&r, "stopped").len(), 1);
    assert_eq!(r.get("released"), Some(&Json::Bool(false)), "{r}");
    // Once the other lets go too, it is released.
    let r = other_api.expect(200, other_api.action("disks/shared/detach"));
    assert_eq!(r.get("released"), Some(&Json::Bool(true)), "{r}");
    other.stop().unwrap();
    other_avalon.stop().unwrap();
    platform.stop();
}

fn list_of(v: &Json, key: &str) -> Vec<Json> {
    list(v, key).to_vec()
}

fn disk_in(list: &Json, name: &str) -> Json {
    list_of(list, "disks")
        .into_iter()
        .find(|d| s(d, "name") == name)
        .unwrap_or_else(|| panic!("no disk {name} in {list}"))
}

fn actions(vm: &Json) -> Vec<String> {
    list(vm, "actions")
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

/// Reads a console until its output contains `want`; returns the tap and the next
/// offset.
fn read_console_until(api: &Api, vm: u64, tap: Option<u64>, from: u64, want: &str) -> (u64, u64, String) {
    let mut text = String::new();
    let (mut tap, mut next) = (tap, from);
    wait_for(&format!("'{want}' on the console"), || {
        let query = match tap {
            Some(t) => format!("from={next}&tap={t}&wait=2000"),
            None => format!("from={next}&wait=2000"),
        };
        let r = api.expect(200, api.get(&format!("vms/{vm}/console?{query}")));
        assert_eq!(n(&r, "from"), next, "{r}");
        text += &String::from_utf8(pendragon::base64::decode(s(&r, "data")).unwrap()).unwrap();
        tap = Some(n(&r, "tap"));
        next = n(&r, "next");
        text.contains(want).then_some(())
    });
    (tap.unwrap(), next, text)
}

#[test]
fn vms_and_disks_end_to_end() {
    let platform = Platform::start("e2e", 256);
    let api = platform.api();
    let lake = platform.lake_addr();

    // The overview: both servers up, nothing on them.
    let o = api.expect(200, api.get("overview"));
    assert_eq!(o.get("avalon").unwrap().get("ok"), Some(&Json::Bool(true)));
    assert!(s(o.get("avalon").unwrap(), "server").starts_with("avalon "));
    assert!(s(o.get("lake").unwrap(), "server").starts_with("lake "));
    assert_eq!(n(o.get("ram").unwrap(), "used"), 0);
    assert_eq!(n(o.get("ram").unwrap(), "budget"), 256 * MIB);
    assert_eq!(s(o.get("ram").unwrap(), "budget_text"), "256 MiB");
    assert_eq!(n(o.get("disks").unwrap(), "total"), 0);
    assert_eq!(n(o.get("vms").unwrap(), "total"), 0);

    // ---- Disks: create, list ----
    let created = api.expect(
        201,
        api.post("disks", &obj([("name", "data".into()), ("size", "1MiB".into())])),
    );
    assert_eq!(n(&created, "sectors"), 2048);
    assert_eq!(s(&created, "size_text"), "1 MiB");
    assert_eq!(
        std::fs::metadata(platform.dir.path().join("data")).unwrap().len(),
        MIB
    );
    api.expect(
        201,
        api.post("disks", &obj([("name", "boot".into()), ("size", "8s".into())])),
    );
    let disks = api.expect(200, api.get("disks"));
    assert_eq!(s(&disks, "lake"), lake);
    let data = disk_in(&disks, "data");
    assert_eq!(n(&data, "bytes"), MIB);
    assert_eq!(data.get("attached"), Some(&Json::Bool(false)));
    assert_eq!(list(&data, "attached_to").len(), 0);
    assert_eq!(disks.get("vms_error"), Some(&Json::Null));
    // In name order, as Lake lists them.
    let names: Vec<&str> = list(&disks, "disks").iter().map(|d| s(d, "name")).collect();
    assert_eq!(names, ["boot", "data"]);

    let r = api.post("disks", &obj([("name", "data".into()), ("size", "1M".into())]));
    assert_eq!(r.status, 409);
    assert_eq!(
        r.json().get("code").and_then(Json::as_str),
        Some("already exists")
    );
    let r = api.post("disks", &obj([("name", "Data".into()), ("size", "1M".into())]));
    assert_eq!(
        (
            r.status,
            r.error().starts_with("name: 'Data' is not a valid name")
        ),
        (400, true)
    );
    let r = api.post("disks", &obj([("name", "x".into()), ("size", "100".into())]));
    assert_eq!(
        (r.status, r.error().starts_with("size: 100 is not a whole number")),
        (400, true)
    );
    let r = api.post("disks", &obj([("name", "x".into())]));
    assert_eq!((r.status, r.error().as_str()), (400, "size: a disk needs a size"));

    // ---- A VM on a Lake disk: create (attach), list, info ----
    let source = format!("lake://{lake}/data");
    let created = api.expect(
        201,
        api.post(
            "vms",
            &obj([
                ("name", "echo".into()),
                ("ram", "16".into()),
                ("clock", "".into()),
                ("program", program("uart-echo")),
                (
                    "disks",
                    Json::Array(vec![obj([("source", source.as_str().into())])]),
                ),
                ("start", true.into()),
            ]),
        ),
    );
    let echo = n(&created, "id");
    assert_eq!(created.get("started"), Some(&Json::Bool(true)));

    let vms = api.expect(200, api.get("vms"));
    let vm = vm_in(&vms, "echo");
    assert_eq!(n(&vm, "id"), echo);
    assert_eq!(s(&vm, "state"), "running");
    assert_eq!(s(&vm, "ram_text"), "16 MiB");
    assert_eq!(s(&vm, "clock_text"), "full speed");
    assert_eq!(actions(&vm), ["pause", "stop", "destroy"]);
    let disk = &list(&vm, "disks")[0];
    assert_eq!(s(disk, "source"), source);
    assert_eq!(disk.get("read_only"), Some(&Json::Bool(false)));
    assert_eq!(disk.get("held"), Some(&Json::Bool(true)));

    // By id and by name.
    let info = api.expect(200, api.get(&format!("vms/{echo}")));
    assert_eq!(s(&info, "name"), "echo");
    assert!(list(&info, "devices").iter().any(|d| s(d, "name") == "UART"));
    assert!(list(&info, "devices").iter().any(|d| s(d, "name") == "block"));
    let by_name = api.expect(200, api.get("vms/echo"));
    assert_eq!(n(&by_name, "id"), echo);
    let r = api.get("vms/nope");
    assert_eq!(
        (r.status, r.json().get("code").and_then(Json::as_str)),
        (404, Some("no such VM"))
    );
    assert_eq!(api.get("vms/4294967296").status, 404);

    // The disk page knows who holds it.
    let data = disk_in(&api.expect(200, api.get("disks")), "data");
    assert_eq!(data.get("writer"), Some(&Json::Bool(true)));
    let holders = list(&data, "attached_to");
    assert_eq!(holders.len(), 1);
    assert_eq!((n(&holders[0], "id"), s(&holders[0], "name")), (echo, "echo"));
    assert_eq!(holders[0].get("read_only"), Some(&Json::Bool(false)));

    // An attached disk cannot be deleted, nor attached read-write a second time.
    let r = api.delete("disks/data");
    assert_eq!(
        (r.status, r.json().get("code").and_then(Json::as_str)),
        (409, Some("busy"))
    );
    let r = api.post(
        "vms",
        &obj([
            ("name", "second".into()),
            ("ram", "16".into()),
            ("program", program("uart-echo")),
            (
                "disks",
                Json::Array(vec![obj([("source", source.as_str().into())])]),
            ),
        ]),
    );
    assert_eq!(r.status, 422, "{}", r.text());
    assert!(r.error().contains("busy"), "{}", r.error());

    // The overview counts it all.
    let o = api.expect(200, api.get("overview"));
    assert_eq!(n(o.get("ram").unwrap(), "used"), 16 * MIB);
    assert_eq!(n(o.get("vms").unwrap(), "running"), 1);
    assert_eq!(n(o.get("disks").unwrap(), "attached"), 1);

    // ---- The console: output live, input to the UART ----
    let (tap, next, _) = {
        let r = api.expect(200, api.get(&format!("vms/{echo}/console")));
        (n(&r, "tap"), n(&r, "next"), ())
    };
    api.expect(
        200,
        api.post(
            &format!("vms/{echo}/console"),
            &obj([("data", "hello pendragon\n".into())]),
        ),
    );
    let (tap2, next, text) = read_console_until(&api, echo, Some(tap), next, "hello pendragon");
    assert_eq!(tap2, tap, "the same tap serves later reads");
    assert!(text.starts_with("hello pendragon"), "{text:?}");
    // Asking again from the start (a new page) gets the same output.
    let r = api.expect(200, api.get(&format!("vms/{echo}/console?from=0&tap={tap}")));
    assert_eq!(n(&r, "next"), next);
    // A page from an earlier tap starts over, and is told so.
    let r = api.expect(200, api.get(&format!("vms/{echo}/console?from=5&tap=1")));
    assert_eq!(r.get("reset"), Some(&Json::Bool(true)));
    assert_eq!(n(&r, "from"), 0);
    // A read with nothing new waits, up to what it asked for.
    let start = std::time::Instant::now();
    let r = api.expect(
        200,
        api.get(&format!("vms/{echo}/console?from={next}&tap={tap}&wait=300")),
    );
    assert!(start.elapsed() >= std::time::Duration::from_millis(250));
    assert_eq!(n(&r, "next"), next);
    assert_eq!(s(&r, "data"), "");
    // ... and returns as soon as there is output.
    let reader = {
        let api = Api {
            addr: api.addr,
            token: api.token.clone(),
        };
        std::thread::spawn(move || {
            api.expect(
                200,
                api.get(&format!("vms/{echo}/console?from={next}&tap={tap}&wait=8000")),
            )
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    let start = std::time::Instant::now();
    api.expect(200, api.post("vms/echo/console", &obj([("data", "x".into())])));
    let r = reader.join().unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(pendragon::base64::decode(s(&r, "data")).unwrap(), b"x", "{r}");
    for bad in ["from=-1", "from=x", "tap=1.5", "wait=lots"] {
        assert_eq!(api.get(&format!("vms/{echo}/console?{bad}")).status, 400, "{bad}");
    }
    let r = api.post(&format!("vms/{echo}/console"), &obj([("text", "x".into())]));
    assert_eq!(r.status, 400);

    // ---- Life cycle: pause, resume; the wrong state is refused ----
    api.expect(200, api.action(&format!("vms/{echo}/pause")));
    let vm = api.expect(200, api.get(&format!("vms/{echo}")));
    assert_eq!(s(&vm, "state"), "paused");
    assert_eq!(actions(&vm), ["resume", "stop", "destroy"]);
    let r = api.action(&format!("vms/{echo}/pause"));
    assert_eq!(
        (r.status, r.json().get("code").and_then(Json::as_str)),
        (409, Some("invalid state"))
    );
    api.expect(200, api.action("vms/echo/resume"));
    assert_eq!(s(&api.expect(200, api.get("vms/echo")), "state"), "running");
    assert_eq!(api.action(&format!("vms/{echo}/reboot")).status, 404);

    // ---- Detach: the VM holding the disk stops and lets go of it ----
    let r = api.expect(200, api.action("disks/data/detach"));
    assert_eq!(n(&list(&r, "stopped")[0], "id"), echo);
    assert_eq!(r.get("released"), Some(&Json::Bool(true)));
    let vm = api.expect(200, api.get(&format!("vms/{echo}")));
    assert_eq!(s(&vm, "state"), "stopped");
    assert_eq!(actions(&vm), ["destroy"]);
    assert_eq!(list(&vm, "disks")[0].get("held"), Some(&Json::Bool(false)));
    let data = disk_in(&api.expect(200, api.get("disks")), "data");
    assert_eq!(data.get("attached"), Some(&Json::Bool(false)));
    assert_eq!(list(&data, "attached_to").len(), 0);
    let r = api.action("disks/data/detach");
    assert_eq!(r.status, 409);
    assert!(r.error().starts_with("no VM on Avalon at"), "{}", r.error());

    // ---- Read-only attachments are shared ----
    for name in ["reader-a", "reader-b"] {
        api.expect(
            201,
            api.post(
                "vms",
                &obj([
                    ("name", name.into()),
                    ("ram", "16".into()),
                    ("program", program("uart-echo")),
                    (
                        "disks",
                        Json::Array(vec![obj([
                            ("source", format!("lake://{lake}/boot").into()),
                            ("read_only", true.into()),
                        ])]),
                    ),
                ]),
            ),
        );
    }
    let boot = disk_in(&api.expect(200, api.get("disks")), "boot");
    assert_eq!(n(&boot, "readers"), 2);
    let holders: Vec<&str> = list(&boot, "attached_to").iter().map(|h| s(h, "name")).collect();
    assert_eq!(holders, ["reader-a", "reader-b"]);
    assert!(list(&boot, "attached_to")
        .iter()
        .all(|h| h.get("read_only") == Some(&Json::Bool(true))));
    // Created VMs hold their disks too: detaching stops both.
    let r = api.expect(200, api.action("disks/boot/detach"));
    assert_eq!(list(&r, "stopped").len(), 2);
    assert_eq!(r.get("released"), Some(&Json::Bool(true)));

    // ---- Delete and destroy ----
    api.expect(200, api.delete("disks/data"));
    assert!(!platform.dir.path().join("data").exists());
    let r = api.delete("disks/data");
    assert_eq!(
        (r.status, r.json().get("code").and_then(Json::as_str)),
        (404, Some("no such disk"))
    );
    for vm in ["echo", "reader-a", "reader-b"] {
        api.expect(200, api.action(&format!("vms/{vm}/destroy")));
    }
    assert_eq!(list(&api.expect(200, api.get("vms")), "vms").len(), 0);
    // The console's session ends with the VM.
    wait_for("the console to go", || {
        (api.get(&format!("vms/{echo}/console")).status == 404).then_some(())
    });
    assert_eq!(
        n(api.expect(200, api.get("overview")).get("ram").unwrap(), "used"),
        0
    );
    platform.stop();
}

#[test]
fn create_options_and_refusals() {
    let platform = Platform::start("create", 64);
    let api = platform.api();
    let create = |members: Vec<(&str, Json)>| api.post("vms", &obj(members));

    // Real time, not started.
    let r = api.expect(
        201,
        create(vec![
            ("name", "clocked".into()),
            ("ram", "8".into()),
            ("clock", "1.5MHz".into()),
            ("program", program("sysinfo")),
        ]),
    );
    let id = n(&r, "id");
    let vm = api.expect(200, api.get(&format!("vms/{id}")));
    assert_eq!(s(&vm, "state"), "created");
    assert_eq!(n(&vm, "clock_hz"), 1_500_000);
    assert_eq!(s(&vm, "clock_text"), "1.5 MHz");
    assert_eq!(n(&vm, "ram"), 8 * MIB);
    assert_eq!(actions(&vm), ["start", "stop", "destroy"]);
    // sysinfo prints its profile and exits.
    api.expect(200, api.action(&format!("vms/{id}/start")));
    let (_, _, text) = read_console_until(&api, id, None, 0, "devices");
    assert!(text.contains("RAM      8 MiB"), "{text}");
    let vm = wait_for("sysinfo to exit", || {
        let vm = api.expect(200, api.get(&format!("vms/{id}")));
        (s(&vm, "state") == "exited").then_some(vm)
    });
    assert_eq!(s(&vm, "state_text"), "exited(0)");
    assert_eq!(n(&vm, "exit_code"), 0);

    // Refusals, with the protocol's codes.
    // The request, the status, the protocol error code.
    type Case<'a> = (Vec<(&'a str, Json)>, u16, Option<&'a str>);
    let cases: Vec<Case> = vec![
        (
            vec![
                ("name", "clocked".into()),
                ("ram", "8".into()),
                ("program", program("hello")),
            ],
            409,
            Some("already exists"),
        ),
        (
            vec![
                ("name", "big".into()),
                ("ram", "128".into()),
                ("program", program("hello")),
            ],
            409,
            Some("over budget"),
        ),
        (
            vec![
                ("name", "junk".into()),
                ("ram", "8".into()),
                ("program", "AAAA".into()),
            ],
            422,
            Some("invalid program"),
        ),
        (
            vec![
                ("name", "nodisk".into()),
                ("ram", "8".into()),
                ("program", program("hello")),
                (
                    "disks",
                    Json::Array(vec![obj([(
                        "source",
                        format!("lake://{}/missing", platform.lake_addr()).into(),
                    )])]),
                ),
            ],
            422,
            Some("invalid hardware profile"),
        ),
        (
            vec![
                ("name", "x".into()),
                ("ram", "4096".into()),
                ("program", program("hello")),
            ],
            400,
            None,
        ),
        (
            vec![
                ("name", "x".into()),
                ("clock", "0".into()),
                ("program", program("hello")),
            ],
            400,
            None,
        ),
        (vec![("name", "x".into())], 400, None),
    ];
    for (members, status, code) in cases {
        let text = obj(members.clone()).encode();
        let r = create(members);
        assert_eq!(r.status, status, "{text}: {}", r.text());
        assert_eq!(r.json().get("code").and_then(Json::as_str), code, "{text}");
        assert!(!r.error().is_empty());
    }
    // A body that is not a JSON object.
    for body in ["", "[]", "{", "\"x\"", "{\"name\": \"a\",}"] {
        let r = request(
            api.addr,
            "POST",
            "/api/vms",
            &[
                ("X-Pendragon-Token", &api.token),
                ("Content-Type", "application/json"),
            ],
            body.as_bytes(),
        );
        assert_eq!(r.status, 400, "{body:?}: {}", r.text());
    }
    platform.stop();
}

/// A switch's JSON in the list, by name.
fn switch_in(list: &Json, name: &str) -> Json {
    list_of(list, "switches")
        .into_iter()
        .find(|sw| s(sw, "name") == name)
        .unwrap_or_else(|| panic!("no switch {name} in {list}"))
}

fn code(r: &Reply) -> Option<String> {
    r.json().get("code").and_then(Json::as_str).map(str::to_string)
}

/// The host port of a forwarding rule as Pendragon shows it (`tcp:127.0.0.1:PORT-...`).
fn host_port(rule: &str) -> u16 {
    let host = rule.split('-').next().unwrap();
    host.rsplit(':').next().unwrap().parse().unwrap()
}

#[test]
fn switches_end_to_end() {
    // The switches page's requests: create (with and without a NAT gateway), plug
    // cards in and out, give a gateway and take it away, delete; a VM running
    // examples/net/net-user gets a lease that names it, and deleting the switch frees
    // the gateway's host ports.
    let platform = Platform::start("switches", 64);
    let api = platform.api();
    let none = api.expect(200, api.get("switches"));
    assert!(list_of(&none, "switches").is_empty(), "{none}");
    assert_eq!(none.get("vms_error"), Some(&Json::Null));

    // A plain switch, and one with a gateway forwarding a host port the system picks.
    let r = api.expect(201, api.post("switches", &obj([("name", "lan".into())])));
    assert_eq!(r.get("nat"), Some(&Json::Null), "{r}");
    let r = api.expect(
        201,
        api.post(
            "switches",
            &obj([
                ("name", "wan".into()),
                ("nat", true.into()),
                ("hostfwd", Json::Array(vec!["tcp::0-:7".into()])),
            ]),
        ),
    );
    let nat = r.get("nat").unwrap();
    assert_eq!(s(nat, "gateway"), "10.0.2.2");
    let rule = list(nat, "hostfwd")[0].as_str().unwrap().to_string();
    assert!(
        rule.starts_with("tcp:127.0.0.1:") && rule.ends_with("-10.0.2.15:7"),
        "{rule}"
    );
    let port = host_port(&rule);
    assert_ne!(port, 0);
    // The port is the gateway's now.
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err());

    // A VM with two cards: one on lan, one on wan, running net-user (which asks the
    // gateway for an address as it starts).
    let net_user =
        pendragon::base64::encode(&std::fs::read(example("net").with_file_name("net-user.elf")).unwrap());
    let r = api.expect(
        201,
        api.post(
            "vms",
            &obj([
                ("name", "user".into()),
                ("ram", "8".into()),
                ("program", net_user.into()),
                (
                    "nets",
                    Json::Array(vec![
                        obj([("switch", "wan".into()), ("mac", "02:ca:00:00:00:0a".into())]),
                        obj([("switch", "lan".into())]),
                    ]),
                ),
                ("start", true.into()),
            ]),
        ),
    );
    let id = n(&r, "id");
    let vm = api.expect(200, api.get(&format!("vms/{id}")));
    let nets = list(&vm, "nets");
    assert_eq!(nets.len(), 2, "{vm}");
    assert_eq!(
        (n(&nets[0], "card"), s(&nets[0], "mac"), s(&nets[0], "switch")),
        (0, "02:ca:00:00:00:0a", "wan")
    );
    assert_eq!((n(&nets[1], "card"), s(&nets[1], "switch")), (1, "lan"));

    // The lease names the VM, and the gateway's port counts the DHCP exchange.
    let wan = wait_for("net-user's lease", || {
        let wan = switch_in(&api.expect(200, api.get("switches")), "wan");
        let leases = list(wan.get("nat").unwrap(), "leases").to_vec();
        (!leases.is_empty() && n(&list(&wan, "ports")[0], "in_frames") > 0).then_some(wan)
    });
    let lease = &list(wan.get("nat").unwrap(), "leases")[0];
    assert_eq!(
        (s(lease, "addr"), s(lease, "mac")),
        ("10.0.2.15", "02:ca:00:00:00:0a")
    );
    let owner = lease.get("vm").unwrap();
    assert_eq!((n(owner, "id"), s(owner, "name")), (id, "user"));
    let ports = list(&wan, "ports");
    assert_eq!(ports.len(), 2, "{wan}");
    assert_eq!(s(&ports[0], "kind"), "gateway");
    assert_eq!(ports[0].get("vm"), Some(&Json::Null));
    assert_eq!((s(&ports[1], "kind"), n(&ports[1], "card")), ("card", 0));
    assert_eq!(s(ports[1].get("vm").unwrap(), "name"), "user");
    assert!(n(&ports[1], "out_frames") > 0, "{wan}");

    // Unplugging names the switch the card is on.
    let r = api.post(
        "switches/lan/detach",
        &obj([("vm", "user".into()), ("card", 0u8.into())]),
    );
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(
        r.error().contains("is not plugged into switch 'lan'"),
        "{}",
        r.error()
    );
    api.expect(
        200,
        api.post(
            "switches/lan/detach",
            &obj([("vm", "user".into()), ("card", 1u8.into())]),
        ),
    );
    let lan = switch_in(&api.expect(200, api.get("switches")), "lan");
    assert!(list(&lan, "ports").is_empty(), "{lan}");
    let r = api.expect(
        200,
        api.post(
            "switches/lan/attach",
            &obj([("vm", Json::from(id)), ("card", 1u8.into())]),
        ),
    );
    assert_eq!(r.get("plugged"), Some(&Json::Bool(true)));
    assert_eq!(
        list(&switch_in(&api.expect(200, api.get("switches")), "lan"), "ports").len(),
        1
    );

    // A gateway for lan, then taken away.
    let r = api.expect(
        200,
        api.post("switches/lan/nat", &obj([("hostfwd", Json::Array(vec![]))])),
    );
    assert!(list(r.get("nat").unwrap(), "hostfwd").is_empty(), "{r}");
    let r = api.post("switches/lan/nat", &obj([]));
    assert_eq!(
        (r.status, code(&r).as_deref()),
        (409, Some("already exists")),
        "{}",
        r.text()
    );
    let r = api.expect(200, api.delete("switches/lan/nat"));
    assert_eq!(r.get("nat"), Some(&Json::Null));
    let r = api.delete("switches/lan/nat");
    assert_eq!(
        (r.status, code(&r).as_deref()),
        (409, Some("invalid state")),
        "{}",
        r.text()
    );

    // A host port that is taken: a clear error, and no switch.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = format!("tcp::{}-:80", taken.local_addr().unwrap().port());
    let r = api.post(
        "switches",
        &obj([
            ("name", "busy".into()),
            ("nat", true.into()),
            ("hostfwd", Json::Array(vec![busy.into()])),
        ]),
    );
    assert_eq!(
        (r.status, code(&r).as_deref()),
        (502, Some("I/O error")),
        "{}",
        r.text()
    );
    assert!(r.error().contains("cannot forward the port"), "{}", r.error());
    let names: Vec<String> = list_of(&api.expect(200, api.get("switches")), "switches")
        .iter()
        .map(|sw| s(sw, "name").to_string())
        .collect();
    assert_eq!(names, ["lan", "wan"]);
    drop(taken);

    // Refusals before Avalon is asked, and Avalon's own.
    for (path, body, status, want) in [
        ("switches", obj([("name", "Bad".into())]), 400, "name: "),
        (
            "switches",
            obj([
                ("name", "x".into()),
                ("hostfwd", Json::Array(vec!["tcp::1-:2".into()])),
            ]),
            400,
            "set nat too",
        ),
        (
            "switches",
            obj([
                ("name", "x".into()),
                ("nat", true.into()),
                ("hostfwd", Json::Array(vec!["nope".into()])),
            ]),
            400,
            "hostfwd",
        ),
        (
            "switches",
            obj([("name", "x".into()), ("speed", 1u8.into())]),
            400,
            "unknown field 'speed'",
        ),
        ("switches", obj([("name", "wan".into())]), 409, "already exists"),
        ("switches/nope/nat", obj([]), 404, "nope"),
        ("switches/nope/attach", obj([("vm", "user".into())]), 404, "nope"),
        ("switches/wan/attach", obj([("vm", "ghost".into())]), 404, "ghost"),
        (
            "switches/wan/attach",
            obj([("vm", "user".into()), ("card", 9u8.into())]),
            404,
            "has 2 network cards",
        ),
        // Unplugging says so the same way: 404 for a card or a switch that is not there.
        (
            "switches/wan/detach",
            obj([("vm", "user".into()), ("card", 9u8.into())]),
            404,
            "has 2 network cards",
        ),
        (
            "switches/nope/detach",
            obj([("vm", "user".into())]),
            404,
            "no switch named 'nope'",
        ),
        ("switches/wan/detach", obj([("vm", "ghost".into())]), 404, "ghost"),
        ("switches/wan/attach", obj([]), 400, "vm: "),
        (
            "switches/wan/attach",
            obj([("vm", "user".into()), ("card", "x".into())]),
            400,
            "card: ",
        ),
    ] {
        let r = api.post(path, &body);
        assert_eq!(r.status, status, "{path} {body}: {}", r.text());
        assert!(r.error().contains(want), "{path} {body}: {}", r.error());
    }
    let r = api.delete("switches/nope");
    assert_eq!((r.status, code(&r).as_deref()), (404, Some("no such switch")));

    // Deleting wan unplugs the VM's card and frees the forwarded host port at once.
    assert_eq!(s(&api.expect(200, api.delete("switches/wan")), "deleted"), "wan");
    drop(std::net::TcpListener::bind(("127.0.0.1", port)).expect("the forwarded port is free again"));
    let vm = api.expect(200, api.get(&format!("vms/{id}")));
    assert_eq!(list(&vm, "nets")[0].get("switch"), Some(&Json::Null), "{vm}");
    api.expect(200, api.delete("switches/lan"));
    assert!(list_of(&api.expect(200, api.get("switches")), "switches").is_empty());
    platform.stop();
}

#[test]
fn servers_that_are_away() {
    // Pendragon starts and serves its pages whatever the servers do; each request
    // says which one it could not reach.
    let avalon = dead_addr();
    let lake = dead_addr();
    let server = pendragon_for(&avalon, &lake, 64 * MIB);
    let api = Api::new(&server);
    let o = api.expect(200, api.get("overview"));
    let a = o.get("avalon").unwrap();
    assert_eq!(a.get("ok"), Some(&Json::Bool(false)));
    assert!(
        s(a, "error").starts_with(&format!("cannot connect to Avalon at {avalon}")),
        "{a}"
    );
    assert!(s(o.get("lake").unwrap(), "error").starts_with(&format!("cannot connect to Lake at {lake}")));
    assert_eq!(o.get("ram"), Some(&Json::Null));
    let r = api.get("vms");
    assert_eq!(r.status, 502);
    assert!(r.error().starts_with("cannot connect to Avalon"), "{}", r.error());
    assert_eq!(api.get("disks").status, 502);
    assert_eq!(api.get("vms/1/console").status, 502);
    assert_eq!(request(api.addr, "GET", "/", &[], b"").status, 200);
    server.stop().unwrap();

    // Lake up, Avalon away: the disks are listed, without who holds them.
    let dir = TempDir::new("no-avalon");
    let lake = lake::Server::bind(&lake::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        ..lake::Config::new(dir.path())
    })
    .unwrap()
    .spawn()
    .unwrap();
    let server = pendragon_for(&avalon, &lake.addr().to_string(), 64 * MIB);
    let api = Api::new(&server);
    api.expect(
        201,
        api.post("disks", &obj([("name", "d".into()), ("size", "1K".into())])),
    );
    let disks = api.expect(200, api.get("disks"));
    assert!(s(&disks, "vms_error").starts_with("cannot connect to Avalon"));
    assert_eq!(disk_in(&disks, "d").get("attached_to"), Some(&Json::Null));
    server.stop().unwrap();
    lake.stop().unwrap();
}

#[test]
fn routes_and_methods() {
    let server = pendragon_for(&hang_up_addr(), &hang_up_addr(), MIB);
    let api = Api::new(&server);
    for (method, path, status, body) in [
        ("GET", "nope", 404, None),
        ("GET", "vms/1/2/3", 404, None),
        ("PUT", "vms", 405, None),
        ("DELETE", "vms/1", 405, None),
        ("GET", "vms/1/start", 405, None),
        ("GET", "disks/x", 405, None),
        ("POST", "overview", 405, None),
        ("GET", "disks/x/detach", 405, None),
        ("PUT", "switches", 405, None),
        ("GET", "switches/x", 405, None),
        ("GET", "switches/x/nat", 405, None),
        ("GET", "switches/x/attach", 405, None),
        ("DELETE", "switches/x/detach", 405, None),
        ("GET", "switches/x/y", 404, None),
        ("POST", "disks/Bad/detach", 404, None),
        ("POST", "", 404, None),
        (
            "POST",
            "vms/1/console?x=1",
            400,
            Some(obj([("data", Json::Null)])),
        ),
        (
            "POST",
            "disks",
            400,
            Some(obj([
                ("name", "a".into()),
                ("size", "1K".into()),
                ("x", 1u8.into()),
            ])),
        ),
    ] {
        let r = api.call(method, path, body.as_ref());
        assert_eq!(r.status, status, "{method} {path}: {}", r.text());
        assert!(r.header("content-type").unwrap().starts_with("application/json"));
        assert!(!r.error().is_empty());
    }
    for (method, path, status) in [
        ("GET", "/", 200),
        ("GET", "/app.js", 200),
        ("GET", "/style.css", 200),
        ("GET", "/favicon.ico", 404),
        ("GET", "/index.html", 404),
        ("POST", "/", 405),
        ("GET", "/../Cargo.toml", 404),
    ] {
        assert_eq!(
            request(api.addr, method, path, &[], b"").status,
            status,
            "{method} {path}"
        );
    }
    server.stop().unwrap();
}
