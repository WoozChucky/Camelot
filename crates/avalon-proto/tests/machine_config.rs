//! The protocol mirrors Grail's types: the hardware profile is `grail::MachineConfig`,
//! a VM's device table is Grail's sysinfo table, keys, pointer buttons and pixels are
//! Grail's keyboard events, pointer buttons and VRAM format. The conversions below
//! destructure Grail's structs without `..`, so adding a field there stops this test
//! compiling until the protocol (and `docs/avalon-protocol.md`) carry it too.
//!
//! Network cards: a Grail config plugs a card into an in-process hub, a profile into
//! one of the server's switches, by name. The two are the same idea (a network the
//! card is on), so here a hub stands for the switch of the same name.

use avalon_proto::{
    DeviceEntry, DeviceSpec, DiskSpec, HardwareProfile, KeyEvent, Message, NetSpec, PIXEL_FORMAT_XRGB8888,
};
use grail::config::{DiskSource, NetSource, DEFAULT_NOMINAL_HZ, MAX_HARTS, MAX_RAM_SIZE, MIB};
use grail::net::{Hub, MacAddr};
use grail::{DiskConfig, MachineConfig, NetConfig};

/// The switches of these tests: hubs with names.
type Switches<'a> = &'a [(&'a str, Hub)];

/// A Grail disk as a protocol device. Memory disks are local to the host that builds
/// the machine and have no wire form.
fn disk_spec_from(disk: &DiskConfig) -> Result<DeviceSpec, String> {
    let DiskConfig { source, read_only } = disk.clone();
    let source = match source {
        DiskSource::File(path) => match path.to_str() {
            Some(path) => avalon_proto::DiskSource::File(path.to_string()),
            None => return Err(format!("{} is not UTF-8", path.display())),
        },
        DiskSource::Memory(_) => return Err("memory disks have no wire form".to_string()),
        DiskSource::Lake { addr, name } => avalon_proto::DiskSource::Lake { addr, name },
    };
    Ok(DeviceSpec::Disk(DiskSpec { source, read_only }))
}

/// A Grail card as a protocol card: on the switch its hub stands for. A capture of one
/// card has no wire form: Avalon captures whole switches (`CaptureSwitch`).
fn net_spec_from(net: &NetConfig, switches: Switches) -> Result<DeviceSpec, String> {
    let NetConfig { source, mac, pcap } = net.clone();
    if pcap.is_some() {
        return Err("a card's capture has no wire form: capture its switch".to_string());
    }
    let hub = match source {
        NetSource::Hub(hub) => hub,
        // User networking is a NAT gateway local to the host that runs the machine,
        // like a memory disk: Avalon's cards are on its switches.
        NetSource::User { .. } => return Err("user networking has no wire form".to_string()),
    };
    let switch = switches
        .iter()
        .find(|(_, h)| *h == hub)
        .map(|(name, _)| name.to_string())
        .ok_or("a hub with no switch name")?;
    Ok(DeviceSpec::Net(NetSpec {
        switch: Some(switch),
        mac: mac.map(|m| m.0),
    }))
}

fn profile_from(config: &MachineConfig, switches: Switches) -> Result<HardwareProfile, String> {
    let MachineConfig {
        ram_size,
        clock_hz,
        nominal_hz,
        harts,
        disks,
        nets,
        paging,
        bios,
    } = config.clone();
    let mut devices: Vec<DeviceSpec> = disks.iter().map(disk_spec_from).collect::<Result<_, _>>()?;
    for net in &nets {
        devices.push(net_spec_from(net, switches)?);
    }
    Ok(HardwareProfile {
        ram_size: ram_size as u64,
        clock_hz,
        nominal_hz,
        harts: harts as u32,
        paging: paging_from(paging),
        bios: bios_from(&bios)?,
        devices,
    })
}

/// Grail's paging modes as the protocol's, every one of them.
fn paging_from(paging: grail::Paging) -> avalon_proto::Paging {
    match paging {
        grail::Paging::Sv48 => avalon_proto::Paging::Sv48,
        grail::Paging::Sv39 => avalon_proto::Paging::Sv39,
    }
}

/// Grail's firmware as the protocol's, every one of them: a file by its path, which
/// must be UTF-8 on the wire.
fn bios_from(bios: &grail::Bios) -> Result<avalon_proto::Bios, String> {
    Ok(match bios {
        grail::Bios::None => avalon_proto::Bios::None,
        grail::Bios::Default => avalon_proto::Bios::Default,
        grail::Bios::File(path) => match path.to_str() {
            Some(path) => avalon_proto::Bios::File(path.to_string()),
            None => return Err(format!("{} is not UTF-8", path.display())),
        },
    })
}

/// The protocol's firmware as Grail's.
fn bios_to(bios: &avalon_proto::Bios) -> grail::Bios {
    match bios {
        avalon_proto::Bios::None => grail::Bios::None,
        avalon_proto::Bios::Default => grail::Bios::Default,
        avalon_proto::Bios::File(path) => grail::Bios::File(path.into()),
    }
}

/// The protocol's paging modes as Grail's.
fn paging_to(paging: avalon_proto::Paging) -> grail::Paging {
    match paging {
        avalon_proto::Paging::Sv48 => grail::Paging::Sv48,
        avalon_proto::Paging::Sv39 => grail::Paging::Sv39,
    }
}

/// What Avalon does with a profile from the wire: every disk, an image file or an image
/// on a Lake server, becomes a config disk, in order; every card a card on its switch
/// (here, the hub of that name).
fn config_from(profile: &HardwareProfile, switches: Switches) -> Result<MachineConfig, String> {
    let ram_size = usize::try_from(profile.ram_size).map_err(|_| "RAM size too large".to_string())?;
    let mut disks = Vec::new();
    let mut nets = Vec::new();
    for device in &profile.devices {
        match device {
            DeviceSpec::Disk(DiskSpec { source, read_only }) => match source {
                avalon_proto::DiskSource::File(path) => {
                    disks.push(DiskConfig::file(path.as_str()).with_read_only(*read_only))
                }
                avalon_proto::DiskSource::Lake { addr, name } => {
                    disks.push(DiskConfig::lake(addr.as_str(), name.as_str()).with_read_only(*read_only))
                }
            },
            DeviceSpec::Net(NetSpec { switch, mac }) => {
                let switch = switch.as_deref().ok_or("Grail has no unplugged cards")?;
                let (_, hub) = switches
                    .iter()
                    .find(|(name, _)| *name == switch)
                    .ok_or_else(|| format!("no switch named '{switch}'"))?;
                let card = NetConfig::hub(hub);
                nets.push(match mac {
                    Some(mac) => card.with_mac(MacAddr(*mac)),
                    None => card,
                });
            }
        }
    }
    let config = MachineConfig {
        ram_size,
        clock_hz: profile.clock_hz,
        nominal_hz: profile.nominal_hz,
        harts: usize::try_from(profile.harts).map_err(|_| "too many harts".to_string())?,
        disks,
        nets,
        paging: paging_to(profile.paging),
        bios: bios_to(&profile.bios),
    };
    config.validate()?;
    Ok(config)
}

fn through_the_wire(profile: HardwareProfile) -> HardwareProfile {
    let msg = Message::CreateVm {
        name: "vm".into(),
        profile,
        program: Vec::new(),
    };
    match avalon_proto::decode(&avalon_proto::encode(&msg).unwrap())
        .unwrap()
        .0
    {
        Message::CreateVm { profile, .. } => profile,
        other => panic!("{other:?}"),
    }
}

#[test]
fn defaults_match_grail() {
    assert_eq!(
        profile_from(&MachineConfig::default(), &[]).unwrap(),
        HardwareProfile::default()
    );
    assert_eq!(avalon_proto::DEFAULT_RAM_SIZE, grail::DEFAULT_RAM_SIZE as u64);
    assert_eq!(avalon_proto::DEFAULT_NOMINAL_HZ, DEFAULT_NOMINAL_HZ);
    assert_eq!(
        avalon_proto::DEFAULT_HARTS as usize,
        MachineConfig::default().harts
    );
    assert_eq!(
        HardwareProfile::with_ram(4096),
        profile_from(&MachineConfig::with_ram(4096), &[]).unwrap()
    );
    // The wire's paging modes the widest one's `satp.MODE` (Sv48 9, Sv39 8).
    for (paging, mode) in [(grail::Paging::Sv48, 9), (grail::Paging::Sv39, 8)] {
        assert_eq!(paging_from(paging).satp_mode(), mode);
        assert_eq!(
            avalon_proto::Paging::from_satp_mode(mode),
            Some(paging_from(paging))
        );
    }
}

#[test]
fn configs_survive_the_wire() {
    let switches = [("lan", Hub::new()), ("dmz", Hub::new())];
    let card = |i: usize| NetConfig::hub(&switches[i].1);
    let configs = [
        MachineConfig::default(),
        MachineConfig::with_ram(MIB),
        // The most RAM it takes, and each paging mode.
        MachineConfig::with_ram(MAX_RAM_SIZE),
        MachineConfig {
            paging: grail::Paging::Sv39,
            harts: 2,
            disks: vec![DiskConfig::lake("127.0.0.1:7401", "os-base")],
            bios: grail::Bios::Default,
            ..MachineConfig::with_ram(64 * MIB)
        },
        // Each firmware.
        MachineConfig {
            bios: grail::Bios::File("firmware/fw_jump.bin".into()),
            ..MachineConfig::with_ram(16 * MIB)
        },
        MachineConfig {
            ram_size: 64 * MIB,
            clock_hz: Some(2_000_000),
            nominal_hz: DEFAULT_NOMINAL_HZ,
            harts: 2,
            disks: Vec::new(),
            nets: Vec::new(),
            paging: grail::Paging::Sv48,
            bios: grail::Bios::None,
        },
        MachineConfig {
            ram_size: 4096,
            clock_hz: Some(u32::MAX),
            nominal_hz: 1,
            harts: MAX_HARTS,
            disks: Vec::new(),
            nets: Vec::new(),
            paging: grail::Paging::Sv48,
            bios: grail::Bios::None,
        },
        MachineConfig {
            disks: vec![
                DiskConfig::file("disks/root.img"),
                DiskConfig::lake("127.0.0.1:7401", "shared"),
                DiskConfig::file("scratch.img").with_read_only(true),
                DiskConfig::lake("[::1]:7500", "base.img").with_read_only(true),
            ],
            ..MachineConfig::with_ram(MIB)
        },
        MachineConfig {
            disks: vec![DiskConfig::file("root.img")],
            nets: vec![
                card(0),
                card(1).with_mac(MacAddr([0x02, 0, 0, 0, 0, 0x42])),
                card(0).with_mac(MacAddr([0x0a, 0xbc, 0xde, 0xf0, 0x12, 0x34])),
                card(1),
            ],
            ..MachineConfig::with_ram(MIB)
        },
    ];
    for config in configs {
        let profile = through_the_wire(profile_from(&config, &switches).unwrap());
        assert_eq!(profile.cpu_hz(), config.cpu_hz());
        assert_eq!(config_from(&profile, &switches).unwrap(), config);
    }
    // What has no wire form.
    let captured = MachineConfig {
        nets: vec![card(0).with_pcap("a.pcap")],
        ..MachineConfig::default()
    };
    assert!(profile_from(&captured, &switches).is_err());
    let user = MachineConfig {
        nets: vec![NetConfig::user(Vec::new())],
        ..MachineConfig::default()
    };
    assert!(profile_from(&user, &switches).is_err());
    let unplugged = HardwareProfile {
        devices: vec![DeviceSpec::Net(NetSpec {
            switch: None,
            mac: None,
        })],
        ..HardwareProfile::default()
    };
    assert_eq!(through_the_wire(unplugged.clone()), unplugged);
    assert!(config_from(&unplugged, &switches).is_err());
}

#[test]
fn invalid_profiles_decode_and_are_left_to_the_server() {
    // The wire carries any values; Grail's validation decides (Avalon answers
    // `InvalidProfile`).
    for bad in [
        HardwareProfile::with_ram(0),
        HardwareProfile::with_ram(4095),
        HardwareProfile::with_ram(MAX_RAM_SIZE as u64 + 4096),
        HardwareProfile::with_ram(u64::MAX),
        HardwareProfile {
            clock_hz: Some(0),
            ..HardwareProfile::default()
        },
        HardwareProfile {
            nominal_hz: 0,
            ..HardwareProfile::default()
        },
        HardwareProfile {
            harts: 0,
            ..HardwareProfile::default()
        },
        HardwareProfile {
            harts: MAX_HARTS as u32 + 1,
            ..HardwareProfile::default()
        },
    ] {
        let back = through_the_wire(bad.clone());
        assert_eq!(back, bad);
        assert!(config_from(&back, &[]).is_err(), "{bad:?}");
    }
}

#[test]
fn device_table_mirrors_sysinfo() {
    // Memory disks have no wire form, but the guest sees them like any other disk.
    let config = MachineConfig {
        disks: vec![
            DiskConfig::memory(vec![0; 512]),
            DiskConfig::memory(vec![0; 1024]).with_read_only(true),
        ],
        ..MachineConfig::with_ram(MIB)
    };
    let table: Vec<DeviceEntry> = grail::bus::device_table(&config, config.disks.len())
        .into_iter()
        .map(|e| {
            let grail::sysinfo::DeviceEntry {
                kind,
                base,
                size,
                irq,
            } = e;
            DeviceEntry {
                kind: kind as u32,
                base: u32::try_from(base).expect("devices are below 4 GiB"),
                size: u32::try_from(size).expect("devices are below 4 GiB"),
                irq,
            }
        })
        .collect();
    // What a running guest reads from its own sysinfo block.
    let mut machine = grail::Machine::new(config).unwrap();
    let read = |m: &mut grail::Machine, off: u32, len: u32| {
        m.cpu
            .bus
            .read(grail::bus::SYSINFO_BASE + u64::from(off), len)
            .unwrap()
    };
    let count = read(&mut machine, grail::sysinfo::REG_DEV_COUNT, 4);
    assert_eq!(count as usize, table.len());
    // Each entry: its type and PLIC source (4 bytes each), then its base and
    // size (8 bytes each).
    use grail::sysinfo::{DEV_BASE, DEV_IRQ, DEV_SIZE, DEV_TYPE};
    for (i, entry) in table.iter().enumerate() {
        let at = grail::sysinfo::DEV_TABLE + i as u32 * grail::sysinfo::DEV_ENTRY_SIZE;
        let fields = [
            read(&mut machine, at + DEV_TYPE, 4),
            read(&mut machine, at + DEV_BASE, 8),
            read(&mut machine, at + DEV_SIZE, 8),
            read(&mut machine, at + DEV_IRQ, 4),
        ];
        assert_eq!(
            fields,
            [entry.kind, entry.base, entry.size, entry.irq].map(u64::from)
        );
    }
}

#[test]
fn keys_and_pixels_mirror_grail() {
    // Field for field: a protocol key is Grail's key event, both forms of the key.
    let grail::keyboard::KeyEvent {
        code,
        usage,
        release,
        repeat,
        shift,
        ctrl,
    } = grail::keyboard::KeyEvent::release(b'q')
        .with_usage(grail::keyboard::usage::A + 16)
        .with_ctrl(true);
    let key = KeyEvent {
        code,
        release,
        shift,
        ctrl,
        repeat,
        usage,
    };
    assert!(grail::keyboard::code::is_valid(key.code));
    assert!(grail::keyboard::usage::is_valid(key.usage));
    assert!(Message::KeyInput { vm: 1, key }.check().is_ok());
    assert_eq!(PIXEL_FORMAT_XRGB8888, grail::framebuffer::FB_FORMAT_XRGB8888);
}

#[test]
fn pointer_buttons_are_grails() {
    use avalon_proto::{POINTER_BUTTONS, POINTER_LEFT, POINTER_MIDDLE, POINTER_RIGHT};
    use grail::pointer::{BUTTONS_MASK, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT};
    assert_eq!(
        [POINTER_LEFT, POINTER_RIGHT, POINTER_MIDDLE, POINTER_BUTTONS],
        [BUTTON_LEFT, BUTTON_RIGHT, BUTTON_MIDDLE, BUTTONS_MASK]
    );
}
