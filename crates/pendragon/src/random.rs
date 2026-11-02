//! Random bytes from the operating system, for Pendragon's per-launch token: the
//! system's cryptographic generator through hand-written FFI on Windows
//! (`BCryptGenRandom`), `/dev/urandom` elsewhere. Never a seeded generator of our own:
//! the token is what keeps other web pages from driving the console.

use std::io;

/// Fills `buf` with random bytes from the OS.
pub fn fill(buf: &mut [u8]) -> io::Result<()> {
    sys::fill(buf)
}

/// Bytes in a token: 256 bits.
pub const TOKEN_BYTES: usize = 32;

/// A fresh token: [`TOKEN_BYTES`] random bytes as lowercase hex.
pub fn token() -> io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::io;

    /// Use the system's preferred generator; no algorithm handle is passed.
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;

    #[link(name = "bcrypt")]
    extern "system" {
        /// `NTSTATUS BCryptGenRandom(BCRYPT_ALG_HANDLE, PUCHAR, ULONG, ULONG)`.
        fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, len: u32, flags: u32) -> i32;
    }

    pub fn fill(buf: &mut [u8]) -> io::Result<()> {
        // ULONG lengths: ask in pieces if a caller ever wants more than 4 GiB.
        for chunk in buf.chunks_mut(u32::MAX as usize) {
            // SAFETY: the buffer is valid for writes of its length, and the flag says
            // there is no algorithm handle.
            let status = unsafe {
                BCryptGenRandom(
                    std::ptr::null_mut(),
                    chunk.as_mut_ptr(),
                    chunk.len() as u32,
                    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                )
            };
            // NTSTATUS: negative is an error.
            if status < 0 {
                return Err(io::Error::other(format!(
                    "BCryptGenRandom failed with status {:#010x}",
                    status as u32
                )));
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
mod sys {
    use std::io::{self, Read};

    pub fn fill(buf: &mut [u8]) -> io::Result<()> {
        std::fs::File::open("/dev/urandom")?.read_exact(buf)
    }
}

#[cfg(not(any(windows, unix)))]
mod sys {
    use std::io;

    pub fn fill(_: &mut [u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no OS random source on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let a = token().unwrap();
        let b = token().unwrap();
        assert_eq!(a.len(), 2 * TOKEN_BYTES);
        assert!(a
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }

    #[test]
    fn bytes_look_random() {
        // Not a statistical test, only a check that the buffer is really written:
        // 4 KiB of OS randomness has every byte value with overwhelming probability,
        // and a zeroed or constant buffer has one.
        let mut buf = vec![0u8; 4096];
        fill(&mut buf).unwrap();
        let mut seen = [false; 256];
        for &b in &buf {
            seen[b as usize] = true;
        }
        assert!(seen.iter().filter(|&&s| s).count() > 200);
        fill(&mut []).unwrap();
    }
}
