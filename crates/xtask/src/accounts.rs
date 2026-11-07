//! The accounts `cargo xtask fs` writes into a bootable image
//! (docs/excalibur-spec.md, "Users"): `/etc/passwd`, `/etc/group` and
//! `/etc/shadow`, root's and one ordinary user's, `user`, with their home
//! directories. The passwords are hashed as the C library's `pw_hash`
//! hashes them (kernel/user/lib/sha256.c): a salted SHA-256, iterated, a
//! teaching system's hash and not hardened cryptography. [`sha256`] is
//! FIPS 180-4's, written out here as the C one is, since the workspace
//! takes no crates.

/// SHA-256's round constants: the first 32 bits of the fractional parts of
/// the cube roots of the first 64 primes.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The SHA-256 digest of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    // The square roots of the first 8 primes' fractional parts.
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    // The message, a 1 bit, zeros up to 8 bytes short of a block's end, and
    // the length in bits, big-endian.
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0; 32];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// `bytes` in lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The rounds `pw_hash` makes (the C library's `PW_ROUNDS`).
pub const PW_ROUNDS: u32 = 1000;
/// The longest salt (`PW_SALT_MAX`).
pub const PW_SALT_MAX: usize = 16;
/// The longest password `login` reads (its `FIELD_MAX`).
pub const PASSWORD_MAX: usize = 64;

/// `password` hashed with `salt` as `pw_hash` hashes it:
/// `$sha256$ROUNDS$SALT$HEX`, HEX the digest after `rounds` rounds, the
/// first of the salt and the password, each other of the digest before it,
/// the salt and the password.
pub fn pw_hash(password: &str, salt: &str, rounds: u32) -> String {
    let mut digest = sha256(&[salt.as_bytes(), password.as_bytes()].concat());
    for _ in 1..rounds {
        digest = sha256(&[&digest[..], salt.as_bytes(), password.as_bytes()].concat());
    }
    format!("$sha256${rounds}${salt}${}", hex(&digest))
}

/// An account of the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: &'static str,
    pub uid: u32,
    pub gid: u32,
    pub home: &'static str,
    pub password: String,
}

/// The ordinary user's name, uid and gid, and home.
pub const USER: &str = "user";
pub const USER_ID: u32 = 1000;
pub const USER_HOME: &str = "/home/user";
/// The default passwords, which the README and the spec give and `cargo
/// xtask fs --root-password`/`--user-password` change.
pub const ROOT_PASSWORD: &str = "root";
pub const USER_PASSWORD: &str = "user";

/// What a test types on a profile image's console, ahead, to log in as root
/// with the default password before its commands: the name and the password,
/// a line each.
pub fn root_login() -> Vec<u8> {
    format!("root\n{ROOT_PASSWORD}\n").into_bytes()
}

/// What `login` prints on the console for [`root_login`]: its prompts, the
/// name echoed, and not the password (the newline after it is `login`'s own).
pub const ROOT_LOGIN_SHOWN: &str = "login: root\nPassword: \n";

/// The image's two accounts' passwords.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passwords {
    pub root: String,
    pub user: String,
}

impl Default for Passwords {
    fn default() -> Self {
        Passwords {
            root: ROOT_PASSWORD.to_string(),
            user: USER_PASSWORD.to_string(),
        }
    }
}

impl Passwords {
    /// root (uid and gid 0, home `/root`) and `user` (1000, `/home/user`).
    pub fn accounts(&self) -> [Account; 2] {
        [
            Account {
                name: "root",
                uid: 0,
                gid: 0,
                home: "/root",
                password: self.root.clone(),
            },
            Account {
                name: USER,
                uid: USER_ID,
                gid: USER_ID,
                home: USER_HOME,
                password: self.user.clone(),
            },
        ]
    }
}

/// A password's salt: the first 16 hex digits of a SHA-256 of the account
/// and its password, the same for the same inputs, so that the same
/// options build the same image (on Camelot, `passwd` makes a new salt from
/// the time).
fn salt(account: &Account) -> String {
    let seed = format!("camelot salt\n{}\n{}\n", account.name, account.password);
    hex(&sha256(seed.as_bytes()))[..PW_SALT_MAX].to_string()
}

/// `/etc/passwd`: `name:x:uid:gid:gecos:home:shell`, a line each.
pub fn passwd(accounts: &[Account]) -> String {
    accounts
        .iter()
        .map(|a| format!("{}:x:{}:{}:{}:{}:/bin/sh\n", a.name, a.uid, a.gid, a.name, a.home))
        .collect()
}

/// `/etc/group`: `name:x:gid:`, each account's own group.
pub fn group(accounts: &[Account]) -> String {
    accounts
        .iter()
        .map(|a| format!("{}:x:{}:\n", a.name, a.gid))
        .collect()
}

/// `/etc/shadow`: `name:hash:`, a line each.
pub fn shadow(accounts: &[Account]) -> String {
    accounts
        .iter()
        .map(|a| format!("{}:{}:\n", a.name, pw_hash(&a.password, &salt(a), PW_ROUNDS)))
        .collect()
}

/// Checks a password `cargo xtask fs` is given: one `login` can read back,
/// 1 to [`PASSWORD_MAX`] bytes (the longest `login` reads) with no line end
/// (`login` reads a line).
pub fn check_password(what: &str, password: &str) -> Result<(), String> {
    if password.is_empty() || password.len() > PASSWORD_MAX {
        return Err(format!("{what}: a password is 1 to {PASSWORD_MAX} bytes"));
    }
    if password.contains(['\n', '\r']) {
        return Err(format!("{what}: a password can't hold a line end"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        // FIPS 180-4's examples (NIST's "SHA256.pdf"), and the empty message.
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // A million a's: many blocks.
        assert_eq!(
            hex(&sha256(&vec![b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn hashes_and_files() {
        // One round is SHA-256 of the salt and the password.
        assert_eq!(
            pw_hash("bc", "a", 1),
            format!("$sha256$1$a${}", hex(&sha256(b"abc")))
        );
        let two = sha256(&[&sha256(b"abc")[..], b"abc"].concat());
        assert_eq!(pw_hash("bc", "a", 2), format!("$sha256$2$a${}", hex(&two)));
        let accounts = Passwords::default().accounts();
        assert_eq!(
            passwd(&accounts),
            "root:x:0:0:root:/root:/bin/sh\nuser:x:1000:1000:user:/home/user:/bin/sh\n"
        );
        assert_eq!(group(&accounts), "root:x:0:\nuser:x:1000:\n");
        let shadow = shadow(&accounts);
        let lines: Vec<&str> = shadow.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("root:$sha256$1000$") && lines[0].ends_with(':'));
        // The same passwords give the same file; another, another hash.
        assert_eq!(super::shadow(&accounts), shadow);
        let other = Passwords {
            root: "other".into(),
            ..Passwords::default()
        };
        assert_ne!(super::shadow(&other.accounts()), shadow);
        assert!(check_password("--root-password", "").is_err());
        assert!(check_password("--root-password", "a\nb").is_err());
        assert!(check_password("--root-password", "camelot").is_ok());
    }
}
