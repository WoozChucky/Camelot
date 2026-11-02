//! A blocking connection: messages in and out over any `Read + Write`, the version
//! handshake, and request/reply matching.

use crate::frame::{read_frame, write_frame, Frame};
use crate::message::{kind_of, Message, MessageKind};
use crate::{Error, Result, PROTOCOL_VERSION};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};

/// One end of an Avalon connection.
///
/// Requests are answered in order, one reply each, but events (console output, screen
/// frames, state changes) can arrive between them. [`Connection::request`] returns the
/// reply and queues any events it reads on the way, including events that fail to
/// decode (as their errors); [`Connection::recv`] hands those out first, so nothing is
/// lost or reordered.
pub struct Connection<S> {
    stream: S,
    events: VecDeque<Result<Message>>,
    /// Set once `request` read a frame it could not place as either an event or the
    /// reply: from then on replies can no longer be matched to requests.
    out_of_step: bool,
}

impl<S: Read + Write> Connection<S> {
    pub fn new(stream: S) -> Self {
        Connection {
            stream,
            events: VecDeque::new(),
            out_of_step: false,
        }
    }

    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    pub fn into_inner(self) -> S {
        self.stream
    }

    /// Sends one message.
    pub fn send(&mut self, msg: &Message) -> Result<()> {
        write_frame(&mut self.stream, &Frame::from_message(msg)?)
    }

