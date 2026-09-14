use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, RwLock};

use bevy::log::error;
use bevy::prelude::debug;
use byteorder::{ByteOrder, LittleEndian, ReadBytesExt};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use crate::net::blowfish::Blowfish;
use crate::net::codec::SilkroadEncodingOptions;
use crate::net::crc::CRC;
use crate::net::frame::{SilkroadFrame, SilkroadFrameError};
use crate::net::handshake::HandshakeError::{
    AlreadyCompleted, ChallengeFailed, InvalidFrame, InvalidHandshakePacket, NotYetInitialized,
};
use crate::net::security::{SilkroadSecurity, SilkroadSecurityData, SilkroadSecurityState};
use crate::net::sequence::Sequence;

#[derive(Error, Debug)]
pub enum HandshakeError {
    #[error("invalid frame")]
    InvalidFrame,
    #[error("invalid handshake packet")]
    InvalidHandshakePacket,
    #[error("handshake already completed")]
    AlreadyCompleted,
    #[error("handshake challenge failed")]
    ChallengeFailed,
    #[error("handshake has not been initialized yet")]
    NotYetInitialized,
    #[error("handshake body has {actual} byte(s), the flag byte {flags:#04x} requires {expected}")]
    UnexpectedBodySize {
        flags: u8,
        expected: usize,
        actual: usize,
    },
    #[error("handshake challenge arrived without a preceding key exchange (flags {0:#04x})")]
    MissingKeyExchange(u8),
}

/// The handshake flag byte, as the original branches on it
/// (`corpus/client-dec/004b1da0_FUN_004b1da0.c:39,42,47,49`).
pub(crate) const FLAG_BLOWFISH: u8 = 0x02;
pub(crate) const FLAG_SECURITY_BYTES: u8 = 0x04;
pub(crate) const FLAG_KEY_EXCHANGE: u8 = 0x08;
pub(crate) const FLAG_CHALLENGE: u8 = 0x10;

/// Body length a `0x5000` with this flag byte must have, in either phase.
///
/// The handshake is the trust boundary for every later packet, and the reads
/// below are `Bytes::get_*`, which **panic** on a short buffer — so a peer that
/// truncates a frame could kill the client before any validation ran. The
/// original guards the same boundary with an explicit size check and its own
/// log line (`:143-147`).
///
/// Each set flag contributes exactly what its branch reads: blowfish key `u64`
/// (`:40` = 8 bytes), the two EDC seeds (`:44-45` = 2 × `u32`), the key
/// exchange's `u64 + 3 × u32` (`:57`, `setup_handshake` below), and the
/// challenge `u64` (`:84-85`).
pub(crate) fn expected_body_len(flags: u8) -> usize {
    let mut len = 1; // the flag byte itself
    if flags & FLAG_BLOWFISH != 0 {
        len += 8;
    }
    if flags & FLAG_SECURITY_BYTES != 0 {
        len += 8;
    }
    if flags & FLAG_KEY_EXCHANGE != 0 {
        len += 20;
    }
    if flags & FLAG_CHALLENGE != 0 {
        len += 8;
    }
    len
}

/// Reject a `0x5000` body that does not match its own flag byte.
///
/// `setup_flags` is what the *previous* phase announced, the original's
/// accumulator at `+0x200` (`:120,162`). A challenge is only meaningful if a
/// key exchange preceded it, so `FLAG_CHALLENGE` without a prior
/// `FLAG_KEY_EXCHANGE` is rejected — the client-side half of the original's
/// "all of `0x02|0x04|0x08` must have been seen" gate (`:138-141`).
///
/// **Deviation:** we do *not* require `FLAG_BLOWFISH`/`FLAG_SECURITY_BYTES`.
/// That check lives in the original's **server** role (the branch that receives
/// the client's 12-byte reply, `:127-148`), and whether a live v1.188 gateway
/// always sends the full `0x0E` or a subset is an open UNKNOWN — failing a
/// login on an unproven constant would be the worse defect.
pub(crate) fn check_body(data: &[u8], setup_flags: u8) -> Result<u8, HandshakeError> {
    let flags = *data.first().ok_or(HandshakeError::UnexpectedBodySize {
        flags: 0,
        expected: 1,
        actual: 0,
    })?;
    let expected = expected_body_len(flags);
    if data.len() != expected {
        return Err(HandshakeError::UnexpectedBodySize {
            flags,
            expected,
            actual: data.len(),
        });
    }
    if flags & FLAG_CHALLENGE != 0 && setup_flags & FLAG_KEY_EXCHANGE == 0 {
        return Err(HandshakeError::MissingKeyExchange(setup_flags));
    }
    Ok(flags)
}

