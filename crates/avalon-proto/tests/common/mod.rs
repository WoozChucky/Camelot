//! Shared by the integration tests: sample messages covering every type and every
//! enum variant, an in-memory stream, and a small PRNG for the fuzz tests.

#![allow(dead_code)]

use avalon_proto::*;
use std::io::{self, Cursor, Read, Write};

pub fn profile() -> HardwareProfile {
    HardwareProfile {
        ram_size: 64 * 1024 * 1024,
        clock_hz: Some(2_000_000),
        nominal_hz: 10_000_000,
        harts: 4,
        devices: vec![
            DeviceSpec::Disk(DiskSpec {
                source: DiskSource::File("C:\\vms\\root.img".into()),
                read_only: false,
            }),
            DeviceSpec::Disk(DiskSpec {
                source: DiskSource::Lake {
                    addr: "127.0.0.1:7401".into(),
                    name: "shared.img".into(),
                },
                read_only: true,
            }),
            DeviceSpec::Net(NetSpec {
                switch: Some("lan".into()),
                mac: Some([0x02, 0xca, 0, 0, 0, 1]),
            }),
            DeviceSpec::Net(NetSpec {
                switch: None,
                mac: None,
            }),
        ],
    }
}

fn summary(id: u32, name: &str, state: VmState, profile: HardwareProfile) -> VmSummary {
    VmSummary {
        id,
        name: name.into(),
        state,
        profile,
    }
}

