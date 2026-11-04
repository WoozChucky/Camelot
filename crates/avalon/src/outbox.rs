//! A connection's outgoing frames: one queue per client, emptied by that client's
//! writer thread.
//!
//! Replies come from the thread reading the client's requests, events from any VM the
//! client is attached to (console output, screen frames, state changes). Both go through this queue,
//! so each frame reaches the socket whole and in the order it was queued, and a VM
//! thread never waits for a slow client's socket.
//!
//! The queue is bounded in bytes ([`MAX_QUEUED`]). A client that falls that far behind
//! on events (it stopped reading while a VM writes to its console) is cut off: its
//! socket is shut down, which ends the connection. Replies instead wait for room, which
//! holds up only that client's own request thread.

use crate::lock;
use avalon_proto::{write_frame, Frame, Message};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Bytes a client may have queued and not yet written before it is considered stuck.
pub const MAX_QUEUED: usize = 8 * 1024 * 1024;

/// One client's outgoing queue.
pub struct Outbox {
    /// The connection's number, unique while the server runs: how VMs tell their
    /// attached clients apart.
    pub id: u64,
    queue: Mutex<Queue>,
    changed: Condvar,
    /// Ends the connection at once (shuts its socket down).
    cut_off: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Frame>,
    /// Bytes in `frames`, headers included.
    bytes: usize,
    /// No more frames are taken; the writer sends what is queued, then stops.
    closed: bool,
    /// The writer has stopped.
    done: bool,
}

fn frame_len(frame: &Frame) -> usize {
    avalon_proto::HEADER_LEN + frame.payload.len()
}

impl Outbox {
    /// A queue for connection `id`. `cut_off` must end the connection, even while the
    /// writer is blocked on it (a socket's `shutdown`).
    pub fn new(id: u64, cut_off: impl Fn() + Send + Sync + 'static) -> Self {
        Outbox {
            id,
            queue: Mutex::default(),
            changed: Condvar::new(),
            cut_off: Box::new(cut_off),
        }
    }

    /// Queues an event. Returns false if the client is gone, or has fallen
    /// [`MAX_QUEUED`] bytes behind and was cut off by this call: either way it should
    /// get no more events.
    pub fn event(&self, frame: &Frame) -> bool {
        let mut q = lock(&self.queue);
        if q.closed {
            return false;
        }
        if q.bytes + frame_len(frame) > MAX_QUEUED {
            q.closed = true;
            q.frames.clear();
            q.bytes = 0;
            drop(q);
            self.changed.notify_all();
            (self.cut_off)();
            return false;
        }
        q.bytes += frame_len(frame);
        q.frames.push_back(frame.clone());
        drop(q);
        self.changed.notify_all();
        true
    }

