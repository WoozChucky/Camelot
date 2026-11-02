//! One TCP connection through the gateway: a small TCP endpoint facing the guest
//! (RFC 793, with RFC 6298's retransmission timer), glued to an ordinary host
//! `TcpStream`.
//!
//! The guest believes it talks to the remote machine; the gateway plays that machine.
//! Bytes the guest sends are acknowledged once they are in the flow's buffer toward
//! the host and written to the host socket as it takes them; bytes the host socket
//! gives are cut into segments for the guest, kept until the guest acknowledges them,
//! and sent again if it does not (the link to the guest loses frames when the guest's
//! receive ring is full). No byte is held without bound: each direction has a buffer of
//! [`crate::Limits::tcp_buffer`] bytes, and the window advertised to the guest is what
//! is left of its buffer, so a guest that sends faster than the host takes waits.
//!
//! Sequence numbers: ours start at `iss` (the SYN), so data starts at `iss + 1`;
//! `snd_una` is the oldest byte not yet acknowledged, `snd_nxt` the next to send,
//! `snd_max` the furthest ever sent (after a retransmission `snd_nxt` goes back to
//! `snd_una`, and a late acknowledgment may still cover up to `snd_max`). The guest's
//! side is `rcv_nxt`, the next byte expected; data that arrives out of order is
//! dropped and the guest's own retransmission fills the gap.

use crate::wire::{seq_le, seq_lt, tcp_flags::*, Mac, Tcp, TCP_MSS};
use crate::Limits;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

/// The segment size assumed for a guest that sends no MSS option (RFC 1122).
pub const DEFAULT_MSS: usize = 536;
/// Longest wait between two retransmissions.
pub const MAX_RTO: Duration = Duration::from_secs(5);

/// A segment for the guest; the gateway adds the addresses, ports and checksums.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub flags: u8,
    pub seq: u32,
    pub ack: u32,
    pub window: u16,
    pub mss: Option<u16>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The guest sent a SYN; a thread is connecting to the host side. The guest gets
    /// its SYN-ACK once that succeeds, or a reset.
    Connecting,
    /// Connected on the host side, SYN-ACK sent; waiting for the guest's ACK.
    SynReceived,
    /// A host connection came in on a forwarded port; SYN sent to the guest.
    SynSent,
    /// Both sides connected: data flows, until both have closed.
    Established,
}

pub struct Flow {
    /// The guest's end, and the end the guest sees on the other side (the remote host,
    /// or the gateway's address for forwarded ports and the host's loopback).
    pub guest: SocketAddrV4,
    pub peer: SocketAddrV4,
    /// Tells this flow from an earlier one with the same addresses, for the answer of
    /// a connect thread.
    pub id: u64,
    /// The guest machine (MAC address) the flow counts against for the gateway's
    /// limits; `None` for a forwarded connection to a guest not heard from yet.
    pub owner: Option<Mac>,
    pub state: State,
    stream: Option<TcpStream>,

    // Our side of the sequence space.
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    snd_max: u32,
    /// Sequence number of `send_buf[0]`.
    buf_seq: u32,
    /// Bytes from the host, from `snd_una` (or the first unacknowledged data byte)
    /// on: those sent and not acknowledged, then those not sent yet.
    send_buf: VecDeque<u8>,
    /// The host closed its side: a FIN follows the data.
    host_eof: bool,
    fin_sent: bool,
    fin_acked: bool,
    /// The guest's receive window and segment size.
    snd_wnd: u32,
    mss: usize,

    // The guest's side.
    rcv_nxt: u32,
    /// Bytes from the guest not yet written to the host socket.
    to_host: VecDeque<u8>,
    guest_fin: bool,
    /// The host socket's write side is shut down (after the guest's FIN).
    host_shut: bool,
    /// The window in the last segment sent, to tell when it opened enough to say so.
    adv_wnd: u32,

    // Timers.
    rto: Duration,
    /// The first timeout, to go back to once the guest acknowledges again.
    rto_initial: Duration,
    retx_at: Option<Instant>,
    retries: u32,
    pub last_active: Instant,
    buffer: usize,

    /// Finished (closed or reset): the gateway forgets it.
    pub done: bool,
}

fn mss_of(opt: Option<u16>) -> usize {
    (opt.map_or(DEFAULT_MSS, |m| m as usize)).clamp(64, TCP_MSS)
}