pub(crate) fn initialize(
    stream: &mut TcpStream,
    security: Arc<RwLock<SilkroadSecurityState>>,
) -> Result<SilkroadSecurityState, HandshakeError> {
    debug!("initializing handshake");
    let sec = security.clone();
    let mut buf = [0; 4096];
    let read_bytes = stream.read(&mut buf).expect("failed to read frame");
    let (_, frame) = SilkroadFrame::parse(&mut buf[..read_bytes], sec.clone())
        .expect("failed to read handshake setup packet");
    match frame {
        SilkroadFrame::Packet { opcode, data, .. } => {
            if opcode != 0x5000 {
                return Err(InvalidHandshakePacket);
            }
            match security.read().expect("lock to be readable").state {
                SilkroadSecurity::None => {
                    // Phase 1 is the first thing an unauthenticated peer sends:
                    // validate its length against its own flag byte before any
                    // `get_*` can panic on a truncated body (#470).
                    check_body(&data, 0)?;
                    match setup_handshake(data, stream) {
                        Ok(state) => Ok(state),
                        Err(_) => Err(InvalidFrame),
                    }
                }
                _ => Err(InvalidHandshakePacket),
            }
        }
        _ => Err(InvalidHandshakePacket),
    }
}

/// Phase 1. The caller has already validated the body against its flag byte
/// ([`check_body`]), so the `get_*` reads below cannot run off the end.
fn setup_handshake(
    mut data: Bytes,
    write: &mut TcpStream,
) -> Result<SilkroadSecurityState, SilkroadFrameError> {
    let flags = data.get_u8();
    let opts = SilkroadEncodingOptions::from(flags);

    let mut sec_data = SilkroadSecurityData::new();
    // The original accumulates the phase-1 flags and gates phase 2 on them
    // (`004b1da0:120,162`); we keep the byte for the same reason.
    sec_data.setup_flags = flags;

    if opts.encryption {
        let handshake_bf_key = data.get_u64_le();
        let bf = Blowfish::new(&handshake_bf_key.to_le_bytes()).expect("invalid blowfish key");
        sec_data.blowfish = Some(bf);
    }

    if opts.edc {
        let sequence_seed = data.get_u32_le();
        let crc_seed = data.get_u32_le();
        debug!("received sequence seed: {}", sequence_seed);
        debug!("received crc seed: {}", crc_seed);
        sec_data.sequence = Sequence::from(sequence_seed);
        sec_data.crc = Box::new(CRC::from(crc_seed));
        sec_data.crc_seed = crc_seed << 8;
        sec_data.sequence_seed = sequence_seed;
    }

    if opts.key_exchange {
        // setup handshake data
        let handshake_key = data.get_u64_le();
        let generator = data.get_u32_le();
        let prime = data.get_u32_le();
        let remote_public = data.get_u32_le();

        // Fresh private exponent per session. The original draws
        // `NextUInt32() & 0x7FFFFFFF` (xBot `Security.cs:589`); the fixed
        // literal this replaces made our public key and the shared secret
        // deterministic for any given set of server parameters.
        let private_exponent = rand::random::<u32>() & 0x7FFF_FFFF;
        let local_public = g_pow_x_mod_p(generator, private_exponent, prime);
        let common_secret = g_pow_x_mod_p(remote_public, private_exponent, prime);

        let new_bf_key = calc_key(common_secret, remote_public, local_public);
        let blowfish = Blowfish::new(&new_bf_key).expect("failed to init new blowfish key");
        let local_challenge = calc_challenge(common_secret, local_public, remote_public);
        let mut lc = BytesMut::new();
        lc.extend_from_slice(&local_challenge);
        blowfish.encrypt(&mut lc);
        let local_challenge = LittleEndian::read_u64(&lc.freeze());

        sec_data.local_public = local_public;
        sec_data.remote_public = remote_public;
        sec_data.common_secret = common_secret;
        sec_data.handshake_key = handshake_key;
        sec_data.generator = generator;
        sec_data.prime = prime;
        sec_data.blowfish = Some(blowfish);
        sec_data.local_challenge = local_challenge;

        let mut frame_data = BytesMut::new();
        frame_data.put_u32_le(local_public);
        frame_data.put_u64_le(local_challenge);

        let frame = SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode: 0x5000,
            encrypted: 0,
            data: frame_data.freeze(),
        };

        let security = SilkroadSecurityState {
            state: SilkroadSecurity::Initialized,
            context: sec_data.clone(),
        };
        let s = Arc::new(RwLock::new(security));
        let mut buf = frame
            .serialize(s.clone())
            .expect("failed to serialize handshake response frame");
        write
            .write(&mut buf)
            .expect("failed to send handshake response data");
        debug!("sent challenge");

        sec_data = s
            .read()
            .expect("security to be freed already")
            .context
            .clone();
    }

    Ok(SilkroadSecurityState {
        state: SilkroadSecurity::Initialized,
        context: sec_data,
    })
}

