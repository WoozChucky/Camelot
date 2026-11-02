//! `herald --tcp HOST:PORT`: a raw TCP connection in the terminal, straight to a port
//! and not through Avalon. It is how one reaches a Camelot shell from `shd` (a port
//! forwarded by a NAT switch, `cargo xtask up --demo`'s 127.0.0.1:2323), on Windows
//! too, which has no `telnet` or `nc` by default.
//!
//! A socket has no line discipline at the far end: Camelot's console edits and echoes
//! what is typed, a connection does not. So Herald does it here, as a telnet client in
//! line mode does: [`LineEditor`] keeps the line being typed, echoes it, handles
//! Backspace, Ctrl-U and Ctrl-W, and sends the line, with a newline, on Enter. Ctrl-D
//! on an empty line ends the input (Herald shuts its side of the connection down, which
//! `shd` takes as a hang-up), and Ctrl-] leaves, closing the connection.
//!
//! [`TcpSession`] is the streaming layer, which, like [`crate::Session`], knows nothing
//! of terminals: input bytes come from a channel and output goes to any `Write`, so the
//! tests drive it without one. When the input is not a terminal (a pipe), nothing is
//! edited or echoed: the bytes go as they are, a Ctrl-] among them still leaves, and
//! the end of the input shuts the sending side down; the session then lasts until the
//! far end closes, so a script's output all comes back.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::DETACH_KEY;

/// Most bytes read from the connection at once.
const READ_SIZE: usize = 4096;

/// Ctrl-D: the end of the input, on an empty line.
const EOT: u8 = 0x04;
/// Ctrl-U: erase the line.
const NAK: u8 = 0x15;
/// Ctrl-W: erase the last word.
const ETB: u8 = 0x17;
const BACKSPACE: u8 = 0x08;
const DEL: u8 = 0x7f;
const ESC: u8 = 0x1b;

/// What erases one character on the terminal: back, a blank over it, back again.
const RUB_OUT: &[u8] = b"\x08 \x08";

/// Why a [`TcpSession`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpEnd {
    /// The detach key: Herald left, closing the connection.
    Left,
    /// The far end closed the connection (or reset it).
    Closed,
}

/// What a burst of keys did besides changing the line: why the input stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The detach key.
    Leave,
    /// Ctrl-D on an empty line: the end of the input.
    EndOfInput,
}

/// What [`LineEditor::feed`] made of some keys.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Edited {
    /// For the terminal: the characters typed, rubbed out, or a new line.
    pub echo: Vec<u8>,
    /// For the connection: each line finished with Enter, with its newline.
    pub send: Vec<u8>,
    /// Set when a key stopped the input; the keys after it were not looked at.
    pub stop: Option<Stop>,
}

/// Where the editor is in an escape sequence (an arrow key, say), which it skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    /// After ESC.
    Start,
    /// After ESC [: parameters until a final byte (0x40 to 0x7e).
    Csi,
    /// After ESC O: one more byte.
    Ss3,
}

/// The line being typed, edited locally as a terminal in cooked mode would.
///
/// - Printable characters (UTF-8 included) go into the line and are echoed; a tab is
///   a blank.
/// - Enter (`\r`, `\n`, or `\r\n` as one) sends the line and a newline.
/// - Backspace (DEL or Ctrl-H) erases the last character, Ctrl-U the whole line, Ctrl-W
///   the last word (and the blanks after it).
/// - Ctrl-D on an empty line ends the input; elsewhere it is ignored.
/// - The detach key leaves.
/// - Other control characters and escape sequences (arrow keys, function keys) are
///   ignored: there is no history or cursor movement.
#[derive(Debug, Clone)]
pub struct LineEditor {
    line: Vec<u8>,
    escape: Escape,
    /// The last key was `\r`: a `\n` right after it is the same Enter.
    after_cr: bool,
    /// The byte that leaves (default [`DETACH_KEY`]); `None` for none.
    pub detach_key: Option<u8>,
}

impl Default for LineEditor {
    fn default() -> Self {
        LineEditor::new()
    }
}

impl LineEditor {
    pub fn new() -> LineEditor {
        LineEditor {
            line: Vec::new(),
            escape: Escape::None,
            after_cr: false,
            detach_key: Some(DETACH_KEY),
        }
    }

    /// The line typed so far.
    pub fn line(&self) -> &[u8] {
        &self.line
    }