impl Flow {
    fn new(
        id: u64,
        guest: SocketAddrV4,
        peer: SocketAddrV4,
        iss: u32,
        now: Instant,
        limits: &Limits,
    ) -> Flow {
        Flow {
            guest,
            peer,
            id,
            owner: None,
            state: State::Connecting,
            stream: None,
            iss,
            snd_una: iss,
            snd_nxt: iss,
            snd_max: iss,
            buf_seq: iss.wrapping_add(1),
            send_buf: VecDeque::new(),
            host_eof: false,
            fin_sent: false,
            fin_acked: false,
            snd_wnd: 0,
            mss: DEFAULT_MSS,
            rcv_nxt: 0,
            to_host: VecDeque::new(),
            guest_fin: false,
            host_shut: false,
            adv_wnd: 0,
            rto: limits.tcp_rto,
            rto_initial: limits.tcp_rto,
            retx_at: None,
            retries: 0,
            last_active: now,
            buffer: limits.tcp_buffer,
            done: false,
        }
    }

    /// A flow for the guest's SYN `syn`, connecting to the host side.
    pub fn from_guest(
        id: u64,
        guest: SocketAddrV4,
        peer: SocketAddrV4,
        syn: &Tcp,
        iss: u32,
        now: Instant,
        limits: &Limits,
    ) -> Flow {
        let mut f = Flow::new(id, guest, peer, iss, now, limits);
        f.rcv_nxt = syn.seq.wrapping_add(1);
        f.snd_wnd = syn.window as u32;
        f.mss = mss_of(syn.mss);
        f
    }

    /// A flow for a host connection accepted on a forwarded port: sends the guest a SYN.
    #[allow(clippy::too_many_arguments)]
    pub fn to_guest(
        id: u64,
        guest: SocketAddrV4,
        peer: SocketAddrV4,
        stream: TcpStream,
        iss: u32,
        now: Instant,
        limits: &Limits,
        out: &mut Vec<Seg>,
    ) -> Flow {
        let mut f = Flow::new(id, guest, peer, iss, now, limits);
        f.stream = Some(stream);
        f.state = State::SynSent;
        f.send_syn(now, out);
        f
    }

    /// The window to advertise: what is left of the buffer toward the host.
    fn window(&self) -> u32 {
        (self.buffer - self.to_host.len()).min(u16::MAX as usize) as u32
    }

    fn seg(&mut self, flags: u8, seq: u32, payload: Vec<u8>) -> Seg {
        self.adv_wnd = self.window();
        Seg {
            flags,
            seq,
            ack: if flags & ACK != 0 { self.rcv_nxt } else { 0 },
            window: self.adv_wnd as u16,
            mss: (flags & SYN != 0).then_some(TCP_MSS as u16),
            payload,
        }
    }

    fn arm(&mut self, now: Instant) {
        if self.retx_at.is_none() {
            self.retx_at = Some(now + self.rto);
        }
    }

    /// SYN (forwarded port) or SYN-ACK (the guest's connection), from `iss`.
    fn send_syn(&mut self, now: Instant, out: &mut Vec<Seg>) {
        let flags = if self.state == State::SynSent {
            SYN
        } else {
            SYN | ACK
        };
        let s = self.seg(flags, self.iss, Vec::new());
        out.push(s);
        self.snd_nxt = self.iss.wrapping_add(1);
        self.snd_max = self.snd_nxt;
        self.arm(now);
    }

    /// The connect thread's answer: on success the guest gets its SYN-ACK, otherwise
    /// a reset (as if the remote refused).
    pub fn connected(&mut self, result: io::Result<TcpStream>, now: Instant, out: &mut Vec<Seg>) {
        if self.state != State::Connecting || self.done {
            return;
        }
        match result {
            Ok(stream) => {
                self.stream = Some(stream);
                self.state = State::SynReceived;
                self.last_active = now;
                self.send_syn(now, out);
            }
            Err(_) => {
                let s = self.seg(RST | ACK, 0, Vec::new());
                out.push(s);
                self.close();
            }
        }
    }