pub(crate) fn finalize(
    stream: &mut TcpStream,
    security: Arc<RwLock<SilkroadSecurityState>>,
) -> Result<SilkroadSecurityState, HandshakeError> {
    debug!("finalizing handshake");
    let sec = security.clone();
    let mut buf = [0; 4096];
    let read_bytes = stream.read(&mut buf).expect("failed to read frame");
    let (_, frame) = SilkroadFrame::parse(&mut buf[..read_bytes], sec.clone())
        .expect("failed to read handshake setup packet");
    return match frame {
        SilkroadFrame::Packet { opcode, data, .. } => {
            if opcode != 0x5000 {
                return Err(InvalidHandshakePacket);
            }
            let mut security = security.write().expect("security to be writable");
            match security.state {
                SilkroadSecurity::None => Err(NotYetInitialized),
                SilkroadSecurity::Established => Err(AlreadyCompleted),
                SilkroadSecurity::Initialized => derive_final_key(data, stream, &mut security),
            }
        }
        _ => Err(InvalidHandshakePacket),
    };
}

fn derive_final_key(
    data: Bytes,
    write: &mut TcpStream,
    security: &mut SilkroadSecurityState,
) -> Result<SilkroadSecurityState, HandshakeError> {
    // Trust boundary (#470): the challenge decides whether we talk to the peer
    // that derived our shared secret, so a body that does not match its own
    // flag byte — or a challenge with no key exchange behind it — is a hard
    // error, not a short read that panics inside `read_u64`.
    let enc_opt = check_body(&data, security.context.setup_flags)?;
    let mut reader = data.reader();
    let _ = reader.read_u8();
    let opts = SilkroadEncodingOptions::from(enc_opt);

    let sec_data = &security.context;
    if opts.key_challenge {
        let remote_challenge = reader
            .read_u64::<LittleEndian>()
            .expect("failed to read remote challenge");
        // The server's signature is the MIRROR of ours: it concatenates
        // `remote_public ‖ local_public` and keys the transform on
        // `remote_public & 7`, where our own signature used
        // `local_public ‖ remote_public` with `local_public & 7`
        // (xBot `Security.cs:608-615`).
        //
        // This used to recompute our *own* signature and compare it against
        // our own stored copy — identical inputs, so the comparison could
        // never fail and the server was never actually authenticated.
        let expected = server_challenge(sec_data);
        if remote_challenge != expected {
            error!(
                "server signature error: expected = {:X}, received = {:X}",
                expected, remote_challenge
            );
            return Err(ChallengeFailed);
        }

        let final_key = key_transform_value(
            Bytes::copy_from_slice(&security.context.handshake_key.to_le_bytes()),
            security.context.common_secret,
            3,
        );
        security.context.blowfish =
            Some(Blowfish::new(&final_key).expect("blowfish to be initialized with final key"));
    }

    let new_frame = SilkroadFrame::Packet {
        count: 0,
        crc: 0,
        opcode: 0x9000,
        encrypted: 0,
        data: Bytes::new(),
    };

    let new_security = SilkroadSecurityState {
        state: SilkroadSecurity::Established,
        context: security.context.clone(),
    };
    let s = Arc::new(RwLock::new(new_security));
    let buf = new_frame
        .serialize(Arc::clone(&s))
        .expect("failed to serialize handshake completed frame");
    write
        .write(&buf)
        .expect("failed to send handshake completed data");

    return Ok(SilkroadSecurityState {
        state: SilkroadSecurity::Established,
        context: s.read().expect("security to be readable").context.clone(),
    });
}