    /// Takes `keys`, as the terminal gives them.
    pub fn feed(&mut self, keys: &[u8]) -> Edited {
        let mut out = Edited::default();
        for &b in keys {
            let after_cr = std::mem::replace(&mut self.after_cr, false);
            if self.skip_escape(b) {
                continue;
            }
            match b {
                _ if Some(b) == self.detach_key => {
                    out.stop = Some(Stop::Leave);
                    return out;
                }
                b'\n' if after_cr => {}
                b'\r' | b'\n' => {
                    self.after_cr = b == b'\r';
                    out.echo.extend_from_slice(b"\r\n");
                    out.send.append(&mut self.line);
                    out.send.push(b'\n');
                }
                EOT if self.line.is_empty() => {
                    out.stop = Some(Stop::EndOfInput);
                    return out;
                }
                DEL | BACKSPACE => self.rub_out(1, &mut out.echo),
                NAK => self.rub_out(usize::MAX, &mut out.echo),
                ETB => {
                    let chars = self.chars();
                    let blanks = chars.iter().rev().take_while(|c| **c == b' ').count();
                    let word = chars[..chars.len() - blanks]
                        .iter()
                        .rev()
                        .take_while(|c| **c != b' ')
                        .count();
                    self.rub_out(blanks + word, &mut out.echo);
                }
                ESC => self.escape = Escape::Start,
                b'\t' => {
                    self.line.push(b' ');
                    out.echo.push(b' ');
                }
                // Other control characters.
                0..=0x1f => {}
                _ => {
                    self.line.push(b);
                    out.echo.push(b);
                }
            }
        }
        out
    }

    /// Moves through an escape sequence; true if `b` is part of one. A control
    /// character (and the detach key) ends the sequence and counts as itself, as a
    /// terminal executes one met in the middle of a sequence: a lone Esc never
    /// swallows Enter or the detach key, and an ESC starts a new sequence.
    fn skip_escape(&mut self, b: u8) -> bool {
        if self.escape != Escape::None && (b < 0x20 || Some(b) == self.detach_key) {
            self.escape = Escape::None;
            return false;
        }
        match self.escape {
            Escape::None => return false,
            Escape::Start => {
                self.escape = match b {
                    b'[' => Escape::Csi,
                    b'O' => Escape::Ss3,
                    _ => Escape::None,
                }
            }
            Escape::Csi => {
                if (0x40..=0x7e).contains(&b) {
                    self.escape = Escape::None;
                }
            }
            Escape::Ss3 => self.escape = Escape::None,
        }
        true
    }

    /// The first byte of each character of the line, for counting and finding words:
    /// a UTF-8 character's continuation bytes (`10xxxxxx`) are left out.
    fn chars(&self) -> Vec<u8> {
        self.line.iter().copied().filter(|b| b & 0xc0 != 0x80).collect()
    }

    /// Erases the last `n` characters (fewer if the line is shorter), each one byte, or
    /// the bytes of one UTF-8 character, and one column on the terminal.
    fn rub_out(&mut self, n: usize, echo: &mut Vec<u8>) {
        for _ in 0..n {
            let Some(at) = self.line.iter().rposition(|b| b & 0xc0 != 0x80) else {
                return;
            };
            self.line.truncate(at);
            echo.extend_from_slice(RUB_OUT);
        }
    }
}

/// What the session loop waits for: the connection and the input, on one channel.
enum Event {
    Data(Vec<u8>),
    /// The far end closed (or reset) the connection.
    Closed,
    Failed(io::Error),
    Input(Vec<u8>),
    InputEnded,
}

/// A raw TCP connection, with the input edited locally or not.
pub struct TcpSession {
    stream: TcpStream,
    /// Edit and echo lines here ([`LineEditor`]); off, the input goes as it is (but for
    /// the detach key). On for a terminal in raw mode.
    pub edit: bool,
    /// The byte that leaves (default [`DETACH_KEY`]); `None` for none.
    pub detach_key: Option<u8>,
}

impl TcpSession {
    pub fn new(stream: TcpStream) -> TcpSession {
        TcpSession {
            stream,
            edit: false,
            detach_key: Some(DETACH_KEY),
        }
    }

