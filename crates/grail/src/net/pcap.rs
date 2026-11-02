//! Packet capture: a backend wrapper that records every frame passing through it, in
//! both directions, in a classic libpcap file (the format Wireshark and tcpdump read).
//!
//! The file is a 24-byte global header followed by one record per frame: a 16-byte
//! record header (timestamp in seconds and microseconds, captured length, original
//! length) and the frame's bytes. Every field is little-endian here, which readers
//! recognise from the magic number. The link type is Ethernet, without the FCS, as the
//! card's frames are.

use super::NetBackend;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Magic number of a microsecond-resolution pcap file.
pub const PCAP_MAGIC: u32 = 0xa1b2_c3d4;
/// Longest frame a record may hold; the card's are at most 1514 bytes, so none is cut.
pub const PCAP_SNAPLEN: u32 = 65535;
/// `LINKTYPE_ETHERNET`: IEEE 802.3 Ethernet frames.
pub const LINKTYPE_ETHERNET: u32 = 1;

/// The time stamp of each record, from the Unix epoch.
pub type Clock = Box<dyn FnMut() -> Duration + Send>;

/// A [`NetBackend`] that passes every frame on to `inner` and writes it to a capture:
/// frames sent when they are sent, received frames when the card takes them from the
/// backend (even if the card then drops them for want of a buffer).
pub struct Pcap {
    inner: Box<dyn NetBackend>,
    out: Box<dyn Write + Send>,
    clock: Clock,
    /// The first write error. Capture stops there; the frames keep flowing.
    failed: Option<io::Error>,
}

impl Pcap {
    /// Captures `inner`'s frames into a new file at `path` (replacing any), time-stamped
    /// with the host's wall clock.
    pub fn create(path: impl AsRef<Path>, inner: Box<dyn NetBackend>) -> io::Result<Self> {
        let file = BufWriter::new(File::create(path)?);
        let wall_clock = || SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Pcap::new(inner, file, Box::new(wall_clock))
    }

    /// Captures `inner`'s frames into `out`, taking each record's time stamp from
    /// `clock` (tests use a fixed sequence, so the capture is the same on every run).
    /// Writes the file header at once.
    pub fn new(
        inner: Box<dyn NetBackend>,
        out: impl Write + Send + 'static,
        clock: Clock,
    ) -> io::Result<Self> {
        let mut out: Box<dyn Write + Send> = Box::new(out);
        out.write_all(&header())?;
        out.flush()?;
        Ok(Pcap {
            inner,
            out,
            clock,
            failed: None,
        })
    }

    /// The write error that stopped the capture, if one did.
    pub fn error(&self) -> Option<&io::Error> {
        self.failed.as_ref()
    }

    pub fn inner(&mut self) -> &mut dyn NetBackend {
        &mut *self.inner
    }

    fn record(&mut self, frame: &[u8]) {
        if self.failed.is_some() {
            return;
        }
        let rec = record((self.clock)(), frame);
        // Flushed per frame, so the file is complete whenever the machine stops, and a
        // reader following it live sees every frame at once.
        if let Err(e) = self.out.write_all(&rec).and_then(|()| self.out.flush()) {
            self.failed = Some(e);
        }
    }
}

impl NetBackend for Pcap {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        self.record(frame);
        self.inner.send(frame)
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let frame = self.inner.recv()?;
        self.record(&frame);
        Some(frame)
    }

    fn link_up(&self) -> bool {
        self.inner.link_up()
    }
}

/// The 24-byte global header: magic, version 2.4, time zone 0, accuracy 0, snap length,
/// link type.
pub fn header() -> [u8; 24] {
    let mut h = Vec::with_capacity(24);
    h.extend_from_slice(&PCAP_MAGIC.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes()); // major version
    h.extend_from_slice(&4u16.to_le_bytes()); // minor version
    h.extend_from_slice(&0i32.to_le_bytes()); // time zone: UTC
    h.extend_from_slice(&0u32.to_le_bytes()); // time stamp accuracy: unused
    h.extend_from_slice(&PCAP_SNAPLEN.to_le_bytes());
    h.extend_from_slice(&LINKTYPE_ETHERNET.to_le_bytes());
    h.try_into().expect("24 bytes")
}

