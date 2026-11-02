//! Pins the crate to `docs/avalon-protocol.md`: the type codes, kinds and error codes
//! are copied from the doc's tables, and every message type has a byte-exact layout
//! built field by field from the doc's payload tables. Round-trip tests alone would
//! not notice a code renumbered or two fields swapped on both sides.

use avalon_proto::*;
use std::collections::BTreeSet;
use MessageKind::{Event, Reply, Request};

/// Section 6, the message catalogue: (code, name, kind).
const SPEC_CATALOGUE: [(u16, &str, MessageKind); 51] = [
    (0x0001, "Hello", Request),
    (0x0002, "Welcome", Reply),
    (0x0003, "Error", Reply),
    (0x0004, "Ok", Reply),
    (0x0005, "Ping", Request),
    (0x0006, "Pong", Reply),
    (0x0101, "CreateVm", Request),
    (0x0102, "VmCreated", Reply),
    (0x0103, "ListVms", Request),
    (0x0104, "VmList", Reply),
    (0x0105, "GetVmInfo", Request),
    (0x0106, "VmInfo", Reply),
    (0x0107, "StartVm", Request),
    (0x0108, "PauseVm", Request),
    (0x0109, "ResumeVm", Request),
    (0x010A, "StopVm", Request),
    (0x010B, "DestroyVm", Request),
    (0x010C, "VmStateChanged", Event),
    (0x010D, "Detached", Event),
    (0x0201, "ConsoleAttach", Request),
    (0x0202, "ConsoleAttached", Reply),
    (0x0203, "ConsoleInput", Event),
    (0x0204, "ConsoleOutput", Event),
    (0x0205, "ConsoleDetach", Request),
    (0x0301, "ScreenAttach", Request),
    (0x0302, "ScreenAttached", Reply),
    (0x0303, "ScreenFrame", Event),
    (0x0304, "KeyInput", Event),
    (0x0305, "ScreenDetach", Request),
    (0x0306, "PointerInput", Event),
    (0x0401, "BlockOpen", Request),
    (0x0402, "BlockOpened", Reply),
    (0x0403, "BlockRead", Request),
    (0x0404, "BlockData", Reply),
    (0x0405, "BlockWrite", Request),
    (0x0406, "BlockFlush", Request),
    (0x0407, "BlockSize", Request),
    (0x0408, "BlockCapacity", Reply),
    (0x0409, "BlockClose", Request),
    (0x0501, "DiskCreate", Request),
    (0x0502, "ListDisks", Request),
    (0x0503, "DiskList", Reply),
    (0x0504, "DiskDelete", Request),
    (0x0601, "CreateSwitch", Request),
    (0x0602, "DeleteSwitch", Request),
    (0x0603, "ListSwitches", Request),
    (0x0604, "SwitchList", Reply),
    (0x0605, "AttachCard", Request),
    (0x0606, "DetachCard", Request),
    (0x0607, "CaptureSwitch", Request),
    (0x0608, "SetSwitchNat", Request),
];

/// Section 5, the error codes.
const SPEC_ERROR_CODES: [(ErrorCode, u16); 20] = [
    (ErrorCode::Malformed, 1),
    (ErrorCode::UnknownMessage, 2),
    (ErrorCode::Unexpected, 3),
    (ErrorCode::Unsupported, 4),
    (ErrorCode::NoSuchVm, 5),
    (ErrorCode::NoSuchDisk, 6),
    (ErrorCode::NoSuchHandle, 7),
    (ErrorCode::AlreadyExists, 8),
    (ErrorCode::InvalidName, 9),
    (ErrorCode::InvalidProfile, 10),
    (ErrorCode::InvalidProgram, 11),
    (ErrorCode::InvalidState, 12),
    (ErrorCode::OverBudget, 13),
    (ErrorCode::Busy, 14),
    (ErrorCode::ReadOnly, 15),
    (ErrorCode::OutOfRange, 16),
    (ErrorCode::Io, 17),
    (ErrorCode::Internal, 18),
    (ErrorCode::NoSuchSwitch, 19),
    (ErrorCode::NoSuchCard, 20),
];