    /// Streams until the detach key or the far end's close: what comes from the
    /// connection goes to `out` (flushed as it comes), and `input` goes to it, edited
    /// (with the echo to `out`) if [`TcpSession::edit`] is on. The end of the input
    /// shuts the sending side down, and the session goes on until the far end closes.
    ///
    /// A failure to write `out`, or of the connection (other than a reset, which is
    /// its end), is an error. `input` is read on a thread of its own, which stays
    /// blocked on it after the session ends until its next byte or its end (a
    /// terminal cannot be interrupted).
    pub fn run(self, input: Receiver<u8>, out: &mut dyn Write) -> io::Result<TcpEnd> {
        let (tx, events) = mpsc::channel();
        let mut reader = self.stream.try_clone()?;
        let server_tx = tx.clone();
        let reading = std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_SIZE];
            loop {
                let event = match reader.read(&mut buf) {
                    Ok(0) => Event::Closed,
                    Ok(n) => Event::Data(buf[..n].to_vec()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if is_reset(&e) => Event::Closed,
                    Err(e) => Event::Failed(e),
                };
                let last = !matches!(event, Event::Data(_));
                if server_tx.send(event).is_err() || last {
                    break;
                }
            }
        });
        std::thread::spawn(move || forward_input(input, tx));

        let result = self.serve(events, out);
        // Unblocks the reading thread, and closes the connection.
        let _ = self.stream.shutdown(Shutdown::Both);
        let _ = reading.join();
        result
    }

    /// The session loop.
    fn serve(&self, events: Receiver<Event>, out: &mut dyn Write) -> io::Result<TcpEnd> {
        let mut editor = LineEditor {
            detach_key: self.detach_key,
            ..LineEditor::new()
        };
        let mut input_done = false;
        let mut stream = &self.stream;
        loop {
            // The channel always has a sender: the reading thread's, until its last
            // event.
            let Ok(event) = events.recv() else {
                return Ok(TcpEnd::Closed);
            };
            match event {
                Event::Data(data) => {
                    out.write_all(&data)?;
                    out.flush()?;
                }
                Event::Closed => return Ok(TcpEnd::Closed),
                Event::Failed(e) => return Err(e),
                // The input has ended (Ctrl-D): the keys go nowhere, but the detach key
                // still leaves, for a far end that does not close.
                Event::Input(keys) if input_done => {
                    if keys.iter().any(|&b| Some(b) == self.detach_key) {
                        return Ok(TcpEnd::Left);
                    }
                }
                Event::InputEnded if input_done => {}
                Event::Input(keys) => {
                    let (send, stop) = if self.edit {
                        let edited = editor.feed(&keys);
                        out.write_all(&edited.echo)?;
                        out.flush()?;
                        (edited.send, edited.stop)
                    } else {
                        match keys.iter().position(|&b| Some(b) == self.detach_key) {
                            Some(at) => (keys[..at].to_vec(), Some(Stop::Leave)),
                            None => (keys, None),
                        }
                    };
                    if !send.is_empty() {
                        if let Err(e) = stream.write_all(&send) {
                            // The far end is gone: its close, or reset, is on its way.
                            if is_reset(&e) {
                                return Ok(TcpEnd::Closed);
                            }
                            return Err(e);
                        }
                    }
                    match stop {
                        Some(Stop::Leave) => return Ok(TcpEnd::Left),
                        Some(Stop::EndOfInput) => {
                            input_done = true;
                            let _ = self.stream.shutdown(Shutdown::Write);
                        }
                        None => {}
                    }
                }
                Event::InputEnded => {
                    input_done = true;
                    let _ = self.stream.shutdown(Shutdown::Write);
                }
            }
        }
    }
}

/// Whether `e` says the far end reset or dropped the connection.
fn is_reset(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe
    )
}