    /// A segment from the guest.
    pub fn on_segment(&mut self, seg: &Tcp, now: Instant, out: &mut Vec<Seg>) {
        if self.done {
            return;
        }
        self.last_active = now;
        if seg.has(RST) {
            // The guest reset the connection (the guest is on a private link with us,
            // so the sequence-number check against blind resets is not needed).
            self.close();
            return;
        }
        match self.state {
            // A retransmitted SYN: it is answered when the host side connects.
            State::Connecting => return,
            State::SynSent => {
                if seg.has(SYN) && seg.has(ACK) && seg.ack == self.iss.wrapping_add(1) {
                    self.rcv_nxt = seg.seq.wrapping_add(1);
                    self.snd_una = seg.ack;
                    self.snd_wnd = seg.window as u32;
                    self.mss = mss_of(seg.mss);
                    self.state = State::Established;
                    self.reset_timer(now);
                    let s = self.seg(ACK, self.snd_nxt, Vec::new());
                    out.push(s);
                    self.output(now, false, out);
                } else if seg.has(ACK) {
                    // Acknowledges something we never sent (RFC 793: reset it).
                    out.push(Seg {
                        flags: RST,
                        seq: seg.ack,
                        ack: 0,
                        window: 0,
                        mss: None,
                        payload: Vec::new(),
                    });
                }
                return;
            }
            State::SynReceived => {
                if seg.has(SYN) {
                    // Our SYN-ACK was lost: the guest sent its SYN again.
                    let s = self.seg(SYN | ACK, self.iss, Vec::new());
                    out.push(s);
                    return;
                }
                if !seg.has(ACK) || seg.ack != self.iss.wrapping_add(1) {
                    return;
                }
                self.state = State::Established;
                self.snd_una = seg.ack;
                self.reset_timer(now);
            }
            State::Established => {
                if seg.has(SYN) {
                    // A duplicate of the guest's SYN: say where we are.
                    let s = self.seg(ACK, self.snd_nxt, Vec::new());
                    out.push(s);
                    return;
                }
            }
        }

        if seg.has(ACK) {
            self.on_ack(seg.ack, seg.window, now);
        }

        // Data and FIN, taken only in order.
        let mut must_ack = false;
        if !seg.payload.is_empty() || seg.has(FIN) {
            must_ack = true;
            let mut data = seg.payload;
            let mut seq = seg.seq;
            if seq_lt(seq, self.rcv_nxt) {
                // Starts with bytes we have: keep only what is new, if anything.
                let old = self.rcv_nxt.wrapping_sub(seq) as usize;
                if old <= data.len() {
                    data = &data[old..];
                    seq = self.rcv_nxt;
                }
            }
            if seq == self.rcv_nxt && !self.guest_fin {
                let take = data.len().min(self.buffer - self.to_host.len());
                self.to_host.extend(&data[..take]);
                self.rcv_nxt = self.rcv_nxt.wrapping_add(take as u32);
                if seg.has(FIN) && take == data.len() {
                    self.guest_fin = true;
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                }
            }
        }
        self.flush_to_host(out);
        if self.done {
            return;
        }
        if must_ack {
            let s = self.seg(ACK, self.snd_nxt, Vec::new());
            out.push(s);
        }
        self.output(now, false, out);
        self.check_done();
    }

    fn reset_timer(&mut self, now: Instant) {
        self.retries = 0;
        self.retx_at = None;
        if self.snd_nxt != self.snd_una {
            self.arm(now);
        }
    }

    /// An acknowledgment and window from the guest.
    fn on_ack(&mut self, ack: u32, window: u16, now: Instant) {
        if seq_lt(self.snd_una, ack) && seq_le(ack, self.snd_max) {
            if seq_lt(self.buf_seq, ack) {
                let n = (ack.wrapping_sub(self.buf_seq) as usize).min(self.send_buf.len());
                self.send_buf.drain(..n);
                self.buf_seq = self.buf_seq.wrapping_add(n as u32);
            }
            if self.fin_sent_ever() && self.send_buf.is_empty() && ack == self.buf_seq.wrapping_add(1) {
                self.fin_acked = true;
            }
            self.snd_una = ack;
            if seq_lt(self.snd_nxt, ack) {
                self.snd_nxt = ack;
            }
            self.rto = self.rto_initial;
            self.reset_timer(now);
        } else if ack == self.snd_una && window == 0 {
            // The guest is there but its buffer is full: not a lost segment.
            self.retries = 0;
        } else if ack == self.snd_una && self.snd_wnd == 0 && self.snd_nxt != self.snd_una {
            // The window opens while a zero-window probe is out. The guest did not take
            // the probe's byte (a full receiver drops it), so sending must start again
            // from `snd_una`: going on from `snd_nxt` would leave a hole that only the
            // retransmission timer fills, at the backed-off timeout. The guest answers,
            // so the timeout starts again from the first one.
            self.snd_nxt = self.snd_una;
            if !self.fin_acked {
                self.fin_sent = false;
            }
            self.rto = self.rto_initial;
            self.reset_timer(now);
        }
        if seq_le(self.snd_una, ack) && seq_le(ack, self.snd_max) {
            self.snd_wnd = window as u32;
        }
    }

