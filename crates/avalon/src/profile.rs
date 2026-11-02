//! A hardware profile from the wire as a Grail machine: `HardwareProfile` mirrors
//! `grail::MachineConfig` field for field (protocol doc, section 3), and Grail's own
//! validation decides what is a machine it can build.

use avalon_proto::{DeviceSpec, DiskSource, DiskSpec, HardwareProfile};
use grail::{DiskConfig, MachineConfig};

/// The machine `profile` describes, validated as Grail validates it. An error is the
/// text of an `InvalidProfile` reply.
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
        let DeviceSpec::Disk(DiskSpec { source, read_only }) = device;
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
        // Profiles have no network cards yet (issue #81).
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