#[test]
fn catalogue_matches_the_doc() {
    assert_eq!(CATALOGUE, &SPEC_CATALOGUE[..]);
    for (code, name, kind) in SPEC_CATALOGUE {
        assert_eq!(kind_of(code), Some(kind), "{name}");
        assert_eq!(type_name(code), name);
    }
}

#[test]
fn error_codes_match_the_doc() {
    for (code, n) in SPEC_ERROR_CODES {
        assert_eq!(code.to_u16(), n, "{code:?}");
        assert_eq!(ErrorCode::from_u16(n), code, "{n}");
    }
    assert_eq!(
        ErrorCode::all().collect::<Vec<_>>(),
        SPEC_ERROR_CODES.map(|(c, _)| c)
    );
}

/// A payload written field by field, as the doc's tables list them.
#[derive(Default)]
struct P(Vec<u8>);

impl P {
    fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    fn u16(mut self, v: u16) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn i8(mut self, v: i8) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn i32(mut self, v: i32) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn raw(mut self, v: &[u8]) -> Self {
        self.0.extend(v);
        self
    }
    fn bytes(self, v: &[u8]) -> Self {
        self.u32(v.len() as u32).raw(v)
    }
    fn str(self, v: &str) -> Self {
        self.bytes(v.as_bytes())
    }
    /// The whole frame: `length` (type + payload), `type`, payload.
    fn frame(self, ty: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend((2 + self.0.len() as u32).to_le_bytes());
        out.extend(ty.to_le_bytes());
        out.extend(self.0);
        out
    }
}

fn simple_profile() -> HardwareProfile {
    HardwareProfile {
        ram_size: 0x1000,
        clock_hz: None,
        nominal_hz: 10,
        harts: 1,
        devices: Vec::new(),
    }
}

/// `simple_profile` per the HardwareProfile table.
fn simple_profile_bytes(p: P) -> P {
    p.u64(0x1000).u8(0).u32(10).u32(1).u32(0)
}