    /// Whether a FIN went out at some point (it may be waiting for retransmission).
    fn fin_sent_ever(&self) -> bool {
        self.host_eof && self.snd_max == self.data_end().wrapping_add(1)
    }

    fn data_end(&self) -> u32 {
        self.buf_seq.wrapping_add(self.send_buf.len() as u32)
    }

    /// Sends what the guest's window allows of the data not yet sent, then a FIN if
    /// the host side has closed and everything is out. `probe` lets one byte through a
    /// zero window (the guest's announcement that it opened may be lost).
    fn output(&mut self, now: Instant, probe: bool, out: &mut Vec<Seg>) {
        if self.state != State::Established || self.done {
            return;
        }
        let end = self.data_end();
        let wnd = if probe { self.snd_wnd.max(1) } else { self.snd_wnd };
        let wnd_end = self.snd_una.wrapping_add(wnd);
        while seq_lt(self.snd_nxt, end) && seq_lt(self.snd_nxt, wnd_end) {
            let len = (end.wrapping_sub(self.snd_nxt) as usize)
                .min(wnd_end.wrapping_sub(self.snd_nxt) as usize)
                .min(self.mss);
            let off = self.snd_nxt.wrapping_sub(self.buf_seq) as usize;
            let payload: Vec<u8> = self.send_buf.range(off..off + len).copied().collect();
            let s = self.seg(ACK | PSH, self.snd_nxt, payload);
            out.push(s);
            self.snd_nxt = self.snd_nxt.wrapping_add(len as u32);
        }
        if self.host_eof && !self.fin_sent && self.snd_nxt == end && !self.fin_acked {
            let s = self.seg(FIN | ACK, self.snd_nxt, Vec::new());
            out.push(s);
            self.snd_nxt = self.snd_nxt.wrapping_add(1);
            self.fin_sent = true;
        }
        if seq_lt(self.snd_max, self.snd_nxt) {
            self.snd_max = self.snd_nxt;
        }
        // Something in flight, or data held back by a closed window: the timer runs.
        if self.snd_nxt != self.snd_una || seq_lt(self.snd_nxt, end) {
            self.arm(now);
        }
    }

    /// Writes what the host socket takes of the guest's bytes; after the guest's FIN
    /// and its last byte, shuts the socket's write side. Tells the guest when its
    /// window has opened by a segment or more.
    fn flush_to_host(&mut self, out: &mut Vec<Seg>) {
        let Some(stream) = &mut self.stream else {
            return;
        };
        while !self.to_host.is_empty() {
            let (chunk, _) = self.to_host.as_slices();
            match stream.write(chunk) {
                Ok(0) => break,
                Ok(n) => {
                    self.to_host.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.abort(out);
                    return;
                }
            }
        }
        if self.to_host.is_empty() && self.guest_fin && !self.host_shut {
            let _ = stream.shutdown(Shutdown::Write);
            self.host_shut = true;
        }
        let w = self.window();
        if self.state == State::Established && w >= self.adv_wnd + self.mss as u32 {
            let s = self.seg(ACK, self.snd_nxt, Vec::new());
            out.push(s);
        }
    }

    /// Reads what the host socket has, up to the buffer's room.
    fn read_host(&mut self, now: Instant, out: &mut Vec<Seg>) {
        let Some(stream) = &mut self.stream else {
            return;
        };
        let mut chunk = [0u8; 16 * 1024];
        while !self.host_eof && self.send_buf.len() < self.buffer {
            let want = (self.buffer - self.send_buf.len()).min(chunk.len());
            match stream.read(&mut chunk[..want]) {
                Ok(0) => self.host_eof = true,
                Ok(n) => {
                    self.send_buf.extend(&chunk[..n]);
                    self.last_active = now;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.abort(out);
                    return;
                }
            }
        }
    }

