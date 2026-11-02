//! The protocol mirrors Grail's types: the hardware profile is `grail::MachineConfig`,
//! a VM's device table is Grail's sysinfo table, keys and pixels are Grail's keyboard
//! events and VRAM format. The conversions below destructure Grail's structs without
//! `..`, so adding a field there stops this test compiling until the protocol (and
//! `docs/avalon-protocol.md`) carry it too.

use avalon_proto::{
    DeviceEntry, DeviceSpec, DiskSpec, HardwareProfile, KeyEvent, Message, PIXEL_FORMAT_XRGB8888,
};
use grail::config::{DiskSource, DEFAULT_NOMINAL_HZ, MAX_RAM_SIZE, MIB};
use grail::{DiskConfig, MachineConfig};

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
    };
    Ok(DeviceSpec::Disk(DiskSpec { source, read_only }))
}

fn profile_from(config: &MachineConfig) -> Result<HardwareProfile, String> {
    let MachineConfig {
        ram_size,
        clock_hz,
        nominal_hz,
        disks,
    } = config.clone();
    Ok(HardwareProfile {
        ram_size: ram_size as u64,
        clock_hz,
        nominal_hz,
        devices: disks.iter().map(disk_spec_from).collect::<Result<_, _>>()?,
    })
}

/// What Avalon does with a profile from the wire. Disks on a Lake server become
/// network backends handed to `Machine::with_backends`, not config disks, so this
/// mirror of the config refuses them.
fn config_from(profile: &HardwareProfile) -> Result<MachineConfig, String> {
    let ram_size = usize::try_from(profile.ram_size).map_err(|_| "RAM size too large".to_string())?;
    let mut disks = Vec::new();
    for device in &profile.devices {
        let DeviceSpec::Disk(DiskSpec { source, read_only }) = device;
        match source {
            avalon_proto::DiskSource::File(path) => {
                disks.push(DiskConfig::file(path.as_str()).with_read_only(*read_only))
            }
            avalon_proto::DiskSource::Lake { .. } => {
                return Err("Lake disks are network backends, not config disks".to_string())
            }
        }
    }
    let config = MachineConfig {
        ram_size,
        clock_hz: profile.clock_hz,
        nominal_hz: profile.nominal_hz,
        disks,
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
        profile_from(&MachineConfig::default()).unwrap(),
        HardwareProfile::default()
    );
    assert_eq!(avalon_proto::DEFAULT_RAM_SIZE, grail::DEFAULT_RAM_SIZE as u64);
    assert_eq!(avalon_proto::DEFAULT_NOMINAL_HZ, DEFAULT_NOMINAL_HZ);
    assert_eq!(
        HardwareProfile::with_ram(4096),
        profile_from(&MachineConfig::with_ram(4096)).unwrap()
    );
}

#[test]
fn configs_survive_the_wire() {
    let configs = [
        MachineConfig::default(),
        MachineConfig::with_ram(MIB),
        MachineConfig::with_ram(MAX_RAM_SIZE),
        MachineConfig {
            ram_size: 64 * MIB,
            clock_hz: Some(2_000_000),
            nominal_hz: DEFAULT_NOMINAL_HZ,
            disks: Vec::new(),
        },
        MachineConfig {
            ram_size: 4096,
            clock_hz: Some(u32::MAX),
            nominal_hz: 1,
            disks: Vec::new(),
        },
        MachineConfig {
            disks: vec![
                DiskConfig::file("disks/root.img"),
                DiskConfig::file("scratch.img").with_read_only(true),
            ],
            ..MachineConfig::with_ram(MIB)
        },
    ];
    for config in configs {
        let profile = through_the_wire(profile_from(&config).unwrap());
        assert_eq!(profile.cpu_hz(), config.cpu_hz());
        assert_eq!(config_from(&profile).unwrap(), config);
    }
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
    ] {
        let back = through_the_wire(bad.clone());
        assert_eq!(back, bad);
        assert!(config_from(&back).is_err(), "{bad:?}");
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
                base,
                size,
                irq,
            }
        })
        .collect();
    // What a running guest reads from its own sysinfo block.
    let mut machine = grail::Machine::new(config).unwrap();
    let read = |m: &mut grail::Machine, off: u32| m.cpu.bus.read(grail::bus::SYSINFO_BASE + off, 4).unwrap();
    let count = read(&mut machine, grail::sysinfo::REG_DEV_COUNT);
    assert_eq!(count as usize, table.len());
    for (i, entry) in table.iter().enumerate() {
        let at = grail::sysinfo::DEV_TABLE + i as u32 * grail::sysinfo::DEV_ENTRY_SIZE;
        let words: Vec<u32> = (0..4).map(|w| read(&mut machine, at + 4 * w)).collect();
        assert_eq!(words, [entry.kind, entry.base, entry.size, entry.irq]);
    }
}

#[test]
fn keys_and_pixels_mirror_grail() {
    let grail::keyboard::KeyEvent {
        code,
        release,
        shift,
        ctrl,
    } = grail::keyboard::KeyEvent {
        code: b'q',
        release: true,
        shift: false,
        ctrl: true,
    };
    let key = KeyEvent {
        code,
        release,
        shift,
        ctrl,
    };
    assert!(grail::keyboard::code::is_valid(key.code));
    assert_eq!(PIXEL_FORMAT_XRGB8888, grail::framebuffer::FB_FORMAT_XRGB8888);
}
