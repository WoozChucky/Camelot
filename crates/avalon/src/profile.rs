//! A hardware profile from the wire as a Grail machine: `HardwareProfile` mirrors
//! `grail::MachineConfig` field for field (protocol doc, section 3), and Grail's own
//! validation decides what is a machine it can build.
//!
//! Network cards are the exception: Grail's configured cards plug into an in-process
//! hub, while a profile's cards plug into Avalon's switches, which only the server
//! has. So [`machine_config`] leaves them out, [`card_configs`] reads them, and the
//! server builds each card's backend itself (`grail::Machine::with_devices`).

use crate::switch::generated_mac;
use avalon_proto::{DeviceSpec, DiskSource, DiskSpec, HardwareProfile, NetSpec};
use grail::bus::MAX_NETS;
use grail::net::MacAddr;
use grail::{DiskConfig, MachineConfig, Xlen};

/// One network card of a profile, as the server builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardConfig {
    /// The switch to plug it into when the VM is created, if any.
    pub switch: Option<String>,
    /// The profile's address, or the one Avalon makes up for it ([`generated_mac`]).
    pub mac: MacAddr,
}

/// The network cards of the VM named `vm` with `profile`, in order, each with its MAC
/// address. An error is the text of an `InvalidProfile` reply: more than four cards,
/// or an address that is not a card's (a group address, or one not locally
/// administered). Whether the switches exist is the server's to check.
pub fn card_configs(vm: &str, profile: &HardwareProfile) -> Result<Vec<CardConfig>, String> {
    let mut cards = Vec::new();
    for device in &profile.devices {
        let DeviceSpec::Net(NetSpec { switch, mac }) = device else {
            continue;
        };
        let i = cards.len();
        if i == MAX_NETS {
            return Err(format!("more than {MAX_NETS} network cards"));
        }
        let mac = match mac {
            Some(mac) => {
                let mac = MacAddr(*mac);
                mac.check_card().map_err(|e| format!("network card {i}: {e}"))?;
                mac
            }
            None => generated_mac(vm, i as u8),
        };
        cards.push(CardConfig {
            switch: switch.clone(),
            mac,
        });
    }
    Ok(cards)
}

/// The machine `profile` describes, validated as Grail validates it, without its
/// network cards ([`card_configs`]). An error is the text of an `InvalidProfile` reply.
///
/// Disks are opened when the machine is built (`Machine::new`), in the order the
/// profile lists them: image files on Avalon's host, relative to Avalon's working
/// directory, and images on a Lake server through Grail's network backend
/// (`grail::lake::LakeDisk`), which attaches the image for as long as the VM's machine
/// exists. Only the disk names are checked here; whether an image exists and can be
/// attached is Lake's to say, when the machine is built.
///
/// The VM is as wide as the profile says (protocol doc, section 3, "The width"),
/// unless its `program` is ELF64: that makes an RV32 VM RV64, as `Machine::load_elf`
/// will when it loads it, so the RAM is checked against the width the VM will have
/// (Grail's `max_ram_size`: 1 TiB, 2 GiB on RV32). An ELF32 program on an RV64 VM is
/// left for `load_elf` to refuse (`InvalidProgram`). A VM that boots from disk (no
/// program) has the profile's width, and its boot ROM with it.
///
/// Firmware (`bios`) is checked here too, as the hardware it is: an RV32 VM (an RV32
/// profile whose program, if any, is not ELF64) has none, and the firmware must be
/// one Grail can load (`grail::firmware::Image::of`: a file that can be read, an ELF
/// of RV64 or a raw binary, that fits in the firmware's 2 MiB). A file is on Avalon's
/// host, relative to its working directory, as a disk image's; and as a disk image
/// that cannot be opened, firmware that cannot be loaded is `InvalidProfile`.
pub fn machine_config(profile: &HardwareProfile, program: &[u8]) -> Result<MachineConfig, String> {
    let HardwareProfile {
        ram_size,
        clock_hz,
        nominal_hz,
        harts,
        xlen,
        paging,
        bios,
        devices,
    } = profile;
    let ram_size = usize::try_from(*ram_size)
        .map_err(|_| format!("RAM size {ram_size:#x} is too large for this host"))?;
    // Out of range on any host: Grail's validation below says so.
    let harts = usize::try_from(*harts).unwrap_or(usize::MAX);
    let mut disks = Vec::new();
    for device in devices {
        let DeviceSpec::Disk(DiskSpec { source, read_only }) = device else {
            continue;
        };
        match source {
            DiskSource::File(path) => disks.push(DiskConfig::file(path).with_read_only(*read_only)),
            DiskSource::Lake { addr, name } => {
                disks.push(DiskConfig::lake(addr.as_str(), name.as_str()).with_read_only(*read_only))
            }
        }
    }
    let config = MachineConfig {
        ram_size,
        clock_hz: *clock_hz,
        nominal_hz: *nominal_hz,
        harts,
        disks,
        // The server makes the cards (`card_configs`).
        nets: Vec::new(),
        xlen: match (grail::elf::class(program), xlen) {
            // What loading it will make the machine.
            (Some(Xlen::Rv64), _) => Xlen::Rv64,
            (_, avalon_proto::Xlen::Rv32) => Xlen::Rv32,
            (_, avalon_proto::Xlen::Rv64) => Xlen::Rv64,
        },
        paging: match paging {
            avalon_proto::Paging::Sv48 => grail::Paging::Sv48,
            avalon_proto::Paging::Sv39 => grail::Paging::Sv39,
        },
        bios: machine_bios(bios),
    };
    config.validate()?;
    if config.bios.is_some() {
        if config.xlen == Xlen::Rv32 {
            return Err("firmware (bios) is for RV64 VMs only: an RV32 VM, which an RV32 profile is                         unless its program is ELF64, boots its program or disk in M-mode (bios none)"
                .into());
        }
        // Loaded and dropped: the machine loads it again when it is built.
        grail::firmware::Image::of(&config.bios, Xlen::Rv64)?;
    }
    Ok(config)
}