/// One message of every type, each field a different value, with its layout per the
/// doc.
fn layouts() -> Vec<(Message, Vec<u8>)> {
    use Message::*;
    let vm = |id: u32, name: &str, state: VmState| VmSummary {
        id,
        name: name.into(),
        state,
        profile: simple_profile(),
    };
    vec![
        (
            Hello {
                version: 0x0102,
                agent: "rt".into(),
            },
            P::default().raw(b"AVLN").u16(0x0102).str("rt").frame(0x0001),
        ),
        (
            Welcome {
                version: 0x0304,
                server: "av".into(),
            },
            P::default().u16(0x0304).str("av").frame(0x0002),
        ),
        (
            Error {
                code: ErrorCode::Busy,
                message: "m".into(),
            },
            P::default().u16(14).str("m").frame(0x0003),
        ),
        (Ok, P::default().frame(0x0004)),
        (
            Ping { token: 0x0102_0304 },
            P::default().u32(0x0102_0304).frame(0x0005),
        ),
        (
            Pong { token: 0x0506_0708 },
            P::default().u32(0x0506_0708).frame(0x0006),
        ),
        (
            CreateVm {
                name: "a".into(),
                profile: HardwareProfile {
                    ram_size: 0x2000,
                    clock_hz: Some(5),
                    nominal_hz: 7,
                    harts: 3,
                    devices: vec![
                        DeviceSpec::Disk(DiskSpec {
                            source: DiskSource::File("f".into()),
                            read_only: true,
                        }),
                        DeviceSpec::Disk(DiskSpec {
                            source: DiskSource::Lake {
                                addr: "h:1".into(),
                                name: "d".into(),
                            },
                            read_only: false,
                        }),
                        DeviceSpec::Net(NetSpec {
                            switch: Some("s".into()),
                            mac: Some([2, 3, 4, 5, 6, 7]),
                        }),
                        DeviceSpec::Net(NetSpec {
                            switch: None,
                            mac: None,
                        }),
                    ],
                },
                program: vec![0xee],
            },
            P::default()
                .str("a")
                .u64(0x2000)
                .u8(1)
                .u32(5)
                .u32(7)
                .u32(3)
                .u32(4)
                .u8(1)
                .u8(1)
                .str("f")
                .u8(1)
                .u8(1)
                .u8(2)
                .str("h:1")
                .str("d")
                .u8(0)
                .u8(2)
                .u8(1)
                .str("s")
                .u8(1)
                .raw(&[2, 3, 4, 5, 6, 7])
                .u8(2)
                .u8(0)
                .u8(0)
                .bytes(&[0xee])
                .frame(0x0101),
        ),
        (VmCreated { id: 9 }, P::default().u32(9).frame(0x0102)),
        (ListVms, P::default().frame(0x0103)),
        (
            VmList {
                vms: vec![vm(3, "v", VmState::Paused)],
            },
            simple_profile_bytes(P::default().u32(1).u32(3).str("v").u8(2)).frame(0x0104),
        ),
        (
            GetVmInfo {
                vm: VmRef::Name("v".into()),
            },
            P::default().u8(1).str("v").frame(0x0105),
        ),
        (
            VmInfo(VmDetails {
                vm: vm(3, "v", VmState::Exited(4)),
                devices: vec![DeviceEntry {
                    kind: 1,
                    base: 2,
                    size: 3,
                    irq: 4,
                }],
                steps: 0x0102_0304_0506_0708,
            }),
            simple_profile_bytes(P::default().u32(3).str("v").u8(4).u32(4))
                .u32(1)
                .u32(1)
                .u32(2)
                .u32(3)
                .u32(4)
                .u64(0x0102_0304_0506_0708)
                .frame(0x0106),
        ),
        (
            StartVm { vm: VmRef::Id(5) },
            P::default().u8(0).u32(5).frame(0x0107),
        ),
        (
            PauseVm { vm: VmRef::Id(6) },
            P::default().u8(0).u32(6).frame(0x0108),
        ),
        (
            ResumeVm {
                vm: VmRef::Name("r".into()),
            },
            P::default().u8(1).str("r").frame(0x0109),
        ),
        (
            StopVm { vm: VmRef::Id(7) },
            P::default().u8(0).u32(7).frame(0x010A),
        ),
        (
            DestroyVm {
                vm: VmRef::Name("d".into()),
            },
            P::default().u8(1).str("d").frame(0x010B),
        ),
        (
            VmStateChanged {
                vm: 2,
                state: VmState::Running,
            },
            P::default().u32(2).u8(1).frame(0x010C),
        ),
        (
            Detached {
                vm: 3,
                kind: AttachKind::Screen,
            },
            P::default().u32(3).u8(2).frame(0x010D),
        ),
        (
            ConsoleAttach { vm: VmRef::Id(1) },
            P::default().u8(0).u32(1).frame(0x0201),
        ),
        (
            ConsoleAttached {
                vm: 1,
                scrollback: b"hi".to_vec(),
            },
            P::default().u32(1).bytes(b"hi").frame(0x0202),
        ),
        (
            ConsoleInput { vm: 2, data: vec![3] },
            P::default().u32(2).bytes(&[3]).frame(0x0203),
        ),
        (
            ConsoleOutput {
                vm: 3,
                data: b"ok".to_vec(),
            },
            P::default().u32(3).bytes(b"ok").frame(0x0204),
        ),
        (ConsoleDetach { vm: 4 }, P::default().u32(4).frame(0x0205)),
        (
            ScreenAttach {
                vm: VmRef::Name("s".into()),
            },
            P::default().u8(1).str("s").frame(0x0301),
        ),
        (
            ScreenAttached {
                vm: 1,
                width: 640,
                height: 480,
                format: 7,
            },
            P::default().u32(1).u32(640).u32(480).u32(7).frame(0x0302),
        ),
        (
            ScreenFrame {
                vm: 1,
                x: 2,
                y: 3,
                width: 1,
                height: 2,
                pixels: (10..18).collect(),
            },
            P::default()
                .u32(1)
                .u32(2)
                .u32(3)
                .u32(1)
                .u32(2)
                .bytes(&[10, 11, 12, 13, 14, 15, 16, 17])
                .frame(0x0303),
        ),
        (
            KeyInput {
                vm: 1,
                key: KeyEvent {
                    code: b'k',
                    release: false,
                    shift: true,
                    ctrl: false,
                },
            },
            P::default().u32(1).u8(b'k').u8(0b010).frame(0x0304),
        ),
        (ScreenDetach { vm: 5 }, P::default().u32(5).frame(0x0305)),
        (
            PointerInput {
                vm: 6,
                pointer: PointerChange {
                    motion: PointerMotion::To { x: 100, y: 50 },
                    buttons: POINTER_LEFT,
                    wheel: -1,
                },
            },
            P::default()
                .u32(6)
                .u8(1)
                .u32(100)
                .u32(50)
                .u8(1)
                .i8(-1)
                .frame(0x0306),
        ),
        (
            BlockOpen {
                name: "d".into(),
                read_only: true,
            },
            P::default().str("d").u8(1).frame(0x0401),
        ),
        (
            BlockOpened {
                handle: 0x11,
                sectors: 0x22,
                read_only: false,
            },
            P::default().u32(0x11).u64(0x22).u8(0).frame(0x0402),
        ),
        (
            BlockRead {
                handle: 0x11,
                sector: 0x22,
                count: 0x33,
            },
            P::default().u32(0x11).u64(0x22).u32(0x33).frame(0x0403),
        ),
        (
            BlockData {
                handle: 0x11,
                sector: 0x22,
                data: vec![0xab; SECTOR_SIZE],
            },
            P::default()
                .u32(0x11)
                .u64(0x22)
                .bytes(&[0xab; SECTOR_SIZE])
                .frame(0x0404),
        ),
        (
            BlockWrite {
                handle: 0x12,
                sector: 0x23,
                data: vec![0xcd; SECTOR_SIZE],
            },
            P::default()
                .u32(0x12)
                .u64(0x23)
                .bytes(&[0xcd; SECTOR_SIZE])
                .frame(0x0405),
        ),
        (BlockFlush { handle: 0x13 }, P::default().u32(0x13).frame(0x0406)),
        (BlockSize { handle: 0x14 }, P::default().u32(0x14).frame(0x0407)),
        (
            BlockCapacity {
                handle: 0x15,
                sectors: 0x1_0000_0002,
            },
            P::default().u32(0x15).u64(0x1_0000_0002).frame(0x0408),
        ),
        (BlockClose { handle: 0x16 }, P::default().u32(0x16).frame(0x0409)),
        (
            DiskCreate {
                name: "d".into(),
                sectors: 0x44,
            },
            P::default().str("d").u64(0x44).frame(0x0501),
        ),
        (ListDisks, P::default().frame(0x0502)),
        (
            DiskList {
                disks: vec![DiskInfo {
                    name: "d".into(),
                    sectors: 0x55,
                    readers: 2,
                    writer: true,
                }],
            },
            P::default().u32(1).str("d").u64(0x55).u32(2).u8(1).frame(0x0503),
        ),
        (
            DiskDelete { name: "d".into() },
            P::default().str("d").frame(0x0504),
        ),
        (
            CreateSwitch {
                name: "s".into(),
                nat: Some(NatConfig {
                    hostfwd: vec![PortForward {
                        proto: FwdProto::Udp,
                        host: "127.0.0.1:8080".parse().unwrap(),
                        guest: "10.0.2.16:80".parse().unwrap(),
                    }],
                }),
            },
            P::default()
                .str("s")
                .u8(1)
                .u32(1)
                .u8(2)
                .raw(&[127, 0, 0, 1])
                .u16(8080)
                .raw(&[10, 0, 2, 16])
                .u16(80)
                .frame(0x0601),
        ),
        (
            DeleteSwitch { name: "t".into() },
            P::default().str("t").frame(0x0602),
        ),
        (ListSwitches, P::default().frame(0x0603)),
        (
            SwitchList {
                switches: vec![SwitchInfo {
                    name: "s".into(),
                    capture: Some("c".into()),
                    nat: Some(NatInfo {
                        hostfwd: vec![PortForward {
                            proto: FwdProto::Tcp,
                            host: "0.0.0.0:2222".parse().unwrap(),
                            guest: "10.0.2.15:22".parse().unwrap(),
                        }],
                        leases: vec![Lease {
                            mac: [2, 3, 4, 5, 6, 7],
                            addr: "10.0.2.15".parse().unwrap(),
                        }],
                    }),
                    ports: vec![
                        SwitchPort {
                            owner: PortOwner::Card { vm: 3, card: 1 },
                            mac: [2, 3, 4, 5, 6, 7],
                            in_frames: 0x11,
                            in_bytes: 0x22,
                            out_frames: 0x33,
                            out_bytes: 0x44,
                            dropped: 0x55,
                        },
                        SwitchPort {
                            owner: PortOwner::Gateway,
                            mac: [0x52, 0x55, 0x0a, 0, 2, 2],
                            in_frames: 1,
                            in_bytes: 2,
                            out_frames: 3,
                            out_bytes: 4,
                            dropped: 5,
                        },
                    ],
                }],
            },
            P::default()
                .u32(1)
                .str("s")
                .u8(1)
                .str("c")
                // nat: some, one rule, one lease
                .u8(1)
                .u32(1)
                .u8(1)
                .raw(&[0, 0, 0, 0])
                .u16(2222)
                .raw(&[10, 0, 2, 15])
                .u16(22)
                .u32(1)
                .raw(&[2, 3, 4, 5, 6, 7])
                .raw(&[10, 0, 2, 15])
                // ports
                .u32(2)
                .u8(1)
                .u32(3)
                .u8(1)
                .raw(&[2, 3, 4, 5, 6, 7])
                .u64(0x11)
                .u64(0x22)
                .u64(0x33)
                .u64(0x44)
                .u64(0x55)
                .u8(2)
                .raw(&[0x52, 0x55, 0x0a, 0, 2, 2])
                .u64(1)
                .u64(2)
                .u64(3)
                .u64(4)
                .u64(5)
                .frame(0x0604),
        ),
        (
            AttachCard {
                vm: VmRef::Id(4),
                card: 2,
                switch: "s".into(),
            },
            P::default().u8(0).u32(4).u8(2).str("s").frame(0x0605),
        ),
        (
            DetachCard {
                vm: VmRef::Name("v".into()),
                card: 3,
            },
            P::default().u8(1).str("v").u8(3).frame(0x0606),
        ),
        (
            CaptureSwitch {
                switch: "s".into(),
                path: Some("p".into()),
            },
            P::default().str("s").u8(1).str("p").frame(0x0607),
        ),
        (
            SetSwitchNat {
                switch: "s".into(),
                nat: Some(NatConfig::default()),
            },
            P::default().str("s").u8(1).u32(0).frame(0x0608),
        ),
    ]
}