    /// The host side's turn: moves bytes both ways, sends what the window allows, and
    /// runs the timers. `may_read` is false while the gateway's queue toward the guest
    /// is long, so nothing more is read from the host until it drains.
    pub fn service(&mut self, now: Instant, limits: &Limits, may_read: bool, out: &mut Vec<Seg>) {
        if self.done || self.state == State::Connecting {
            return;
        }
        if self.state == State::Established {
            self.flush_to_host(out);
            if may_read && !self.done {
                self.read_host(now, out);
            }
            self.output(now, false, out);
        }
        if self.retx_at.is_some_and(|t| now >= t) {
            self.timeout(now, limits, out);
        }
        let closing = self.guest_fin || self.host_eof || self.state != State::Established;
        let limit = if closing {
            limits.tcp_closing
        } else {
            limits.tcp_idle
        };
        if !self.done && now.saturating_duration_since(self.last_active) >= limit {
            self.abort(out);
        }
        self.check_done();
    }

    /// The retransmission timer went off: send again from `snd_una` (or probe a zero
    /// window), doubling the timeout; give up after [`Limits::tcp_retries`] tries.
    fn timeout(&mut self, now: Instant, limits: &Limits, out: &mut Vec<Seg>) {
        self.retx_at = None;
        if self.snd_nxt == self.snd_una {
            // Nothing in flight, data held back by a zero window: probe it.
            self.output(now, true, out);
            return;
        }
        self.retries += 1;
        if self.retries > limits.tcp_retries {
            self.abort(out);
            return;
        }
        self.rto = (self.rto * 2).min(MAX_RTO);
        match self.state {
            State::Connecting => {}
            State::SynSent | State::SynReceived => self.send_syn(now, out),
            State::Established => {
                self.snd_nxt = self.snd_una;
                if !self.fin_acked {
                    self.fin_sent = false;
                }
                self.output(now, true, out);
            }
        }
        self.arm(now);
    }

    /// Resets the connection on both sides.
    pub fn abort(&mut self, out: &mut Vec<Seg>) {
        if matches!(self.state, State::Established | State::SynReceived) {
            let s = self.seg(RST | ACK, self.snd_nxt, Vec::new());
            out.push(s);
        }
        self.close();
    }

    /// Forgets the connection: the host socket is closed when dropped.
    fn close(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.done = true;
    }

    /// Both directions closed and acknowledged: done.
    fn check_done(&mut self) {
        if self.state == State::Established && self.guest_fin && self.host_shut && self.fin_acked {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// An established flow with no host stream, so nothing it takes from the guest
    /// ever leaves its buffer: what the buffer and the window do is all there is.
    fn established(buffer: usize) -> (Flow, Instant) {
        let limits = Limits {
            tcp_buffer: buffer,
            ..Limits::default()
        };
        let now = Instant::now();
        let guest = SocketAddrV4::new(Ipv4Addr::new(10, 0, 2, 15), 4000);
        let peer = SocketAddrV4::new(Ipv4Addr::new(10, 0, 2, 2), 80);
        let syn = Tcp {
            src_port: 4000,
            dst_port: 80,
            seq: 99,
            ack: 0,
            flags: SYN,
            window: 8192,
            mss: Some(1000),
            payload: &[],
        };
        let mut f = Flow::from_guest(1, guest, peer, &syn, 5000, now, &limits);
        f.state = State::Established;
        f.snd_una = 5001;
        f.snd_nxt = 5001;
        f.snd_max = 5001;
        (f, now)
    }

    fn data(seq: u32, payload: &[u8]) -> Tcp<'_> {
        Tcp {
            src_port: 4000,
            dst_port: 80,
            seq,
            ack: 5001,
            flags: ACK | PSH,
            window: 8192,
            mss: None,
            payload,
        }
    }

    #[test]
    fn guest_bytes_stop_at_the_buffer_and_the_window_shrinks() {
        let (mut f, now) = established(2500);
        let bytes = [7u8; 1000];
        let mut acks = Vec::new();
        let mut seq = 100;
        for _ in 0..4 {
            let mut out = Vec::new();
            f.on_segment(&data(seq, &bytes), now, &mut out);
            let [s] = &out[..] else { panic!("{out:?}") };
            acks.push((s.ack, s.window));
            seq = s.ack;
        }
        // 1000, 1000, then the 500 bytes left of the buffer; then nothing.
        assert_eq!(
            acks,
            [(1100, 1500), (2100, 500), (2600, 0), (2600, 0)],
            "(ack, window) after each segment"
        );
        assert_eq!(f.to_host.len(), 2500);
    }

    #[test]
    fn window_starts_as_the_buffer() {
        let (mut f, now) = established(3000);
        f.state = State::SynReceived;
        let mut out = Vec::new();
        f.send_syn(now, &mut out);
        assert_eq!(out[0].window, 3000);
    }
}