/// Grail's firmware for a profile's `bios`: a file on Avalon's host, relative to its
/// working directory, as a disk image's.
pub fn machine_bios(bios: &avalon_proto::Bios) -> grail::Bios {
    match bios {
        avalon_proto::Bios::None => grail::Bios::None,
        avalon_proto::Bios::Default => grail::Bios::Default,
        avalon_proto::Bios::File(path) => grail::Bios::File(path.into()),
    }
}

/// The protocol's name for Grail's width `xlen`: what a VM's profile says once its
/// machine is built (an ELF64 program has made it RV64).
pub fn profile_xlen(xlen: Xlen) -> avalon_proto::Xlen {
    match xlen {
        Xlen::Rv32 => avalon_proto::Xlen::Rv32,
        Xlen::Rv64 => avalon_proto::Xlen::Rv64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grail::config::MIB;

    #[test]
    fn valid_profiles_become_their_machine() {
        let profile = HardwareProfile {
            ram_size: 64 * MIB as u64,
            clock_hz: Some(2_000_000),
            nominal_hz: 5,
            harts: 4,
            xlen: avalon_proto::Xlen::Rv64,
            paging: avalon_proto::Paging::Sv39,
            bios: avalon_proto::Bios::Default,
            devices: vec![
                DeviceSpec::Disk(DiskSpec {
                    source: DiskSource::File("a.img".into()),
                    read_only: false,
                }),
                DeviceSpec::Disk(DiskSpec {
                    source: DiskSource::Lake {
                        addr: "127.0.0.1:7401".into(),
                        name: "root".into(),
                    },
                    read_only: false,
                }),
                DeviceSpec::Disk(DiskSpec {
                    source: DiskSource::File("b.img".into()),
                    read_only: true,
                }),
            ],
        };
        assert_eq!(
            machine_config(&profile, &[]).unwrap(),
            MachineConfig {
                ram_size: 64 * MIB,
                clock_hz: Some(2_000_000),
                nominal_hz: 5,
                harts: 4,
                disks: vec![
                    DiskConfig::file("a.img"),
                    DiskConfig::lake("127.0.0.1:7401", "root"),
                    DiskConfig::file("b.img").with_read_only(true)
                ],
                nets: Vec::new(),
                xlen: Xlen::Rv64,
                paging: grail::Paging::Sv39,
                bios: grail::Bios::Default,
            }
        );
        assert_eq!(
            machine_config(&HardwareProfile::default(), &[]).unwrap(),
            MachineConfig::default()
        );
    }

    fn net(switch: Option<&str>, mac: Option<[u8; 6]>) -> DeviceSpec {
        DeviceSpec::Net(NetSpec {
            switch: switch.map(str::to_string),
            mac,
        })
    }

    #[test]
    fn cards_get_their_addresses() {
        let profile = HardwareProfile {
            devices: vec![
                net(Some("lan"), None),
                DeviceSpec::Disk(DiskSpec {
                    source: DiskSource::File("a.img".into()),
                    read_only: false,
                }),
                net(None, Some([0x02, 0, 0, 0, 0, 0x42])),
                net(Some("dmz"), None),
            ],
            ..HardwareProfile::default()
        };
        let cards = card_configs("web", &profile).unwrap();
        assert_eq!(
            cards,
            [
                CardConfig {
                    switch: Some("lan".into()),
                    mac: generated_mac("web", 0)
                },
                CardConfig {
                    switch: None,
                    mac: MacAddr([0x02, 0, 0, 0, 0, 0x42])
                },
                CardConfig {
                    switch: Some("dmz".into()),
                    mac: generated_mac("web", 2)
                },
            ]
        );
        // The disks are the machine's; the cards are not in its config.
        let config = machine_config(&profile, &[]).unwrap();
        assert_eq!(config.disks, [DiskConfig::file("a.img")]);
        assert!(config.nets.is_empty());
        assert_eq!(card_configs("web", &HardwareProfile::default()), Ok(Vec::new()));
    }

    #[test]
    fn bad_cards_are_refused() {
        for (devices, why) in [
            (vec![net(None, None); 5], "more than 4 network cards"),
            (
                vec![net(None, None), net(Some("lan"), Some([0x01, 0, 0x5e, 0, 0, 1]))],
                "network card 1: 01:00:5e:00:00:01 is a group address",
            ),
            (
                vec![net(None, Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]))],
                "network card 0: 00:11:22:33:44:55 is not locally administered",
            ),
        ] {
            let profile = HardwareProfile {
                devices,
                ..HardwareProfile::default()
            };
            let err = card_configs("vm", &profile).unwrap_err();
            assert!(err.starts_with(why), "{err}");
        }
        let four = HardwareProfile {
            devices: vec![net(None, None); 4],
            ..HardwareProfile::default()
        };
        assert_eq!(card_configs("vm", &four).unwrap().len(), 4);
    }

    #[test]
    fn a_vm_is_as_wide_as_its_program() {
        // An ELF64 program makes the VM RV64, which takes more than RV32's 2 GiB; only
        // the header's class is read here.
        let big = HardwareProfile::with_ram(16 << 30);
        let elf64 = b"\x7fELF\x02\x01\x01";
        let config = machine_config(&big, elf64).unwrap();
        assert_eq!((config.xlen, config.ram_size), (Xlen::Rv64, 16 << 30));
        for program in [&b"\x7fELF\x01\x01\x01"[..], &[]] {
            let e = machine_config(&big, program).unwrap_err();
            assert!(e.contains("an RV32 machine has at most 2 GiB"), "{e}");
        }
        assert_eq!(
            machine_config(&HardwareProfile::default(), &[]).unwrap().xlen,
            Xlen::Rv32
        );
    }

    #[test]
    fn a_profile_sets_the_width_and_the_paging() {
        let rv64 = HardwareProfile {
            xlen: avalon_proto::Xlen::Rv64,
            paging: avalon_proto::Paging::Sv39,
            ..HardwareProfile::with_ram(16 << 30)
        };
        // Booting from disk, or with an ELF64 program: RV64 with Sv39 alone.
        for program in [&[][..], b"\x7fELF\x02\x01\x01"] {
            let config = machine_config(&rv64, program).unwrap();
            assert_eq!(
                (config.xlen, config.paging, config.ram_size),
                (Xlen::Rv64, grail::Paging::Sv39, 16 << 30)
            );
        }
        // An ELF32 program stays on an RV64 machine, which refuses it when it loads it.
        assert_eq!(
            machine_config(&rv64, b"\x7fELF\x01\x01\x01").unwrap().xlen,
            Xlen::Rv64
        );
        // Past RV64's 1 TiB.
        let huge = HardwareProfile {
            ram_size: (1 << 40) + 4096,
            ..rv64
        };
        let e = machine_config(&huge, &[]).unwrap_err();
        assert!(e.contains("1 TiB"), "{e}");
        assert_eq!(profile_xlen(Xlen::Rv64), avalon_proto::Xlen::Rv64);
        assert_eq!(profile_xlen(Xlen::Rv32), avalon_proto::Xlen::Rv32);
    }

    #[test]
    fn invalid_profiles_are_refused() {
        let disk = |source| {
            DeviceSpec::Disk(DiskSpec {
                source,
                read_only: false,
            })
        };
        for (profile, why) in [
            (HardwareProfile::with_ram(0), "multiple of 4 KiB"),
            (HardwareProfile::with_ram(4097), "multiple of 4 KiB"),
            (HardwareProfile::with_ram(4 << 30), "2 GiB"),
            (HardwareProfile::with_ram(u64::MAX), ""),
            (
                HardwareProfile {
                    clock_hz: Some(0),
                    ..HardwareProfile::default()
                },
                "0 Hz",
            ),
            (
                HardwareProfile {
                    nominal_hz: 0,
                    ..HardwareProfile::default()
                },
                "0 Hz",
            ),
            (
                HardwareProfile {
                    harts: 0,
                    ..HardwareProfile::default()
                },
                "0 harts, must be 1 to 8",
            ),
            (
                HardwareProfile {
                    harts: 9,
                    ..HardwareProfile::default()
                },
                "9 harts, must be 1 to 8",
            ),
            (
                HardwareProfile {
                    harts: u32::MAX,
                    ..HardwareProfile::default()
                },
                "harts, must be 1 to 8",
            ),
            (
                HardwareProfile {
                    devices: vec![disk(DiskSource::File("d.img".into())); 9],
                    ..HardwareProfile::default()
                },
                "at most 8",
            ),
            (
                HardwareProfile {
                    devices: vec![disk(DiskSource::Lake {
                        addr: "127.0.0.1:7401".into(),
                        name: "Root".into(),
                    })],
                    ..HardwareProfile::default()
                },
                "lake://127.0.0.1:7401/Root",
            ),
        ] {
            let err = machine_config(&profile, &[]).unwrap_err();
            assert!(err.contains(why), "{profile:?}: {err}");
        }
    }
}
