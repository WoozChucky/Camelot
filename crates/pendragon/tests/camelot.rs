//! Creating Camelot VMs from the page, over HTTP, against Avalon and Lake in process:
//! no ELF to upload, each VM on a copy of Lake's `os-base` image of its own, two of
//! them running at once; and the page's form that offers it.
//!
//! The `os-base` image here is a small bootable disk whose boot block spins forever,
//! so a VM booted from it stays running and holds its disk; the real Camelot image is
//! booted by `cargo xtask up --demo`'s test (crates/xtask/tests/up.rs).

mod common;

use common::*;
use pendragon::json::Json;

/// A bootable image of `sectors` sectors whose boot code is `j .`: it runs until it
/// is stopped.
fn spinning_image(sectors: usize) -> Vec<u8> {
    let mut block = [0u8; grail::boot::BOOT_BLOCK_SIZE];
    block[..4].copy_from_slice(&0x0000_006fu32.to_le_bytes()); // jal x0, 0
    grail::boot::seal(&mut block);
    let mut image = block.to_vec();
    image.resize(sectors * 512, 0);
    image
}

fn disk<'a>(disks: &'a Json, name: &str) -> &'a Json {
    list(disks, "disks")
        .iter()
        .find(|d| s(d, "name") == name)
        .unwrap_or_else(|| panic!("no disk {name} in {disks}"))
}

#[test]
fn camelot_vms_need_no_program() {
    let platform = Platform::start("camelot", 256);
    let api = platform.api();
    let lake = platform.lake_addr();
    let create = |members: Vec<(&str, Json)>| api.post("vms", &obj(members));

    // No image yet: the page is told its name, and a create says what to do.
    let disks = api.expect(200, api.get("disks"));
    assert_eq!(s(&disks, "camelot_image"), "os-base");
    let r = create(vec![("name", "early".into()), ("camelot", true.into())]);
    assert_eq!(r.status, 404, "{}", r.text());
    assert_eq!(
        r.error(),
        format!(
            "the Lake at {lake} has no 'os-base' image to copy (start the platform with \
             'cargo xtask up', which puts it in Lake's images directory)"
        )
    );
    assert_eq!(r.json().get("code").and_then(Json::as_str), Some("no such disk"));

    std::fs::write(platform.dir.path().join("os-base"), spinning_image(16)).unwrap();
    // Two VMs, no ELF: each boots from a copy of its own, and both run at once.
    for name in ["one", "two"] {
        let r = api.expect(
            201,
            create(vec![
                ("name", name.into()),
                ("ram", "16".into()),
                ("camelot", true.into()),
                ("start", true.into()),
            ]),
        );
        assert_eq!(s(&r, "disk"), format!("{name}-disk0"));
        assert_eq!(r.get("started"), Some(&Json::Bool(true)));
    }
    let vms = api.expect(200, api.get("vms"));
    for (i, name) in ["one", "two"].iter().enumerate() {
        let vm = &list(&vms, "vms")[i];
        assert_eq!((s(vm, "name"), s(vm, "state")), (*name, "running"));
        let disks = list(vm, "disks");
        assert_eq!(disks.len(), 1);
        assert_eq!(s(&disks[0], "source"), format!("lake://{lake}/{name}-disk0"));
        assert_eq!(disks[0].get("held"), Some(&Json::Bool(true)));
    }
    let disks = api.expect(200, api.get("disks"));
    for name in ["one-disk0", "two-disk0"] {
        let d = disk(&disks, name);
        assert_eq!(d.get("writer"), Some(&Json::Bool(true)), "{d}");
        assert_eq!(n(d, "sectors"), 16);
    }
    assert_eq!(disk(&disks, "os-base").get("writer"), Some(&Json::Bool(false)));
    // Byte for byte the image they were copied from.
    let base = std::fs::read(platform.dir.path().join("os-base")).unwrap();
    assert_eq!(
        std::fs::read(platform.dir.path().join("one-disk0")).unwrap(),
        base
    );

    // With other disks and cards, the copy is disk 0.
    api.expect(
        201,
        create(vec![
            ("name", "three".into()),
            ("ram", "16".into()),
            ("camelot", true.into()),
            (
                "disks",
                Json::Array(vec![obj([
                    ("source", format!("lake://{lake}/os-base").into()),
                    ("read_only", true.into()),
                ])]),
            ),
        ]),
    );
    let three = api.expect(200, api.get("vms/three"));
    let sources: Vec<&str> = list(&three, "disks").iter().map(|d| s(d, "source")).collect();
    assert_eq!(
        sources,
        [
            format!("lake://{lake}/three-disk0"),
            format!("lake://{lake}/os-base")
        ]
    );
    assert_eq!(s(&three, "state"), "created");

    // A program and a disk: the program is optional; the VM boots from the disk.
    api.expect(
        201,
        api.post("disks", &obj([("name", "plain".into()), ("size", "8K".into())])),
    );
    let r = api.expect(
        201,
        create(vec![
            ("name", "plain".into()),
            ("ram", "16".into()),
            (
                "disks",
                Json::Array(vec![obj([("source", format!("lake://{lake}/plain").into())])]),
            ),
        ]),
    );
    assert_eq!(r.get("disk"), Some(&Json::Null));

    // Refusals: the copy's name taken; Avalon's VM name taken (the copy is deleted
    // again); a program as well; no program and no disk.
    let r = create(vec![("name", "one".into()), ("camelot", true.into())]);
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(
        r.error(),
        "cannot create disk one-disk0: already exists: a disk named 'one-disk0' already exists"
    );
    let r = create(vec![("name", "plain".into()), ("camelot", true.into())]);
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.error().starts_with("already exists: "), "{}", r.error());
    assert!(!platform.dir.path().join("plain-disk0").exists());
    let r = create(vec![
        ("name", "four".into()),
        ("camelot", true.into()),
        ("program", program("uart-echo")),
    ]);
    assert_eq!(r.status, 400);
    assert_eq!(
        r.error(),
        "program: a Camelot VM boots from its disk, and takes no program"
    );
    let r = create(vec![("name", "four".into())]);
    assert_eq!(r.status, 400);
    assert_eq!(
        r.error(),
        "program: choose an ELF file to run, or a disk to boot from"
    );
    assert!(!platform.dir.path().join("four-disk0").exists());

    // Destroying a VM lets go of its copy, which can then be deleted.
    api.expect(200, api.action("vms/one/destroy"));
    wait_for("one-disk0 released", || {
        let disks = api.expect(200, api.get("disks"));
        (disk(&disks, "one-disk0").get("writer") == Some(&Json::Bool(false))).then_some(())
    });
    api.expect(200, api.delete("disks/one-disk0"));
    platform.stop();
}

