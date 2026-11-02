//! The protocol mirrors Grail's types: the hardware profile is `grail::MachineConfig`,
//! a VM's device table is Grail's sysinfo table, keys and pixels are Grail's keyboard
//! events and VRAM format. The conversions below destructure Grail's structs without
//! `..`, so adding a field there stops this test compiling until the protocol (and
//! `docs/avalon-protocol.md`) carry it too.

use avalon_proto::{DeviceEntry, HardwareProfile, KeyEvent, Message, PIXEL_FORMAT_XRGB8888};
use grail::config::{DEFAULT_NOMINAL_HZ, MAX_RAM_SIZE, MIB};
use grail::MachineConfig;

fn profile_from(config: &MachineConfig) -> HardwareProfile {
    let MachineConfig {
        ram_size,
        clock_hz,
        nominal_hz,
    } = config.clone();
    HardwareProfile {
        ram_size: ram_size as u64,
        clock_hz,
        nominal_hz,
        devices: Vec::new(),
    }
}

/// What Avalon does with a profile from the wire.
fn config_from(profile: &HardwareProfile) -> Result<MachineConfig, String> {
    let ram_size = usize::try_from(profile.ram_size).map_err(|_| "RAM size too large".to_string())?;
    let config = MachineConfig {
        ram_size,
        clock_hz: profile.clock_hz,
        nominal_hz: profile.nominal_hz,
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
        profile_from(&MachineConfig::default()),
        HardwareProfile::default()
    );
    assert_eq!(avalon_proto::DEFAULT_RAM_SIZE, grail::DEFAULT_RAM_SIZE as u64);
    assert_eq!(avalon_proto::DEFAULT_NOMINAL_HZ, DEFAULT_NOMINAL_HZ);
    assert_eq!(
        HardwareProfile::with_ram(4096),
        profile_from(&MachineConfig::with_ram(4096))
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
        },
        MachineConfig {
            ram_size: 4096,
            clock_hz: Some(u32::MAX),
            nominal_hz: 1,
        },
    ];
    for config in configs {
        let profile = through_the_wire(profile_from(&config));
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
    let config = MachineConfig::with_ram(MIB);
    let table: Vec<DeviceEntry> = grail::bus::device_table(&config)
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
