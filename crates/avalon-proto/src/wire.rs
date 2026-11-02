//! Field encoding inside a payload (protocol doc, "Field types"): little-endian
//! integers, `bool` as one byte 0/1, length-prefixed bytes and strings, tagged options
//! and counted lists.
//!
//! The reader never trusts a length: every one is checked against the bytes actually
//! left in the payload before anything is read, and a list count never decides how
//! much memory is reserved up front, so a forged count costs nothing.

use crate::{Error, Result};

/// Builds a payload.
#[derive(Default)]
pub(crate) struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    pub fn raw(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    /// A `u32` length followed by the bytes. Lengths over `u32::MAX` cannot fit in a
    /// frame anyway; the frame size check rejects them (the length written here is
    /// then irrelevant).
    pub fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len().min(u32::MAX as usize) as u32);
        self.raw(v);
    }
    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
    pub fn option<T>(&mut self, v: &Option<T>, put: impl FnOnce(&mut Self, &T)) {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1);
                put(self, x);
            }
        }
    }
    pub fn list<T>(&mut self, v: &[T], mut put: impl FnMut(&mut Self, &T)) {
        self.u32(v.len().min(u32::MAX as usize) as u32);
        for x in v {
            put(self, x);
        }
    }
}

/// Most list elements [`Reader::list`] reserves room for before decoding any.
const MAX_LIST_PREALLOC: usize = 1024;

/// Reads a payload front to back.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len()
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.buf.len() {
            return Err(Error::Truncated);
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    pub fn magic(&mut self) -> Result<[u8; 4]> {
        self.array()
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(Error::Invalid(format!("bool must be 0 or 1, not {b}"))),
        }
    }

    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    pub fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        String::from_utf8(raw.to_vec()).map_err(|_| Error::Invalid("string is not valid UTF-8".into()))
    }

    pub fn option<T>(&mut self, get: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(get(self)?)),
            t => Err(Error::Invalid(format!("option tag must be 0 or 1, not {t}"))),
        }
    }

    /// A counted list. Every element takes at least one byte, so a count larger than
    /// the bytes left is truncated before any element is read.
    ///
    /// The count is the peer's word, and an element in memory can be many times its
    /// wire size, so it only bounds the up-front reservation: at most
    /// [`MAX_LIST_PREALLOC`] elements, and never more bytes than the payload has left.
    /// A longer list grows as its elements actually decode.
    pub fn list<T>(&mut self, mut get: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let count = self.u32()? as usize;
        if count > self.remaining() {
            return Err(Error::Truncated);
        }
        let fit = self.remaining() / std::mem::size_of::<T>().max(1);
        let mut v = Vec::with_capacity(count.min(fit).min(MAX_LIST_PREALLOC));
        for _ in 0..count {
            v.push(get(self)?);
        }
        Ok(v)
    }

    /// The payload must be used up exactly.
    pub fn finish(self) -> Result<()> {
        match self.buf.len() {
            0 => Ok(()),
            n => Err(Error::TrailingBytes(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_little_endian() {
        let mut w = Writer::default();
        w.u8(0x01);
        w.u16(0x0302);
        w.u32(0x0706_0504);
        w.u64(0x0f0e_0d0c_0b0a_0908);
        assert_eq!(w.buf, (1..=15).collect::<Vec<u8>>());
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.u8().unwrap(), 0x01);
        assert_eq!(r.u16().unwrap(), 0x0302);
        assert_eq!(r.u32().unwrap(), 0x0706_0504);
        assert_eq!(r.u64().unwrap(), 0x0f0e_0d0c_0b0a_0908);
        r.finish().unwrap();
    }

    #[test]
    fn strings_and_bytes_are_length_prefixed() {
        let mut w = Writer::default();
        w.str("hi");
        w.bytes(&[9]);
        assert_eq!(w.buf, [2, 0, 0, 0, b'h', b'i', 1, 0, 0, 0, 9]);
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.string().unwrap(), "hi");
        assert_eq!(r.bytes().unwrap(), [9]);
        r.finish().unwrap();
    }

    #[test]
    fn bad_values_are_errors() {
        assert!(matches!(Reader::new(&[2]).bool(), Err(Error::Invalid(_))));
        assert!(matches!(
            Reader::new(&[2]).option(|r| r.u8()),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            Reader::new(&[1, 0, 0, 0, 0xff]).string(),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            Reader::new(&[5, 0, 0, 0, 1]).bytes(),
            Err(Error::Truncated)
        ));
        assert!(matches!(Reader::new(&[1, 2]).u32(), Err(Error::Truncated)));
        assert!(matches!(Reader::new(&[1]).finish(), Err(Error::TrailingBytes(1))));
    }

    #[test]
    fn huge_list_count_is_rejected_before_allocating() {
        let mut r = Reader::new(&[0xff, 0xff, 0xff, 0xff, 0]);
        assert!(matches!(r.list(|r| r.u8()), Err(Error::Truncated)));
    }

    #[test]
    fn lists_longer_than_the_reservation_still_decode() {
        // 4096 elements of 512 bytes each in memory, one byte each on the wire: more
        // than both preallocation caps, so the vector has to grow.
        let mut payload = 4096u32.to_le_bytes().to_vec();
        payload.resize(4 + 4096, 7);
        let list = Reader::new(&payload)
            .list(|r| r.u8().map(|b| [b as u64; 64]))
            .unwrap();
        assert_eq!(list.len(), 4096);
        assert!(list.iter().all(|e| e[63] == 7));
    }
}