/// The page's form: a choice between Camelot and a program, Camelot sending no
/// program and the program left out when there is a disk.
#[test]
fn the_new_vm_form_offers_camelot() {
    let platform = Platform::start("camelot-page", 64);
    let js = request(platform.pendragon.addr(), "GET", "/app.js", &[], b"").text();
    for needed in [
        "Camelot (boot from disk)",
        "Program (ELF)",
        "camelot: bootCamelot.checked",
        "list.camelot_image",
        "cargo xtask up",
        "or a disk to boot from",
        // The width and the paging, as Round Table's --xlen and --paging take them,
        // and the Camelot image of the width chosen.
        "xlen: xlen.value,",
        "paging: paging.value,",
        "option(\"32\", \"RV32\", true), option(\"64\", \"RV64\")",
        "option(\"sv48\", \"Sv48 and Sv39\", true), option(\"sv39\", \"Sv39 alone\")",
        "xlen.value === \"64\" ? images.camelot_image_rv64 : images.camelot_image",
        "xlen.addEventListener(\"change\", showBoot);",
        "[\"Arch\", vm.arch_text]",
    ] {
        assert!(js.contains(needed), "app.js lacks {needed:?}");
    }
    // The program goes only with the program choice.
    let send = js.find("request.program = toBase64").unwrap();
    assert_eq!(
        &js[js[..send].rfind("if (").unwrap()..send].trim(),
        &"if (!request.camelot && file) {"
    );
    // Camelot is the default when Lake has the image (and the form was not opened
    // for a disk of the VM's own): the statement that chooses it, whole.
    let choose = js.find("bootCamelot.checked = true;").unwrap();
    assert_eq!(
        &js[js[..choose].rfind("if (").unwrap()..choose].trim(),
        &"if (hasImage && !disk) {"
    );
    assert!(js.contains("const hasImage = list.disks.some((d) => d.name === list.camelot_image);"));
    // The form's RAM is the platform's default, in MiB: 4.
    let mib = avalon_proto::DEFAULT_RAM_SIZE >> 20;
    assert_eq!(mib, 4);
    assert!(js.contains(&format!(
        "const ram = el(\"input\", {{ type: \"text\", value: \"{mib}\" }});"
    )));
    let css = request(platform.pendragon.addr(), "GET", "/style.css", &[], b"").text();
    assert!(css.contains("fieldset.boot") && css.contains("label[hidden] { display: none; }"));
    platform.stop();
}

/// A Camelot VM is a change like any other: another site's page, a request without
/// the token, a form body or a foreign `Host` is refused before anything is copied.
#[test]
fn camelot_creates_are_guarded() {
    let platform = Platform::start("camelot-guard", 64);
    std::fs::write(platform.dir.path().join("os-base"), spinning_image(16)).unwrap();
    let api = platform.api();
    let addr = api.addr;
    let token = ("X-Pendragon-Token", api.token.as_str());
    let json = ("Content-Type", "application/json");
    let own = format!("http://{addr}");
    let body = br#"{"name": "web", "camelot": true}"#;
    for (headers, why) in [
        (
            vec![token, json, ("Origin", "http://evil.example")],
            "foreign origin",
        ),
        (vec![json, ("Origin", own.as_str())], "no token"),
        (
            vec![("X-Pendragon-Token", "00"), json, ("Origin", own.as_str())],
            "wrong token",
        ),
        (
            vec![
                token,
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Origin", own.as_str()),
            ],
            "form body",
        ),
    ] {
        let r = request(addr, "POST", "/api/vms", &headers, body);
        assert_eq!(r.status, 403, "{why}: {}", r.text());
    }
    let head = format!(
        "POST /api/vms HTTP/1.1\r\nHost: evil.example\r\nX-Pendragon-Token: {}\r\nContent-Type: \
         application/json\r\nContent-Length: {}\r\n\r\n",
        api.token,
        body.len()
    );
    let r = parse_reply(&raw(addr, &[head.as_bytes(), body].concat()));
    assert_eq!(r.status, 403, "{}", r.text());
    // Nothing was copied, and no VM made.
    assert!(!platform.dir.path().join("web-disk0").exists());
    assert!(list(&api.expect(200, api.get("vms")), "vms").is_empty());
    // From Pendragon's own page it goes through.
    let r = api.expect(
        201,
        api.post(
            "vms",
            &obj([
                ("name", "web".into()),
                ("camelot", true.into()),
                ("ram", "8".into()),
            ]),
        ),
    );
    assert_eq!(s(&r, "disk"), "web-disk0");
    platform.stop();
}
