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
use grail::{DiskConfig, MachineConfig};

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
pub fn machine_config(profile: &HardwareProfile) -> Result<MachineConfig, String> {
    let HardwareProfile {
        ram_size,
        clock_hz,
        nominal_hz,
        devices,
    } = profile;
    let ram_size = usize::try_from(*ram_size)
        .map_err(|_| format!("RAM size {ram_size:#x} is too large for this host"))?;
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
        disks,
        // The server makes the cards (`card_configs`).
        nets: Vec::new(),
    };
    config.validate()?;
    Ok(config)
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
            machine_config(&profile).unwrap(),
            MachineConfig {
                ram_size: 64 * MIB,
                clock_hz: Some(2_000_000),
                nominal_hz: 5,
                disks: vec![
                    DiskConfig::file("a.img"),
                    DiskConfig::lake("127.0.0.1:7401", "root"),
                    DiskConfig::file("b.img").with_read_only(true)
                ],
                nets: Vec::new(),
            }
        );
        assert_eq!(
            machine_config(&HardwareProfile::default()).unwrap(),
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
        let config = machine_config(&profile).unwrap();
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
            let err = machine_config(&profile).unwrap_err();
            assert!(err.contains(why), "{profile:?}: {err}");
        }
    }
}
