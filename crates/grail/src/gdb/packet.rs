//! Framing of the GDB remote serial protocol (GDB manual, "Remote Protocol",
//! "Overview"), with nothing about what packets mean.
//!
//! A packet is `$`, its payload, `#` and a two-digit hex checksum: the sum of the
//! payload's bytes modulo 256. The receiver answers `+` (received) or `-` (bad
//! checksum, send it again), until the two sides agree to drop the acks
//! (`QStartNoAckMode`). A single 0x03 byte outside a packet is an interrupt request
//! (the client's Ctrl-C). Binary data (an `X` packet's, a `qXfer` reply's) escapes the
//! bytes `$`, `#`, `}` and `*` as `}` followed by the byte XOR 0x20.

/// The longest payload accepted. Longer ones are dropped as malformed; the stub
/// announces a smaller size to the client (`PacketSize` in `qSupported`).
pub const MAX_PAYLOAD: usize = 0x10000;

/// The checksum of a payload: its bytes added up, modulo 256.
pub fn checksum(payload: &[u8]) -> u8 {
    payload.iter().fold(0u8, |sum, &b| sum.wrapping_add(b))
}

/// The packet that carries `payload`: `$payload#cc`. The payload is sent as is: one
/// with binary data escapes it first ([`escape`]).
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.push(b'$');
    out.extend_from_slice(payload);
    out.extend_from_slice(format!("#{:02x}", checksum(payload)).as_bytes());
    out
}

/// Escapes binary data for a payload: `$`, `#`, `}` and `*` become `}` and the byte
/// XOR 0x20.
pub fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for &b in data {
        if matches!(b, b'$' | b'#' | b'}' | b'*') {
            out.extend_from_slice(&[b'}', b ^ 0x20]);
        } else {
            out.push(b);
        }
    }
    out
}

/// Undoes [`escape`]. `None` if the data ends in the middle of an escape.
pub fn unescape(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    let mut bytes = data.iter();
    while let Some(&b) = bytes.next() {
        if b == b'}' {
            out.push(bytes.next()? ^ 0x20);
        } else {
            out.push(b);
        }
    }
    Some(out)
}

/// What the bytes from the client amount to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A packet with a good checksum: its payload, still escaped.
    Packet(Vec<u8>),
    /// A packet with a bad checksum, or too long: answered with `-`.
    Bad,
    /// `+`: the last packet sent arrived.
    Ack,
    /// `-`: it did not; send it again.
    Nack,
    /// 0x03: stop the running machine.
    Interrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Between packets.
    Idle,
    /// After the `$`.
    Payload,
    /// After the `#`: waiting for the first checksum digit, then the second.
    Checksum1,
    Checksum2(u8),
}

/// Splits the byte stream from a client into [`Event`]s, one byte at a time, so it
/// does not matter how the bytes were split into reads.
pub struct Decoder {
    state: State,
    payload: Vec<u8>,
    /// The payload grew past [`MAX_PAYLOAD`]: the packet is dropped when it ends.
    too_long: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder {
            state: State::Idle,
            payload: Vec::new(),
            too_long: false,
        }
    }
}

impl Decoder {
    /// Feeds one byte; returns the event it completes, if any.
    pub fn push(&mut self, b: u8) -> Option<Event> {
        match self.state {
            State::Idle => match b {
                b'$' => {
                    self.restart();
                    self.state = State::Payload;
                    None
                }
                b'+' => Some(Event::Ack),
                b'-' => Some(Event::Nack),
                0x03 => Some(Event::Interrupt),
                // Anything else between packets is noise: skipped.
                _ => None,
            },
            State::Payload => {
                match b {
                    b'#' => self.state = State::Checksum1,
                    // A `$` inside a packet starts it over: the client gave up on the
                    // one before.
                    b'$' => self.restart(),
                    _ if self.payload.len() >= MAX_PAYLOAD => self.too_long = true,
                    _ => self.payload.push(b),
                }
                None
            }
            State::Checksum1 => match hex_digit(b) {
                Some(d) => {
                    self.state = State::Checksum2(d);
                    None
                }
                None => self.bad(),
            },
            State::Checksum2(hi) => match hex_digit(b) {
                Some(lo) if !self.too_long && (hi << 4 | lo) == checksum(&self.payload) => {
                    self.state = State::Idle;
                    Some(Event::Packet(std::mem::take(&mut self.payload)))
                }
                _ => self.bad(),
            },
        }
    }

    fn restart(&mut self) {
        self.payload.clear();
        self.too_long = false;
    }

    fn bad(&mut self) -> Option<Event> {
        self.state = State::Idle;
        self.restart();
        Some(Event::Bad)
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Vec<Event> {
        let mut d = Decoder::default();
        bytes.iter().filter_map(|&b| d.push(b)).collect()
    }

    #[test]
    fn frames_with_checksums() {
        assert_eq!(checksum(b""), 0);
        assert_eq!(checksum(b"OK"), 0x9a);
        assert_eq!(frame(b"OK"), b"$OK#9a");
        assert_eq!(frame(b""), b"$#00");
        assert_eq!(frame(b"g"), b"$g#67");
        // The sum wraps: 3 * 0xff = 0x2fd.
        assert_eq!(checksum(&[0xff; 3]), 0xfd);
    }

    #[test]
    fn decodes_packets_acks_and_interrupts() {
        assert_eq!(
            decode(b"+$g#67-\x03$?#3f"),
            vec![
                Event::Ack,
                Event::Packet(b"g".to_vec()),
                Event::Nack,
                Event::Interrupt,
                Event::Packet(b"?".to_vec()),
            ]
        );
        // Upper-case checksum digits are fine; noise between packets is skipped.
        assert_eq!(decode(b"xx$OK#9A"), vec![Event::Packet(b"OK".to_vec())]);
        assert_eq!(decode(b"$m1000,4#8e"), vec![Event::Packet(b"m1000,4".to_vec())]);
    }

    #[test]
    fn bad_checksums_and_garbage_are_reported() {
        assert_eq!(decode(b"$g#00"), vec![Event::Bad]);
        assert_eq!(decode(b"$g#zz"), vec![Event::Bad]);
        assert_eq!(decode(b"$g#6"), vec![], "not finished yet");
        // After a bad packet the next one decodes.
        assert_eq!(
            decode(b"$g#00$g#67"),
            vec![Event::Bad, Event::Packet(b"g".to_vec())]
        );
        // A `$` in a packet restarts it.
        assert_eq!(decode(b"$abc$g#67"), vec![Event::Packet(b"g".to_vec())]);
        // 0x03 inside a packet is data, not an interrupt.
        assert_eq!(decode(b"$\x03#03"), vec![Event::Packet(vec![3])]);
    }

    #[test]
    fn oversized_packets_are_dropped() {
        let mut p = vec![b'$'];
        p.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD + 1));
        let sum = checksum(&p[1..]);
        p.extend_from_slice(format!("#{sum:02x}").as_bytes());
        p.extend_from_slice(b"$g#67");
        assert_eq!(decode(&p), vec![Event::Bad, Event::Packet(b"g".to_vec())]);
    }

    #[test]
    fn escapes_round_trip() {
        let data = b"a$b#c}d*e\x00\xff";
        let escaped = escape(data);
        assert_eq!(escaped, b"a}\x04b}\x03c}]d}\x0ae\x00\xff");
        assert_eq!(unescape(&escaped).unwrap(), data);
        assert_eq!(unescape(b"ab}"), None);
        assert_eq!(unescape(b""), Some(vec![]));
    }
}