pub fn calc_key(common_secret: u32, secret1: u32, secret2: u32) -> Bytes {
    let s1s2 = concat_u32_as_bytes(secret1, secret2);
    key_transform_value(s1s2, common_secret, (common_secret & 3) as u8)
}

/// The challenge value we expect the server to send back, proving it derived
/// the same shared secret: `blowfish(key_transform(A‖B, K, A & 7))` with
/// `A = remote_public`, `B = local_public` — the mirror of the signature we
/// sent (xBot `Security.cs:608-615`).
///
/// Uses the handshake blowfish (keyed from [`calc_key`]), which is still the
/// active one at this point; the final session key is derived only after this
/// check passes.
pub(crate) fn server_challenge(sec_data: &SilkroadSecurityData) -> u64 {
    let challenge = calc_challenge(
        sec_data.common_secret,
        sec_data.remote_public,
        sec_data.local_public,
    );
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&challenge);
    sec_data
        .blowfish
        .clone()
        .expect("blowfish to be setup")
        .encrypt(&mut buf);
    LittleEndian::read_u64(&buf.freeze())
}

pub fn calc_challenge(common_secret: u32, secret1: u32, secret2: u32) -> Bytes {
    let s1s2 = concat_u32_as_bytes(secret1, secret2);
    key_transform_value(s1s2, common_secret, (secret1 & 7) as u8)
}

fn concat_u32_as_bytes(a: u32, b: u32) -> Bytes {
    let a_bytes: [u8; 4] = a.to_le_bytes();
    let b_bytes: [u8; 4] = b.to_le_bytes();

    let bytes = &[
        a_bytes[0], a_bytes[1], a_bytes[2], a_bytes[3], b_bytes[0], b_bytes[1], b_bytes[2],
        b_bytes[3],
    ];

    return Bytes::copy_from_slice(bytes);
}

fn key_transform_value(v: Bytes, key: u32, key_byte: u8) -> Bytes {
    let mut value = BytesMut::new();
    value.extend_from_slice(&v);
    let key_byte = key_byte as u32;
    value[0] ^= (value[0] as u32 + ((key >> 0) & 0xFF_u32) + key_byte) as u8;
    value[1] ^= (value[1] as u32 + ((key >> 8) & 0xFF_u32) + key_byte) as u8;
    value[2] ^= (value[2] as u32 + ((key >> 16) & 0xFF_u32) + key_byte) as u8;
    value[3] ^= (value[3] as u32 + ((key >> 24) & 0xFF_u32) + key_byte) as u8;

    value[4] ^= (value[4] as u32 + ((key >> 0) & 0xFF_u32) + key_byte) as u8;
    value[5] ^= (value[5] as u32 + ((key >> 8) & 0xFF_u32) + key_byte) as u8;
    value[6] ^= (value[6] as u32 + ((key >> 16) & 0xFF_u32) + key_byte) as u8;
    value[7] ^= (value[7] as u32 + ((key >> 24) & 0xFF_u32) + key_byte) as u8;

    return value.freeze();
}

pub fn g_pow_x_mod_p(generator: u32, private: u32, prime: u32) -> u32 {
    let prime = prime as i64;
    if prime <= 1 {
        return 0;
    }
    let mut result: i64 = 1;
    let mut p = private;
    // Reduce before the first squaring. `generator` comes straight off the
    // wire, and a peer sending g > 0xB504F333 would otherwise overflow the
    // i64 in `mult * mult` — a panic in debug builds, silent wraparound in
    // release. Every later value is already < prime, and for a well-formed
    // g < p this reduction is a no-op.
    let mut mult = (generator as i64) % prime;

    while p != 0 {
        if (p & 1) > 0 {
            result = (mult * result) % prime;
        }
        p >>= 1;
        mult = (mult * mult) % prime;
    }
    result as u32
}