#[test]
fn every_message_type_has_the_documented_layout() {
    let cases = layouts();
    let covered: BTreeSet<u16> = cases.iter().map(|(m, _)| m.type_code()).collect();
    let all: BTreeSet<u16> = SPEC_CATALOGUE.iter().map(|(c, _, _)| *c).collect();
    assert_eq!(covered, all, "a message type has no layout case");
    for (msg, want) in cases {
        assert_eq!(encode(&msg).unwrap(), want, "{}", msg.name());
        assert_eq!(decode(&want).unwrap(), (msg, want.len()));
    }
}

#[test]
fn every_enum_tag_is_the_documented_one() {
    let states = [
        (VmState::Created, P::default().u8(0)),
        (VmState::Running, P::default().u8(1)),
        (VmState::Paused, P::default().u8(2)),
        (VmState::Stopped, P::default().u8(3)),
        (VmState::Exited(9), P::default().u8(4).u32(9)),
    ];
    for (state, tail) in states {
        let msg = Message::VmStateChanged { vm: 1, state };
        let want = P::default().u32(1).raw(&tail.0).frame(0x010C);
        assert_eq!(encode(&msg).unwrap(), want, "{state:?}");
        assert_eq!(decode(&want).unwrap().0, msg);
    }
    for (kind, tag) in [(AttachKind::Console, 1), (AttachKind::Screen, 2)] {
        let msg = Message::Detached { vm: 1, kind };
        assert_eq!(encode(&msg).unwrap(), P::default().u32(1).u8(tag).frame(0x010D));
    }
    for (release, shift, ctrl, flags) in [
        (true, false, false, 0b001),
        (false, true, false, 0b010),
        (false, false, true, 0b100),
        (true, true, true, 0b111),
    ] {
        let msg = Message::KeyInput {
            vm: 1,
            key: KeyEvent {
                code: 1,
                release,
                shift,
                ctrl,
            },
        };
        assert_eq!(
            encode(&msg).unwrap(),
            P::default().u32(1).u8(1).u8(flags).frame(0x0304)
        );
    }
    for (motion, fields) in [
        (PointerMotion::None, P::default().u8(0)),
        (PointerMotion::To { x: 7, y: 8 }, P::default().u8(1).u32(7).u32(8)),
        (
            PointerMotion::By { dx: -7, dy: 8 },
            P::default().u8(2).i32(-7).i32(8),
        ),
    ] {
        for (buttons, wheel) in [
            (POINTER_LEFT, 0),
            (POINTER_RIGHT, 1),
            (POINTER_MIDDLE, -1),
            (0b111, 127),
        ] {
            let msg = Message::PointerInput {
                vm: 1,
                pointer: PointerChange {
                    motion,
                    buttons,
                    wheel,
                },
            };
            let want = P::default()
                .u32(1)
                .raw(&fields.0)
                .u8(buttons)
                .i8(wheel)
                .frame(0x0306);
            assert_eq!(encode(&msg).unwrap(), want, "{msg:?}");
            assert_eq!(decode(&want).unwrap().0, msg);
        }
    }
    assert_eq!((POINTER_LEFT, POINTER_RIGHT, POINTER_MIDDLE), (1, 2, 4));
}

