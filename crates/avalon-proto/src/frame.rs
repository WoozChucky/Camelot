//! Framing (protocol doc, "Framing"):
//!
//! ```text
//! +----------------+----------------+-----------------------+
//! | length: u32 LE | type: u16 LE   | payload: length-2 B   |
//! +----------------+----------------+-----------------------+
//! ```
//!
//! `length` counts the type and the payload, so it is at least 2 and at most
//! [`MAX_FRAME_LEN`]. Payloads are raw bytes: console output, pixels and disk blocks
//! travel as they are, with no escaping.

use crate::message::Message;
use crate::wire::Writer;
use crate::{Error, Result, HEADER_LEN, MAX_FRAME_LEN};
use std::io::{self, Read, Write};

/// One frame: a message type and its still-undecoded payload. Reading a whole frame
/// before decoding it keeps the stream in step even when the payload is bad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub ty: u16,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Encodes `msg`'s payload. Fails if the message breaks a rule
    /// ([`Message::check`]) or does not fit in [`MAX_FRAME_LEN`].
    pub fn from_message(msg: &Message) -> Result<Frame> {
        msg.check()?;
        let mut w = Writer::default();
        msg.put(&mut w);
        let len = 2 + w.buf.len() as u64;
        if len > MAX_FRAME_LEN as u64 {
            return Err(Error::FrameTooLarge(len));
        }
        Ok(Frame {
            ty: msg.type_code(),
            payload: w.buf,
        })
    }

    /// Decodes the payload as a message of this frame's type.
    pub fn message(&self) -> Result<Message> {
        Message::get(self.ty, &self.payload)
    }

    /// The frame's bytes, header included.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        let len = (2 + self.payload.len() as u64).min(u32::MAX as u64) as u32;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&self.ty.to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// Encodes `msg` as one complete frame.
pub fn encode(msg: &Message) -> Result<Vec<u8>> {
    Ok(Frame::from_message(msg)?.to_bytes())
}

/// Decodes the frame at the start of `buf`, returning the message and the number of
/// bytes it took (so a buffer holding several frames can be walked). A buffer that
/// ends inside the frame gives [`Error::Truncated`].
pub fn decode(buf: &[u8]) -> Result<(Message, usize)> {
    let Some(len_bytes) = buf.get(..4) else {
        return Err(Error::Truncated);
    };
    let len = u32::from_le_bytes(len_bytes.try_into().unwrap());
    check_len(len)?;
    let end = 4 + len as usize;
    let Some(body) = buf.get(4..end) else {
        return Err(Error::Truncated);
    };
    let ty = u16::from_le_bytes([body[0], body[1]]);
    Ok((Message::get(ty, &body[2..])?, end))
}

fn check_len(len: u32) -> Result<()> {
    if len > MAX_FRAME_LEN {
        Err(Error::FrameTooLarge(len as u64))
    } else if len < 2 {
        Err(Error::FrameTooShort(len))
    } else {
        Ok(())
    }
}

/// Reads one frame. A clean end of stream before the first byte is [`Error::Closed`];
/// one inside a frame is an `UnexpectedEof` [`Error::Io`]. An oversize length is
/// rejected before anything is allocated, and leaves the stream unusable; a too-short
/// one (0 or 1) is consumed, so the next frame can still be read.
pub fn read_frame(r: &mut impl Read) -> Result<Frame> {
    let mut len_bytes = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len_bytes[got..]) {
            Ok(0) if got == 0 => return Err(Error::Closed),
            Ok(0) => return Err(Error::Io(io::ErrorKind::UnexpectedEof.into())),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(Error::FrameTooLarge(len as u64));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    check_len(len)?;
    let payload = body.split_off(2);
    Ok(Frame {
        ty: u16::from_le_bytes([body[0], body[1]]),
        payload,
    })
}

/// Writes one frame with a single `write_all`, then flushes.
pub fn write_frame(w: &mut impl Write, frame: &Frame) -> Result<()> {
    if 2 + frame.payload.len() as u64 > MAX_FRAME_LEN as u64 {
        return Err(Error::FrameTooLarge(2 + frame.payload.len() as u64));
    }
    w.write_all(&frame.to_bytes())?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_length_type_payload() {
        let bytes = encode(&Message::Ping { token: 0xddcc_bbaa }).unwrap();
        assert_eq!(bytes, [6, 0, 0, 0, 0x05, 0x00, 0xaa, 0xbb, 0xcc, 0xdd]);
        assert_eq!(encode(&Message::Ok).unwrap(), [2, 0, 0, 0, 0x04, 0x00]);
    }

    #[test]
    fn decode_walks_a_buffer_of_frames() {
        let mut buf = encode(&Message::Ok).unwrap();
        buf.extend(encode(&Message::Ping { token: 7 }).unwrap());
        let (first, n) = decode(&buf).unwrap();
        assert_eq!((first, n), (Message::Ok, 6));
        let (second, m) = decode(&buf[n..]).unwrap();
        assert_eq!((second, n + m), (Message::Ping { token: 7 }, buf.len()));
    }

    #[test]
    fn short_length_is_consumed() {
        let mut stream: &[u8] = &[1, 0, 0, 0, 0xee, 2, 0, 0, 0, 4, 0];
        assert!(matches!(read_frame(&mut stream), Err(Error::FrameTooShort(1))));
        assert_eq!(read_frame(&mut stream).unwrap().message().unwrap(), Message::Ok);
        assert!(matches!(read_frame(&mut stream), Err(Error::Closed)));
    }
}