#[cfg(test)]
mod test {
    use super::*;

    /// A session mid-handshake: the client has computed B and K from the
    /// server's g/p/A and keyed the handshake blowfish.
    fn session(
        generator: u32,
        prime: u32,
        remote_public: u32,
        private_exponent: u32,
    ) -> SilkroadSecurityData {
        let local_public = g_pow_x_mod_p(generator, private_exponent, prime);
        let common_secret = g_pow_x_mod_p(remote_public, private_exponent, prime);
        let bf_key = calc_key(common_secret, remote_public, local_public);

        let mut data = SilkroadSecurityData::new();
        data.local_public = local_public;
        data.remote_public = remote_public;
        data.common_secret = common_secret;
        data.blowfish = Some(Blowfish::new(&bf_key).expect("valid blowfish key"));
        data
    }

    /// Our own signature, as sent in the 0x5000 response.
    fn client_signature(sec: &SilkroadSecurityData) -> u64 {
        let challenge = calc_challenge(sec.common_secret, sec.local_public, sec.remote_public);
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&challenge);
        sec.blowfish
            .clone()
            .expect("blowfish to be setup")
            .encrypt(&mut buf);
        LittleEndian::read_u64(&buf.freeze())
    }

    /// The server's challenge is the MIRROR of the client's signature — it
    /// concatenates `remote‖local` and keys on `remote & 7`, not `local‖remote`
    /// on `local & 7`. Comparing the two used to be the whole "validation",
    /// which made it a tautology that could never fail (#248).
    #[test]
    fn server_challenge_is_not_the_client_signature() {
        let sec = session(0xF2E1_5D3A, 0x7FFF_FFC3, 0x1234_5678, 0x0BAD_C0DE);
        assert_ne!(
            server_challenge(&sec),
            client_signature(&sec),
            "the two signatures must differ, or authentication is vacuous"
        );
    }

    /// The expected value matches the original's formula:
    /// `blowfish(key_transform(A‖B, K, A & 7))` (xBot `Security.cs:608-615`).
    #[test]
    fn server_challenge_follows_the_original_formula() {
        let sec = session(0xF2E1_5D3A, 0x7FFF_FFC3, 0x1234_5678, 0x0BAD_C0DE);

        let mut expected = BytesMut::new();
        expected.extend_from_slice(&key_transform_value(
            concat_u32_as_bytes(sec.remote_public, sec.local_public),
            sec.common_secret,
            (sec.remote_public & 7) as u8,
        ));
        sec.blowfish
            .clone()
            .expect("blowfish to be setup")
            .encrypt(&mut expected);

        assert_eq!(
            server_challenge(&sec),
            LittleEndian::read_u64(&expected.freeze())
        );
    }

    /// A server that cannot derive the same shared secret produces a different
    /// challenge, and must be rejected. Flipping any single bit is enough.
    #[test]
    fn a_tampered_challenge_does_not_match() {
        let sec = session(0xF2E1_5D3A, 0x7FFF_FFC3, 0x1234_5678, 0x0BAD_C0DE);
        let expected = server_challenge(&sec);
        for bit in 0..64 {
            assert_ne!(expected, expected ^ (1u64 << bit), "bit {bit}");
        }
        // and a session with a different secret yields a different challenge
        let other = session(0xF2E1_5D3A, 0x7FFF_FFC3, 0x1234_5678, 0x0BAD_C0DF);
        assert_ne!(server_challenge(&other), expected);
    }

    /// The private exponent must vary per session — it used to be the literal
    /// 123, making our public key and shared secret deterministic.
    #[test]
    fn private_exponent_is_drawn_per_session() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let x = rand::random::<u32>() & 0x7FFF_FFFF;
            assert!(x <= 0x7FFF_FFFF, "must fit the original's 31-bit mask");
            seen.insert(x);
        }
        assert!(seen.len() > 1, "exponent must not be constant");
    }

    /// `generator` is read straight off the wire. A peer sending a value
    /// above 0xB504F333 used to overflow the i64 in `mult * mult` — panic in
    /// debug, silent wraparound in release — before any of the handshake's
    /// own validation could run.
    #[test]
    fn modpow_survives_a_generator_larger_than_the_prime() {
        let prime = 0x7FFF_FFC3u32;
        let huge = 0xF2E1_5D3Au32;
        // no panic, and the result is the properly reduced one
        assert_eq!(
            g_pow_x_mod_p(huge, 0x0BAD_C0DE, prime),
            g_pow_x_mod_p(huge % prime, 0x0BAD_C0DE, prime)
        );
        // a degenerate prime must not divide by zero either
        assert_eq!(g_pow_x_mod_p(huge, 5, 0), 0);
        assert_eq!(g_pow_x_mod_p(huge, 5, 1), 0);
    }

    /// Sanity: the modpow still agrees with a plain reference implementation.
    #[test]
    fn modpow_matches_a_reference_implementation() {
        let reference = |g: u32, x: u32, p: u32| -> u32 {
            let mut acc: u64 = 1;
            for _ in 0..x {
                acc = (acc * g as u64) % p as u64;
            }
            acc as u32
        };
        for (g, x, p) in [(5u32, 17u32, 23u32), (7, 11, 1009), (2, 20, 65_537)] {
            assert_eq!(
                g_pow_x_mod_p(g, x, p),
                reference(g, x, p),
                "{g}^{x} mod {p}"
            );
        }
    }

    /// #470: the handshake is the trust boundary for every later packet, and
    /// the body reads panic on a short buffer. The original guards it with an
    /// explicit size check plus its own log line (`004b1da0:143-147`); a body
    /// that does not match its own flag byte must be rejected.
    #[test]
    fn a_handshake_body_must_match_its_flag_byte() {
        // Phase 1, full flags 0x0E: 1 + 8 (blowfish) + 8 (edc seeds) + 20 (kex).
        assert_eq!(
            expected_body_len(FLAG_BLOWFISH | FLAG_SECURITY_BYTES | FLAG_KEY_EXCHANGE),
            37
        );
        // Phase 2, challenge only: 1 + 8.
        assert_eq!(expected_body_len(FLAG_CHALLENGE), 9);

        let full = FLAG_BLOWFISH | FLAG_SECURITY_BYTES | FLAG_KEY_EXCHANGE;
        let mut body = vec![full];
        body.extend_from_slice(&[0u8; 36]);
        assert!(check_body(&body, 0).is_ok());

        // one byte short, one byte long, and empty
        assert!(matches!(
            check_body(&body[..36], 0),
            Err(HandshakeError::UnexpectedBodySize {
                expected: 37,
                actual: 36,
                ..
            })
        ));
        body.push(0);
        assert!(matches!(
            check_body(&body, 0),
            Err(HandshakeError::UnexpectedBodySize {
                expected: 37,
                actual: 38,
                ..
            })
        ));
        assert!(matches!(
            check_body(&[], 0),
            Err(HandshakeError::UnexpectedBodySize { actual: 0, .. })
        ));
    }

    /// The challenge only proves anything if a key exchange produced a shared
    /// secret first — the client-side half of the original's
    /// "all of 0x02|0x04|0x08 must have been seen" gate (`:138-141`).
    #[test]
    fn a_challenge_without_a_prior_key_exchange_is_rejected() {
        let mut body = vec![FLAG_CHALLENGE];
        body.extend_from_slice(&[0u8; 8]);

        assert!(matches!(
            check_body(&body, 0),
            Err(HandshakeError::MissingKeyExchange(0))
        ));
        assert!(matches!(
            check_body(&body, FLAG_BLOWFISH | FLAG_SECURITY_BYTES),
            Err(HandshakeError::MissingKeyExchange(_))
        ));
        // with the key exchange seen in phase 1 it is accepted
        assert_eq!(
            check_body(
                &body,
                FLAG_BLOWFISH | FLAG_SECURITY_BYTES | FLAG_KEY_EXCHANGE
            )
            .unwrap(),
            FLAG_CHALLENGE
        );
    }
}