/// At least one message of every type, with the enum-valued fields taking every
/// variant somewhere, plus edge values (empty strings and lists, `None`, maximum
/// integers, non-ASCII text).
pub fn samples() -> Vec<Message> {
    use Message::*;
    let all_states = [
        VmState::Created,
        VmState::Running,
        VmState::Paused,
        VmState::Stopped,
        VmState::Exited(0),
        VmState::Exited(u32::MAX),
        VmState::Debug,
    ];
    let mut v = vec![
        Hello {
            version: PROTOCOL_VERSION,
            agent: "round-table".into(),
        },
        Hello {
            version: u16::MAX,
            agent: String::new(),
        },
        Welcome {
            version: PROTOCOL_VERSION,
            server: "avalon 0.1".into(),
        },
        Ok,
        Ping { token: 0 },
        Pong { token: 0xdead_beef },
        CreateVm {
            name: "web-1".into(),
            profile: profile(),
            program: vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0],
        },
        CreateVm {
            name: "tiny".into(),
            profile: HardwareProfile::with_ram(4096),
            program: Vec::new(),
        },
        VmCreated { id: 7 },
        ListVms,
        VmList { vms: Vec::new() },
        VmList {
            vms: all_states
                .iter()
                .enumerate()
                .map(|(i, s)| summary(i as u32, &format!("vm{i}"), *s, HardwareProfile::default()))
                .collect(),
        },
        GetVmInfo { vm: VmRef::Id(3) },
        GetVmInfo {
            vm: VmRef::Name("web-1".into()),
        },
        VmInfo(VmDetails {
            vm: summary(1, "web-1", VmState::Running, profile()),
            devices: vec![
                DeviceEntry {
                    kind: 4,
                    base: 0x0010_0000,
                    size: 0x1000,
                    irq: 0,
                },
                DeviceEntry {
                    kind: 3,
                    base: 0x1000_0000,
                    size: 0x100,
                    irq: 1,
                },
            ],
            steps: u64::MAX,
        }),
        StartVm { vm: VmRef::Id(1) },
        PauseVm {
            vm: VmRef::Name("a".into()),
        },
        ResumeVm {
            vm: VmRef::Id(u32::MAX),
        },
        StopVm { vm: VmRef::Id(0) },
        DestroyVm {
            vm: VmRef::Name("web-1".into()),
        },
        Detached {
            vm: 1,
            kind: AttachKind::Console,
        },
        Detached {
            vm: 2,
            kind: AttachKind::Screen,
        },
        Detached {
            vm: 3,
            kind: AttachKind::Debug,
        },
        DebugAttach { vm: VmRef::Id(4) },
        DebugAttach {
            vm: VmRef::Name("kernel".into()),
        },
        DebugAttached { vm: 4 },
        DebugInput {
            vm: 4,
            data: Vec::new(),
        },
        // A packet, an ack and a Ctrl-C: the stream's bytes as they are.
        DebugInput {
            vm: 4,
            data: b"+$vCont;c#a8\x03".to_vec(),
        },
        DebugOutput {
            vm: 4,
            data: b"$T05thread:1;#40".to_vec(),
        },
        // Binary data (an `X` packet's) travels unchanged too.
        DebugOutput {
            vm: u32::MAX,
            data: (0..=255).collect(),
        },
        DebugDetach { vm: 4 },
        ConsoleAttach { vm: VmRef::Id(1) },
        ConsoleAttached {
            vm: 1,
            scrollback: b"Hello from Camelot!\n".to_vec(),
        },
        ConsoleInput {
            vm: 1,
            data: vec![0x03, 0x04, 0x1d, b'\r'],
        },
        ConsoleOutput {
            vm: 1,
            data: (0..=255).collect(),
        },
        ConsoleDetach { vm: 1 },
        ScreenAttach {
            vm: VmRef::Name("desk".into()),
        },
        ScreenAttached {
            vm: 4,
            width: 640,
            height: 480,
            format: PIXEL_FORMAT_XRGB8888,
        },
        ScreenResized {
            vm: 4,
            width: 1920,
            height: 1080,
            format: PIXEL_FORMAT_XRGB8888,
        },
        CursorShape {
            vm: 4,
            width: 64,
            height: 64,
            hot_x: 63,
            hot_y: 0,
            pixels: (0..64 * 64 * 4).map(|i| (i % 253) as u8).collect(),
        },
        CursorShape {
            vm: 4,
            width: MAX_CURSOR_SIZE,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            pixels: vec![0xff; MAX_CURSOR_SIZE as usize * 4],
        },
        CursorPosition {
            vm: 4,
            visible: true,
            x: -12,
            y: i32::MAX,
        },
        CursorPosition {
            vm: 4,
            visible: false,
            x: 0,
            y: 0,
        },
        ScreenFrame {
            vm: 4,
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            pixels: (0..640 * 480 * 4).map(|i| (i % 251) as u8).collect(),
        },
        ScreenFrame {
            vm: 4,
            x: 10,
            y: 20,
            width: 2,
            height: 1,
            pixels: vec![0x11, 0x22, 0x33, 0x00, 0x44, 0x55, 0x66, 0x00],
        },
        ScreenDetach { vm: 4 },
        BlockOpen {
            name: "root.img".into(),
            read_only: false,
        },
        BlockOpen {
            name: "shared.img".into(),
            read_only: true,
        },
        BlockOpened {
            handle: 1,
            sectors: 131_072,
            read_only: false,
        },
        BlockRead {
            handle: 1,
            sector: 0,
            count: 1,
        },
        BlockRead {
            handle: 1,
            sector: u64::MAX,
            count: MAX_BLOCK_SECTORS,
        },
        BlockData {
            handle: 1,
            sector: 0,
            data: vec![0xaa; SECTOR_SIZE],
        },
        BlockWrite {
            handle: 1,
            sector: 8,
            data: (0..SECTOR_SIZE * 3).map(|i| i as u8).collect(),
        },
        BlockWrite {
            handle: 2,
            sector: 0,
            data: vec![0x5a; SECTOR_SIZE * MAX_BLOCK_SECTORS as usize],
        },
        BlockFlush { handle: 1 },
        BlockSize { handle: 1 },
        BlockCapacity {
            handle: 1,
            sectors: 131_072,
        },
        BlockClose { handle: 1 },
        DiskCreate {
            name: "root.img".into(),
            sectors: 131_072,
            source: None,
        },
        DiskCreate {
            name: "web-disk0".into(),
            sectors: 0,
            source: Some("os-base".into()),
        },
        ListDisks,
        DiskList { disks: Vec::new() },
        DiskList {
            disks: vec![
                DiskInfo {
                    name: "root.img".into(),
                    sectors: 131_072,
                    readers: 0,
                    writer: true,
                },
                DiskInfo {
                    name: "shared.img".into(),
                    sectors: 2048,
                    readers: 3,
                    writer: false,
                },
            ],
        },
        DiskDelete {
            name: "old.img".into(),
        },
        CreateSwitch {
            name: "lan".into(),
            nat: None,
        },
        CreateSwitch {
            name: "wan".into(),
            nat: Some(NatConfig::default()),
        },
        CreateSwitch {
            name: "wan".into(),
            nat: Some(NatConfig {
                hostfwd: vec![
                    PortForward {
                        proto: FwdProto::Tcp,
                        host: "127.0.0.1:8080".parse().unwrap(),
                        guest: "10.0.2.15:80".parse().unwrap(),
                    },
                    PortForward {
                        proto: FwdProto::Udp,
                        host: "0.0.0.0:0".parse().unwrap(),
                        guest: "10.0.2.254:65535".parse().unwrap(),
                    },
                ],
            }),
        },
        SetSwitchNat {
            switch: "lan".into(),
            nat: Some(NatConfig::default()),
        },
        SetSwitchNat {
            switch: "lan".into(),
            nat: None,
        },
        DeleteSwitch { name: "dmz".into() },
        ListSwitches,
        SwitchList { switches: Vec::new() },
        SwitchList {
            switches: vec![
                SwitchInfo {
                    name: "lan".into(),
                    capture: Some("C:\\captures\\lan.pcap".into()),
                    nat: Some(NatInfo {
                        hostfwd: vec![PortForward {
                            proto: FwdProto::Tcp,
                            host: "127.0.0.1:49999".parse().unwrap(),
                            guest: "10.0.2.16:22".parse().unwrap(),
                        }],
                        leases: vec![
                            Lease {
                                mac: [0x02, 0xca, 0, 0, 0, 1],
                                addr: "10.0.2.15".parse().unwrap(),
                            },
                            Lease {
                                mac: [0x02, 0xca, 0, 0, 0, 2],
                                addr: "10.0.2.16".parse().unwrap(),
                            },
                        ],
                    }),
                    ports: vec![
                        SwitchPort {
                            owner: PortOwner::Gateway,
                            mac: [0x52, 0x55, 0x0a, 0, 2, 2],
                            in_frames: 3,
                            in_bytes: 1000,
                            out_frames: 4,
                            out_bytes: 1200,
                            dropped: 1,
                        },
                        SwitchPort {
                            owner: PortOwner::Card { vm: 1, card: 0 },
                            mac: [0x02, 0xca, 0, 0, 0, 1],
                            in_frames: 10,
                            in_bytes: 600,
                            out_frames: 12,
                            out_bytes: 720,
                            dropped: 0,
                        },
                        SwitchPort {
                            owner: PortOwner::Card {
                                vm: u32::MAX,
                                card: u8::MAX,
                            },
                            mac: [0xff; 6],
                            in_frames: u64::MAX,
                            in_bytes: u64::MAX,
                            out_frames: 0,
                            out_bytes: 0,
                            dropped: u64::MAX,
                        },
                    ],
                },
                SwitchInfo {
                    name: "empty".into(),
                    capture: None,
                    nat: None,
                    ports: Vec::new(),
                },
            ],
        },
        AttachCard {
            vm: VmRef::Name("web-1".into()),
            card: 0,
            switch: "lan".into(),
        },
        DetachCard {
            vm: VmRef::Id(2),
            card: 3,
        },
        CaptureSwitch {
            switch: "lan".into(),
            path: Some("lan.pcap".into()),
        },
        CaptureSwitch {
            switch: "lan".into(),
            path: None,
        },
    ];
    for state in all_states {
        v.push(VmStateChanged { vm: 9, state });
    }
    for code in ErrorCode::all().chain([ErrorCode::Other(999)]) {
        v.push(Error {
            code,
            message: format!("{code} \u{2014} détails"),
        });
    }
    for pointer in [
        PointerChange::default(),
        PointerChange {
            motion: PointerMotion::To { x: 639, y: 0 },
            buttons: POINTER_LEFT,
            wheel: 0,
        },
        PointerChange {
            motion: PointerMotion::To {
                x: u32::MAX,
                y: u32::MAX,
            },
            buttons: POINTER_BUTTONS,
            wheel: i8::MIN,
        },
        PointerChange {
            motion: PointerMotion::By { dx: -5, dy: 7 },
            buttons: POINTER_RIGHT | POINTER_MIDDLE,
            wheel: 3,
        },
        PointerChange {
            motion: PointerMotion::By {
                dx: i32::MIN,
                dy: i32::MAX,
            },
            buttons: 0,
            wheel: i8::MAX,
        },
    ] {
        v.push(PointerInput { vm: 4, pointer });
    }
    for (code, release, shift, ctrl, repeat, usage) in [
        (b'a', false, false, false, false, 0),
        (b'A', false, true, false, false, 0x04),
        (b'c', true, false, true, false, 0x06),
        (0x9b, false, true, true, true, 0x45),
        (0x9b, true, true, true, false, 0x45),
        (0, false, false, false, true, 0xe1),
        (0, true, false, false, false, 0xffff),
    ] {
        v.push(KeyInput {
            vm: 4,
            key: KeyEvent {
                code,
                release,
                shift,
                ctrl,
                repeat,
                usage,
            },
        });
    }
    v
}

/// An in-memory stream: reads come from `input`, writes collect in `output`.
pub struct Duplex {
    pub input: Cursor<Vec<u8>>,
    pub output: Vec<u8>,
}

impl Duplex {
    pub fn new(input: Vec<u8>) -> Self {
        Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        }
    }

    /// Input made of these messages' frames.
    pub fn with_messages(msgs: &[Message]) -> Self {
        Duplex::new(msgs.iter().flat_map(|m| encode(m).unwrap()).collect())
    }
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Decodes every frame in `bytes`.
pub fn decode_all(mut bytes: &[u8]) -> Vec<Message> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let (msg, n) = decode(bytes).unwrap();
        out.push(msg);
        bytes = &bytes[n..];
    }
    out
}

/// xorshift64*: deterministic, so a failing fuzz case can be replayed from its seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}