/// One record: time stamp (seconds, microseconds), captured length, original length,
/// then the frame (cut to the snap length, which no card frame reaches).
pub fn record(time: Duration, frame: &[u8]) -> Vec<u8> {
    let len = frame.len().min(PCAP_SNAPLEN as usize);
    let mut rec = Vec::with_capacity(16 + len);
    for v in [
        time.as_secs() as u32,
        time.subsec_micros(),
        len as u32,
        frame.len() as u32,
    ] {
        rec.extend_from_slice(&v.to_le_bytes());
    }
    rec.extend_from_slice(&frame[..len]);
    rec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Hub;
    use std::sync::{Arc, Mutex};

    /// A writer the test can still read after handing it over.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// 1.5 s, 2.5 s, ... after the epoch.
    fn ticking() -> Clock {
        let mut t = Duration::from_millis(500);
        Box::new(move || {
            t += Duration::from_secs(1);
            t
        })
    }

    #[test]
    fn header_bytes() {
        assert_eq!(
            header(),
            [
                0xd4, 0xc3, 0xb2, 0xa1, // magic, little-endian
                2, 0, 4, 0, // version 2.4
                0, 0, 0, 0, // this zone
                0, 0, 0, 0, // sigfigs
                0xff, 0xff, 0, 0, // snaplen 65535
                1, 0, 0, 0, // LINKTYPE_ETHERNET
            ]
        );
    }

    #[test]
    fn records_both_directions() {
        let hub = Hub::new();
        let out = Shared::default();
        let mut cap = Pcap::new(Box::new(hub.attach()), out.clone(), ticking()).unwrap();
        let mut peer = hub.attach();
        assert_eq!(out.0.lock().unwrap().len(), 24, "the header comes first");

        cap.send(&[1; 14]).unwrap();
        assert_eq!(peer.recv(), Some(vec![1; 14]), "passed on");
        assert_eq!(cap.recv(), None, "nothing to receive: no record");
        peer.send(&[2; 20]).unwrap();
        assert_eq!(cap.recv(), Some(vec![2; 20]));

        let mut want = header().to_vec();
        want.extend([1, 0, 0, 0, 0x20, 0xa1, 0x07, 0, 14, 0, 0, 0, 14, 0, 0, 0]);
        want.extend([1; 14]);
        want.extend([2, 0, 0, 0, 0x20, 0xa1, 0x07, 0, 20, 0, 0, 0, 20, 0, 0, 0]);
        want.extend([2; 20]);
        assert_eq!(*out.0.lock().unwrap(), want);
        assert!(cap.error().is_none());
        assert!(cap.link_up());
    }

    #[test]
    fn long_frames_are_cut_to_the_snap_length() {
        let frame = vec![7; PCAP_SNAPLEN as usize + 10];
        let rec = record(Duration::new(3, 999_999_999), &frame);
        assert_eq!(&rec[..4], &3u32.to_le_bytes());
        assert_eq!(&rec[4..8], &999_999u32.to_le_bytes());
        assert_eq!(&rec[8..12], &PCAP_SNAPLEN.to_le_bytes());
        assert_eq!(&rec[12..16], &(PCAP_SNAPLEN + 10).to_le_bytes());
        assert_eq!(rec.len(), 16 + PCAP_SNAPLEN as usize);
    }

    /// A writer that fails once `limit` bytes are written.
    struct Full(usize);

    impl Write for Full {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() > self.0 {
                return Err(io::Error::other("disk full"));
            }
            self.0 -= buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failing_capture_stops_but_frames_keep_flowing() {
        let hub = Hub::new();
        assert!(Pcap::new(Box::new(hub.attach()), Full(10), ticking()).is_err());
        let mut cap = Pcap::new(Box::new(hub.attach()), Full(24 + 30), ticking()).unwrap();
        let mut peer = hub.attach();
        cap.send(&[1; 14]).unwrap();
        assert!(cap.error().is_none());
        cap.send(&[2; 14]).unwrap();
        assert_eq!(cap.error().unwrap().to_string(), "disk full");
        cap.send(&[3; 14]).unwrap();
        let got: Vec<_> = std::iter::from_fn(|| peer.recv()).collect();
        assert_eq!(got, [vec![1; 14], vec![2; 14], vec![3; 14]]);
    }
}