/// Passes input bytes on to the session loop, a burst at a time, then its end.
fn forward_input(input: Receiver<u8>, events: Sender<Event>) {
    while let Ok(first) = input.recv() {
        let mut burst = vec![first];
        while burst.len() < READ_SIZE {
            match input.try_recv() {
                Ok(b) => burst.push(b),
                Err(_) => break,
            }
        }
        if events.send(Event::Input(burst)).is_err() {
            return;
        }
    }
    let _ = events.send(Event::InputEnded);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(keys: &[u8]) -> (LineEditor, Edited) {
        let mut ed = LineEditor::new();
        let edited = ed.feed(keys);
        (ed, edited)
    }

    #[test]
    fn enter_sends_the_line() {
        let (ed, e) = feed(b"ls /bin\r");
        assert_eq!(e.echo, b"ls /bin\r\n");
        assert_eq!(e.send, b"ls /bin\n");
        assert_eq!(e.stop, None);
        assert!(ed.line().is_empty());
        // \n alone, and \r\n as one Enter, even across bursts.
        assert_eq!(feed(b"a\nb\n").1.send, b"a\nb\n");
        let mut ed = LineEditor::new();
        assert_eq!(ed.feed(b"a\r").send, b"a\n");
        let e = ed.feed(b"\nb\r\n\r\n");
        assert_eq!(e.send, b"b\n\n");
        assert_eq!(e.echo, b"b\r\n\r\n");
    }

    #[test]
    fn backspace_ctrl_u_and_ctrl_w_edit_the_line() {
        let (ed, e) = feed(b"echo helo\x7fxx\x08\x08lo");
        assert_eq!(ed.line(), b"echo hello");
        assert_eq!(e.echo, b"echo helo\x08 \x08xx\x08 \x08\x08 \x08lo");
        // A backspace on an empty line does nothing.
        let (ed, e) = feed(b"\x7f\x7fa");
        assert_eq!((ed.line(), &e.echo[..]), (&b"a"[..], &b"a"[..]));
        // Ctrl-U: all of it.
        let (ed, e) = feed(b"abc\x15d\r");
        assert!(ed.line().is_empty());
        assert_eq!(e.send, b"d\n");
        assert_eq!(e.echo, b"abc\x08 \x08\x08 \x08\x08 \x08d\r\n");
        // Ctrl-W: the last word and the blanks after it.
        let (ed, _) = feed(b"cat one two  \x17");
        assert_eq!(ed.line(), b"cat one ");
        let (ed, _) = feed(b"cat one two  \x17\x17\x17\x17");
        assert!(ed.line().is_empty());
    }

    #[test]
    fn a_utf8_character_is_erased_whole() {
        let (ed, e) = feed("olá✓\x7f\x7f!".as_bytes());
        assert_eq!(ed.line(), b"ol!");
        let mut want = "olá✓".as_bytes().to_vec();
        want.extend_from_slice(b"\x08 \x08\x08 \x08!");
        assert_eq!(e.echo, want);
    }

    #[test]
    fn escape_sequences_and_control_characters_are_ignored() {
        // Arrow keys (CSI and SS3), Delete (ESC [ 3 ~), a lone ESC before a letter.
        let (ed, e) = feed(b"a\x1b[A\x1bOB\x1b[3~b\x1bxc\x01\x02\x07\td\r");
        assert_eq!(e.send, b"abc d\n");
        assert_eq!(e.echo, b"abc d\r\n");
        assert!(ed.line().is_empty());
        // A sequence split across bursts.
        let mut ed = LineEditor::new();
        ed.feed(b"x\x1b[1");
        ed.feed(b";5");
        assert_eq!(ed.feed(b"Cy").echo, b"y");
        assert_eq!(ed.line(), b"xy");
    }

    #[test]
    fn a_control_character_ends_an_escape_sequence() {
        // Esc, then the detach key: it still leaves.
        assert_eq!(feed(b"\x1b\x1d").1.stop, Some(Stop::Leave));
        assert_eq!(feed(b"\x1b[1\x1d").1.stop, Some(Stop::Leave));
        // An unfinished sequence does not swallow Enter, nor what follows it.
        let (_, e) = feed(b"\x1b[\rls\r\x1d");
        assert_eq!(e.send, b"\nls\n");
        assert_eq!(e.stop, Some(Stop::Leave));
        assert_eq!(feed(b"a\x1bO\rb\r").1.send, b"a\nb\n");
        // Nor Ctrl-D on an empty line.
        assert_eq!(feed(b"\x1b[\x04").1.stop, Some(Stop::EndOfInput));
        // An ESC in a sequence starts another one.
        let (ed, _) = feed(b"\x1b[1\x1b[Az");
        assert_eq!(ed.line(), b"z");
    }

    #[test]
    fn ctrl_d_ends_the_input_on_an_empty_line_only() {
        let (_, e) = feed(b"ls\x04\r\x04ignored");
        assert_eq!(e.send, b"ls\n");
        assert_eq!(e.stop, Some(Stop::EndOfInput));
        assert_eq!(e.echo, b"ls\r\n");
    }

    #[test]
    fn the_detach_key_leaves_with_the_line_unsent() {
        let (_, e) = feed(b"one\rtwo\x1dthree\r");
        assert_eq!(e.send, b"one\n");
        assert_eq!(e.stop, Some(Stop::Leave));
        let mut ed = LineEditor {
            detach_key: None,
            ..LineEditor::new()
        };
        let e = ed.feed(b"a\x1db\r");
        assert_eq!((e.send, e.stop), (b"ab\n".to_vec(), None));
    }
}