#[test]
fn optional_fields_are_tagged() {
    let stop = Message::CaptureSwitch {
        switch: "s".into(),
        path: None,
    };
    assert_eq!(encode(&stop).unwrap(), P::default().str("s").u8(0).frame(0x0607));
    let list = Message::SwitchList {
        switches: vec![SwitchInfo {
            name: "s".into(),
            capture: None,
            nat: None,
            ports: Vec::new(),
        }],
    };
    assert_eq!(
        encode(&list).unwrap(),
        P::default().u32(1).str("s").u8(0).u8(0).u32(0).frame(0x0604)
    );
    let plain = Message::CreateSwitch {
        name: "s".into(),
        nat: None,
    };
    assert_eq!(encode(&plain).unwrap(), P::default().str("s").u8(0).frame(0x0601));
    let no_nat = Message::SetSwitchNat {
        switch: "s".into(),
        nat: None,
    };
    assert_eq!(
        encode(&no_nat).unwrap(),
        P::default().str("s").u8(0).frame(0x0608)
    );
}

#[test]
fn doc_examples_are_exact() {
    // Section 6.1: Hello version 6 from agent "rt".
    assert_eq!(
        encode(&Message::Hello {
            version: 6,
            agent: "rt".into()
        })
        .unwrap(),
        [0x0e, 0, 0, 0, 0x01, 0x00, 0x41, 0x56, 0x4c, 0x4e, 0x06, 0x00, 0x02, 0, 0, 0, 0x72, 0x74]
    );
    // Section 7: the version this document describes.
    assert_eq!(PROTOCOL_VERSION, 6);
    // Section 6.2: StartVm by id 5.
    assert_eq!(
        encode(&Message::StartVm { vm: VmRef::Id(5) }).unwrap(),
        [0x07, 0, 0, 0, 0x07, 0x01, 0x00, 0x05, 0, 0, 0]
    );
}

#[test]
fn guest_addresses_are_the_gateways() {
    // Section 3: the rule for a forwarded port's guest address is the one the NAT
    // gateway applies (the protocol crate depends on nothing, so it has its own copy).
    for last in 0..=255u8 {
        for net in [[10, 0, 2], [10, 0, 3], [192, 168, 2]] {
            let ip = std::net::Ipv4Addr::new(net[0], net[1], net[2], last);
            assert_eq!(is_guest_addr(ip), gateway::is_guest_addr(ip), "{ip}");
        }
    }
    // And a rule reads back as the gateway writes it.
    let rule = PortForward {
        proto: FwdProto::Udp,
        host: "127.0.0.1:5353".parse().unwrap(),
        guest: "10.0.2.16:53".parse().unwrap(),
    };
    let theirs = gateway::HostFwd::parse(&rule.to_string()).unwrap();
    assert_eq!(
        (theirs.proto, theirs.host, theirs.guest),
        (gateway::Proto::Udp, rule.host, rule.guest)
    );
    assert_eq!(theirs.to_string(), rule.to_string());
}