    /// Queues a reply, first waiting while the queue is over [`MAX_QUEUED`] (the client
    /// is not reading its replies). Dropped if the connection is closing.
    pub fn reply(&self, msg: &Message) {
        let frame = reply_frame(msg);
        let mut q = lock(&self.queue);
        while !q.closed && q.bytes > MAX_QUEUED {
            q = self.changed.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
        self.push(q, frame);
    }

    /// Queues a reply without waiting, for callers that hold a lock a VM thread needs
    /// (the reply to `ConsoleAttach` is queued under its VM's lock, so that no output
    /// slips in between the scrollback and the first `ConsoleOutput`).
    pub fn reply_now(&self, msg: &Message) {
        let frame = reply_frame(msg);
        let q = lock(&self.queue);
        self.push(q, frame);
    }

    fn push(&self, mut q: std::sync::MutexGuard<Queue>, frame: Frame) {
        if q.closed {
            return;
        }
        q.bytes += frame_len(&frame);
        q.frames.push_back(frame);
        drop(q);
        self.changed.notify_all();
    }

    /// No more frames: the writer sends what is queued, then stops; the caller ends
    /// the connection once [`Outbox::wait_done`] says it has.
    pub fn close(&self) {
        lock(&self.queue).closed = true;
        self.changed.notify_all();
    }

    /// Bytes queued and not yet written: how far behind the client is. A VM streaming
    /// its screen holds frames back from a client this far behind (see `vm.rs`).
    pub fn queued(&self) -> usize {
        lock(&self.queue).bytes
    }

    /// Whether [`Outbox::close`] (or a cut-off) happened.
    pub fn is_closed(&self) -> bool {
        lock(&self.queue).closed
    }

    /// Waits up to `timeout` for the writer to stop. Returns whether it has.
    pub fn wait_done(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut q = lock(&self.queue);
        while !q.done {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            q = self
                .changed
                .wait_timeout(q, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// The writer thread's body: writes queued frames to `out` until the queue is
    /// closed and empty, or a write fails. A failed write ends the connection (the
    /// client is gone, so its reading thread must stop too); after the last frame, the
    /// thread that closed the queue ends it, gracefully (`close_gracefully`, so that a
    /// reset over input the server did not read cannot discard those last frames).
    pub fn run_writer(&self, mut out: impl Write) {
        let mut failed = false;
        loop {
            let frame = {
                let mut q = lock(&self.queue);
                loop {
                    if let Some(frame) = q.frames.pop_front() {
                        q.bytes -= frame_len(&frame);
                        break Some(frame);
                    }
                    if q.closed {
                        break None;
                    }
                    q = self.changed.wait(q).unwrap_or_else(PoisonError::into_inner);
                }
            };
            // Room for a waiting reply.
            self.changed.notify_all();
            let Some(frame) = frame else { break };
            if write_frame(&mut out, &frame).is_err() {
                failed = true;
                break;
            }
        }
        {
            let mut q = lock(&self.queue);
            q.closed = true;
            q.frames.clear();
            q.bytes = 0;
            q.done = true;
        }
        self.changed.notify_all();
        if failed {
            (self.cut_off)();
        }
    }
}

/// `msg` as a frame. Replies are small or bounded by the server (scrollback), so this
/// does not fail in practice; if it did, the client still gets exactly one reply.
fn reply_frame(msg: &Message) -> Frame {
    Frame::from_message(msg).unwrap_or_else(|e| {
        Frame::from_message(&Message::Error {
            code: avalon_proto::ErrorCode::Internal,
            message: format!("cannot encode the reply: {e}"),
        })
        .expect("a short Error message encodes")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn counting() -> (Arc<AtomicUsize>, Outbox) {
        let cuts = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&cuts);
        let outbox = Outbox::new(7, move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        (cuts, outbox)
    }

    fn output(n: usize) -> Frame {
        Frame::from_message(&Message::ConsoleOutput {
            vm: 1,
            data: vec![b'x'; n],
        })
        .unwrap()
    }

    #[test]
    fn frames_are_written_in_order_then_the_writer_stops() {
        let (cuts, outbox) = counting();
        outbox.reply(&Message::Ok);
        assert!(outbox.event(&output(3)));
        outbox.reply_now(&Message::Pong { token: 9 });
        assert_eq!(outbox.queued(), 6 + 6 + 8 + 3 + 6 + 4);
        outbox.close();
        assert!(!outbox.event(&output(1)), "closed");
        outbox.reply(&Message::Ok); // dropped
        let mut out = Vec::new();
        outbox.run_writer(&mut out);
        let mut rest = &out[..];
        let mut got = Vec::new();
        while !rest.is_empty() {
            let (msg, n) = avalon_proto::decode(rest).unwrap();
            got.push(msg);
            rest = &rest[n..];
        }
        assert_eq!(
            got,
            [
                Message::Ok,
                Message::ConsoleOutput {
                    vm: 1,
                    data: b"xxx".to_vec()
                },
                Message::Pong { token: 9 }
            ]
        );
        // Not cut off: whoever closed the queue ends the connection, gracefully.
        assert_eq!(cuts.load(Ordering::SeqCst), 0);
        assert!(outbox.wait_done(Duration::ZERO));
    }

    #[test]
    fn a_client_too_far_behind_is_cut_off() {
        let (cuts, outbox) = counting();
        let big = output(1024 * 1024);
        let mut sent = 0;
        while outbox.event(&big) {
            sent += 1;
            assert!(sent < 100, "never cut off");
        }
        assert_eq!(sent, MAX_QUEUED / frame_len(&big));
        assert_eq!(cuts.load(Ordering::SeqCst), 1);
        assert!(outbox.is_closed());
        assert!(!outbox.event(&output(1)));
        assert_eq!(cuts.load(Ordering::SeqCst), 1, "cut off once");
    }

    #[test]
    fn replies_wait_for_room() {
        let (cuts, outbox) = counting();
        let outbox = Arc::new(outbox);
        // Events never take the queue past its limit; a reply queued without waiting
        // can.
        let big = output(1024 * 1024);
        for _ in 0..MAX_QUEUED / frame_len(&big) {
            assert!(outbox.event(&big));
        }
        outbox.reply_now(&Message::ConsoleAttached {
            vm: 1,
            scrollback: vec![0; 1024 * 1024],
        });
        let replier = {
            let outbox = Arc::clone(&outbox);
            std::thread::spawn(move || outbox.reply(&Message::Ok))
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(!replier.is_finished(), "the reply waits while the queue is full");
        let writer = {
            let outbox = Arc::clone(&outbox);
            std::thread::spawn(move || outbox.run_writer(std::io::sink()))
        };
        replier.join().unwrap();
        outbox.close();
        writer.join().unwrap();
        assert!(outbox.wait_done(Duration::from_secs(5)));
        assert_eq!(
            cuts.load(Ordering::SeqCst),
            0,
            "waiting is not a reason to cut off"
        );
    }

    #[test]
    fn a_failed_write_ends_the_writer() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (cuts, outbox) = counting();
        outbox.reply(&Message::Ok);
        outbox.run_writer(Broken);
        assert_eq!(cuts.load(Ordering::SeqCst), 1);
        assert!(outbox.is_closed());
        assert!(!outbox.event(&output(1)));
    }
}
