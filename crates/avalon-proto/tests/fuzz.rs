//! Fuzz-style tests with a fixed-seed PRNG (no crates, reproducible): random and
//! mutated bytes never panic the decoder, never make it allocate past the frame, and
//! whatever does decode re-encodes to exactly the same bytes (the encoding is
//! canonical, so a decoded message means one thing only).

mod common;

use avalon_proto::*;
use common::{samples, Duplex, Rng};

/// Decodes `bytes` and, if that works, checks the canonical-encoding property.
fn check_decode(bytes: &[u8]) -> bool {
    match decode(bytes) {
        Ok((msg, used)) => {
            assert!(used <= bytes.len());
            let again = encode(&msg).unwrap_or_else(|e| panic!("decoded {msg:?} but cannot encode it: {e}"));
            assert_eq!(again, &bytes[..used], "{msg:?} does not re-encode to its bytes");
            true
        }
        Err(_) => false,
    }
}

fn frame(ty: u16, payload: &[u8]) -> Vec<u8> {
    Frame {
        ty,
        payload: payload.to_vec(),
    }
    .to_bytes()
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng::new(0x5eed_0001);
    for _ in 0..200_000 {
        let len = rng.below(64);
        let bytes = rng.bytes(len);
        check_decode(&bytes);
        let _ = read_frame(&mut &bytes[..]);
    }
}

#[test]
fn random_payloads_of_every_type_never_panic() {
    // Random bytes behind a valid header reach the payload decoders, which pure random
    // bytes almost never do. Small payloads and small values make tags and lengths
    // hit their valid ranges often enough to get deep into the messages.
    let mut rng = Rng::new(0x5eed_0002);
    let mut decoded = 0;
    for i in 0..300_000 {
        let (code, _, _) = CATALOGUE[i % CATALOGUE.len()];
        let len = rng.below(48);
        let payload: Vec<u8> = (0..len)
            .map(|_| match rng.below(4) {
                0 => rng.next() as u8,
                _ => rng.below(3) as u8,
            })
            .collect();
        if check_decode(&frame(code, &payload)) {
            decoded += 1;
        }
    }
    // Sanity check that the fuzzer gets past the first field now and then.
    assert!(decoded > 1000, "only {decoded} random payloads decoded");
}

#[test]
fn mutated_frames_never_panic() {
    let mut rng = Rng::new(0x5eed_0003);
    // Keep the big samples out: mutating megabyte frames is slow and adds nothing.
    let seeds: Vec<Vec<u8>> = samples()
        .iter()
        .map(|m| encode(m).unwrap())
        .filter(|b| b.len() < 4096)
        .collect();
    for _ in 0..200_000 {
        let mut bytes = seeds[rng.below(seeds.len())].clone();
        for _ in 0..1 + rng.below(4) {
            match rng.below(5) {
                // Flip a byte.
                0 => {
                    let i = rng.below(bytes.len());
                    bytes[i] ^= 1 << rng.below(8);
                }
                // Set a byte to a random value.
                1 => {
                    let i = rng.below(bytes.len());
                    bytes[i] = rng.next() as u8;
                }
                // Insert a byte.
                2 => {
                    let i = rng.below(bytes.len() + 1);
                    bytes.insert(i, rng.next() as u8);
                }
                // Remove a byte.
                3 if bytes.len() > 1 => {
                    let i = rng.below(bytes.len());
                    bytes.remove(i);
                }
                // Truncate.
                _ => bytes.truncate(rng.below(bytes.len() + 1)),
            }
            if bytes.is_empty() {
                break;
            }
        }
        check_decode(&bytes);
        // Also with the length field fixed up, so mutations reach the payload decoders.
        if bytes.len() >= 6 {
            let fixed = frame(u16::from_le_bytes([bytes[4], bytes[5]]), &bytes[6..]);
            check_decode(&fixed);
        }
    }
}

#[test]
fn random_streams_never_panic_a_connection() {
    // A connection reading garbage keeps going after non-fatal errors and stops at the
    // first fatal one, without panicking or looping forever.
    let mut rng = Rng::new(0x5eed_0004);
    let seeds: Vec<Vec<u8>> = samples()
        .iter()
        .map(|m| encode(m).unwrap())
        .filter(|b| b.len() < 512)
        .collect();
    for _ in 0..5_000 {
        let mut input = Vec::new();
        for _ in 0..rng.below(8) {
            let mut piece = if rng.below(2) == 0 {
                seeds[rng.below(seeds.len())].clone()
            } else {
                let n = rng.below(32);
                rng.bytes(n)
            };
            if !piece.is_empty() && rng.below(3) == 0 {
                let i = rng.below(piece.len());
                piece[i] = rng.next() as u8;
            }
            input.extend(piece);
        }
        let total = input.len();
        let mut conn = Connection::new(Duplex::new(input));
        let mut reads = 0;
        loop {
            reads += 1;
            assert!(reads <= total + 1, "connection made no progress");
            match conn.recv() {
                Ok(_) => {}
                Err(e) if e.is_fatal() => break,
                Err(e) => conn.send_error(&e).unwrap(),
            }
        }
    }
}