    /// Sends a raw frame, bypassing the message rules (for tests and forwarding).
    pub fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        write_frame(&mut self.stream, frame)
    }

    /// The next message: a queued event (or a queued event's decode error) if there is
    /// one, else the next frame from the stream. On a non-fatal error
    /// ([`Error::is_fatal`]) the bad frame has been consumed and the connection can go
    /// on.
    pub fn recv(&mut self) -> Result<Message> {
        match self.events.pop_front() {
            Some(event) => event,
            None => self.read_message(),
        }
    }

    /// The next frame from the stream, undecoded, so a server that cannot decode it
    /// still knows its type: it answers a bad request (or a frame of unknown type)
    /// with an `Error` reply, but never answers an event, even a bad one
    /// ([`kind_of`]). This bypasses the event queue, which only
    /// [`Connection::request`] fills.
    ///
    /// [`kind_of`]: crate::kind_of
    pub fn recv_frame(&mut self) -> Result<Frame> {
        read_frame(&mut self.stream)
    }

    /// Events read by [`Connection::request`] and not yet returned by `recv`.
    pub fn queued_events(&self) -> usize {
        self.events.len()
    }

    fn read_message(&mut self) -> Result<Message> {
        read_frame(&mut self.stream)?.message()
    }

    /// Sends a request and waits for its reply. An `Error` reply comes back as
    /// [`Error::Remote`]; events that arrive first are queued for [`Connection::recv`],
    /// and so are events that fail to decode, as their errors.
    ///
    /// Replies carry no request id, so this must never take the wrong frame for the
    /// reply. A reply-type frame that fails to decode is the reply, and comes back as
    /// its decode error (non-fatal: the next request gets the next reply). A frame it
    /// cannot place (a request, an unknown type, a too-short frame) gives the fatal
    /// [`Error::OutOfStep`], and every later `request` fails the same way: close the
    /// connection. After any error for which [`Error::is_fatal`] is false, the
    /// connection is still in step.
    pub fn request(&mut self, msg: &Message) -> Result<Message> {
        if self.out_of_step {
            return Err(Error::OutOfStep(
                "an earlier request lost track of its reply".into(),
            ));
        }
        if msg.kind() != MessageKind::Request {
            return Err(Error::Invalid(format!("{} is not a request", msg.name())));
        }
        self.send(msg)?;
        loop {
            let frame = match read_frame(&mut self.stream) {
                Ok(frame) => frame,
                Err(e) if e.is_fatal() => return Err(e),
                Err(e) => return Err(self.lose_step(e)),
            };
            match (kind_of(frame.ty), frame.message()) {
                (Some(MessageKind::Event), event) => self.events.push_back(event),
                (Some(MessageKind::Reply), Ok(Message::Error { code, message })) => {
                    return Err(Error::Remote { code, message })
                }
                (Some(MessageKind::Reply), reply) => return reply,
                (Some(MessageKind::Request), Ok(_)) => {
                    return Err(self.lose_step(Error::Unexpected(frame.ty)))
                }
                (Some(MessageKind::Request) | None, Err(e)) => return Err(self.lose_step(e)),
                (None, Ok(_)) => unreachable!("a frame of unknown type decoded"),
            }
        }
    }

    /// Marks the connection out of step after `cause`, read where a reply was due.
    fn lose_step(&mut self, cause: Error) -> Error {
        self.out_of_step = true;
        Error::OutOfStep(format!("waiting for a reply, got: {cause}"))
    }

    /// Like [`Connection::request`] for requests answered by `Ok`.
    pub fn request_ok(&mut self, msg: &Message) -> Result<()> {
        match self.request(msg)? {
            Message::Ok => Ok(()),
            other => Err(Error::Unexpected(other.type_code())),
        }
    }

    /// Sends `err` to the peer as an `Error` message (what a server does with a request
    /// it cannot serve, or a frame it cannot decode).
    pub fn send_error(&mut self, err: &Error) -> Result<()> {
        let message = match err {
            Error::Remote { message, .. } => message.clone(),
            other => other.to_string(),
        };
        self.send(&Message::Error {
            code: err.code(),
            message,
        })
    }

    /// Client side of the handshake: sends `Hello` and reads `Welcome`. Returns the
    /// server's name. Different versions give [`Error::VersionMismatch`].
    pub fn client_handshake(&mut self, agent: &str) -> Result<String> {
        self.send(&Message::Hello {
            version: PROTOCOL_VERSION,
            agent: agent.to_string(),
        })?;
        match self.read_message()? {
            Message::Welcome { version, server } if version == PROTOCOL_VERSION => Ok(server),
            Message::Welcome { version, .. } => Err(Error::VersionMismatch {
                ours: PROTOCOL_VERSION,
                theirs: version,
            }),
            Message::Error { code, message } => Err(Error::Remote { code, message }),
            other => Err(Error::Unexpected(other.type_code())),
        }
    }

    /// Server side of the handshake: reads `Hello` and answers `Welcome` with our
    /// version. Returns the client's agent name. If the versions differ, the client
    /// still gets the `Welcome` (so it learns ours) and this returns
    /// [`Error::VersionMismatch`]: the caller then closes the connection. Anything but
    /// a valid `Hello` is answered with an `Error` message and returned as an error.
    pub fn server_handshake(&mut self, server: &str) -> Result<String> {
        let hello = match self.read_message() {
            Ok(msg) => msg,
            Err(e) => {
                if !e.is_fatal() {
                    let _ = self.send_error(&e);
                }
                return Err(e);
            }
        };
        match hello {
            Message::Hello { version, agent } => {
                self.send(&Message::Welcome {
                    version: PROTOCOL_VERSION,
                    server: server.to_string(),
                })?;
                if version != PROTOCOL_VERSION {
                    return Err(Error::VersionMismatch {
                        ours: PROTOCOL_VERSION,
                        theirs: version,
                    });
                }
                Ok(agent)
            }
            other => {
                let e = Error::Unexpected(other.type_code());
                let _ = self.send_error(&e);
                Err(e)
            }
        }
    }
}

impl Connection<TcpStream> {
    /// Connects to a server and does the client handshake. `agent` names this client
    /// (`"round-table"`, `"herald"`, ...).
    pub fn connect(addr: impl ToSocketAddrs, agent: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr)?;
        // Requests and keystrokes are small frames; do not wait to batch them.
        stream.set_nodelay(true)?;
        let mut conn = Connection::new(stream);
        conn.client_handshake(agent)?;
        Ok(conn)
    }

    /// A second handle on the same socket, for reading on one thread while writing on
    /// another (Herald). Queued events stay with `self`.
    pub fn try_clone(&self) -> Result<Self> {
        Ok(Connection::new(self.stream.try_clone()?))
    }
}
