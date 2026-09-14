use std::fmt::{Display, Formatter};
use std::sync::{Arc, RwLock};

use bevy::log::debug;
use byteorder::{ByteOrder, LittleEndian};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use packets::Packet;
use thiserror::Error;

use crate::net::frame::SilkroadFrameError::{
    EncryptionBufferInvalidSize, Incomplete, InvalidMassiveHeaderFlag, TruncatedMassiveFrame,
};
use crate::net::security::{SilkroadSecurity, SilkroadSecurityState};

const BLOWFISH_BLOCK_SIZE: usize = 8;

#[derive(Debug)]
pub enum SilkroadFrame {
    Packet {
        count: u8,
        crc: u8,
        opcode: u16,
        encrypted: u8,
        data: Bytes,
    },
    MassiveHeader {
        count: u8,
        crc: u8,
        opcode: u16,
        amount: u16,
        data: Bytes,
    },
    MassivePayload {
        count: u8,
        crc: u8,
        inner: Bytes,
    },
}

#[derive(Error, Debug)]
pub enum SilkroadFrameError {
    #[error("i/o error occurred during en/decoding")]
    IoError(#[from] std::io::Error),
    #[error("transmitted data is still incomplete")]
    Incomplete,
    #[error("encryption buffer has invalid size: {0}. Must be a factor of 8")]
    EncryptionBufferInvalidSize(usize),
    #[error("invalid header flag in massive packet: {0}")]
    InvalidMassiveHeaderFlag(u8),
    #[error("massive packet body has {0} byte(s), needs {1}")]
    TruncatedMassiveFrame(usize, usize),
}

impl Display for SilkroadFrame {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SilkroadFrame::Packet {
                opcode,
                encrypted,
                data,
                ..
            } => f.write_fmt(format_args!(
                "Packet[{:#X}][encrypted: {}]: {:X}",
                opcode, encrypted, data
            )),
            SilkroadFrame::MassiveHeader { opcode, amount, .. } => f.write_fmt(format_args!(
                "MassiveHeader[{:#X}] amount: {}",
                opcode, amount
            )),
            SilkroadFrame::MassivePayload { count, .. } => {
                f.write_fmt(format_args!("MassivePayload: {}", count))
            }
        }
    }
}

impl SilkroadFrame {
    pub fn serialize(
        &self,
        security: Arc<RwLock<SilkroadSecurityState>>,
    ) -> Result<Bytes, SilkroadFrameError> {
        let mut buf = BytesMut::with_capacity(4096);
        let mut security = security.write().expect("lock is available");
        match self {
            SilkroadFrame::Packet {
                opcode,
                data,
                encrypted,
                ..
            } => {
                let mut size = data.len() as u16;
                if *encrypted == 1 {
                    size |= 1 << 15
                }
                buf.put_u16_le(size);
                buf.put_u16_le(*opcode);
                buf.put_u8(security.context.sequence.next());
                buf.put_u8(0);
                buf.put_slice(data);

                let crc = security.context.crc.calculate(&buf[0..6 + data.len()]);
                buf[5] = crc;
                if *encrypted == 1 {
                    let enc_len = (data.len() + 4 + 7) & (-8_isize as usize);
                    if enc_len + 2 > 4096 {
                        return Err(EncryptionBufferInvalidSize(enc_len + 2));
                    }
                    if 2 + enc_len > buf.len() {
                        // extend buffer when not big enough
                        let diff = 2 + enc_len - buf.len();
                        buf.extend_from_slice(&BytesMut::zeroed(diff).freeze());
                    }
                    let encrypt_buf = &mut buf[2..2 + enc_len];
                    let bf = security
                        .context
                        .blowfish
                        .expect("security must be initialized before encryption");
                    bf.encrypt(encrypt_buf);
                }
            }
            _ => todo!("massive packet handling"),
        }
        Ok(buf.freeze())
    }

    pub fn parse(
        data: &mut [u8],
        security: Arc<RwLock<SilkroadSecurityState>>,
    ) -> Result<(usize, SilkroadFrame), SilkroadFrameError> {
        if data.len() < 4 {
            // info!("not enough data: {}", data.len());
            return Err(Incomplete);
        }

        let len = LittleEndian::read_u16(&data[0..2]);
        let encrypted = (len & 0x8000) != 0;
        let content_len = (len & 0x7FFF) as usize;
        let data_without_size = &mut data[2..];
        let total_size = if encrypted {
            let given_length = content_len + 4;
            let given_length = (given_length + 7) & (-8isize as usize);
            let aligned_length = given_length % BLOWFISH_BLOCK_SIZE;
            if aligned_length == 0 {
                // Already block-aligned, no need to pad
                given_length
            } else {
                given_length + (8 - aligned_length) // Add padding
            }
        } else {
            content_len + 4
        };

        if data_without_size.len() < total_size {
            debug!(
                "data smaller than total size: {} < {}",
                data_without_size.len(),
                total_size
            );
            return Err(Incomplete);
        }

        if encrypted {
            let sec = security.read().expect("lock is held by current thread");
            match sec.state {
                SilkroadSecurity::Established => {
                    let bf = sec
                        .context
                        .blowfish
                        .expect("handshake did not set a blowfish instance");
                    let data = &mut data_without_size[0..total_size];
                    bf.decrypt(data);
                }
                _ => {}
            }
        }

        let opcode = LittleEndian::read_u16(&data_without_size[0..2]);
        let count = data_without_size[2];
        let crc = data_without_size[3];

        // `content_len` is the body length the server wrote on the wire. What
        // follows it in the buffer is not ours: an encrypted frame is padded up
        // to the blowfish block (up to 7 bytes), and a single read can carry the
        // next frame right behind this one. Slicing to the end of the buffer
        // over-reports both, so bind the body once, here.
        let body = data_without_size
            .get(4..4 + content_len)
            .ok_or(Incomplete)?;
        let body = Bytes::copy_from_slice(body);

        if opcode == 0x600D {
            let mut data = body;
            // `Buf::get_*` panics on underflow, and a desynchronised stream
            // reaches here with arbitrary bytes — a short body has to become a
            // recoverable parse error so `next_packet` can resynchronise.
            if data.is_empty() {
                return Err(TruncatedMassiveFrame(0, 1));
            }
            let header_flag = data.get_u8();

            return match header_flag {
                0 => {
                    // body
                    Ok((
                        total_size,
                        SilkroadFrame::MassivePayload {
                            count,
                            crc,
                            inner: data,
                        },
                    ))
                }
                1 => {
                    // header: amount + inner opcode, two u16 each
                    if data.remaining() < 4 {
                        return Err(TruncatedMassiveFrame(data.remaining(), 4));
                    }
                    let amount = data.get_u16_le();
                    let inner_opcode = data.get_u16_le();
                    Ok((
                        total_size,
                        SilkroadFrame::MassiveHeader {
                            count,
                            crc,
                            opcode: inner_opcode,
                            amount,
                            data,
                        },
                    ))
                }
                _ => Err(InvalidMassiveHeaderFlag(header_flag)),
            };
        }

        let frame = SilkroadFrame::Packet {
            encrypted: if encrypted { 1 } else { 0 },
            opcode,
            crc,
            count,
            data: body,
        };

        if encrypted {
            Ok((total_size - 4, frame))
        } else {
            Ok((total_size, frame))
        }
    }

    /// On-wire byte length of the frame at the start of `buf`: the 2-byte
    /// length prefix plus its content (blowfish-padded when encrypted). `None`
    /// if `buf` is too short to hold the length prefix.
    ///
    /// [`parse`](Self::parse) decrypts in place and reports only a
    /// decrypt-relative size, so the receive loop uses this to walk multiple
    /// frames packed into a single read.
    /// Whether the frame at the front of `buf` carries the wire `0x8000`
    /// encryption bit. Read from the always-plaintext size prefix, so it is
    /// valid *before* [`SilkroadFrame::parse`] decrypts the buffer in place —
    /// which is the only place it survives for a `MassiveHeader`, whose parsed
    /// form drops the flag.
    pub fn wire_is_encrypted(buf: &[u8]) -> Option<bool> {
        if buf.len() < 2 {
            return None;
        }
        Some(LittleEndian::read_u16(&buf[0..2]) & 0x8000 != 0)
    }

    pub fn wire_len(buf: &[u8]) -> Option<usize> {
        if buf.len() < 2 {
            return None;
        }
        let len = LittleEndian::read_u16(&buf[0..2]);
        let encrypted = (len & 0x8000) != 0;
        let content_len = (len & 0x7FFF) as usize;
        // Mirror `parse`: encrypted frames pad the 4-byte header + body up to
        // the 8-byte blowfish block.
        let total_size = if encrypted {
            (content_len + 4 + 7) & !7usize
        } else {
            content_len + 4
        };
        Some(2 + total_size)
    }
}

impl Into<SilkroadFrame> for Packet {
    fn into(self) -> SilkroadFrame {
        let (opcode, data) = self.into_serialize();
        SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode,
            encrypted: 0,
            data,
        }
    }
}

/// The **login/identity** opcodes the original Blowfish-encrypts on the way
/// out, gated behind `network_settings.outbound_encryption`.
///
/// The original decides per opcode: every gateway-phase message is allocated
/// through the `CNetEngine` vtable slot `+0x48`, whose second boolean argument
/// is the outbound-encrypt flag. A negotiated Blowfish key is therefore
/// necessary but *not* sufficient — most gameplay traffic is deliberately
/// cleartext, and encrypting everything would drift just as far the other way.
///
/// Entry provenance (`[S]` = sourced from the decompile call site, `[U]` =
/// unsourced), per `docs/re/net/login-gateway.md` §11 (#462):
///
/// | opcode | call site | flag |
/// |---|---|---|
/// | `0x2001` | `004ce8c0:16` | 1 `[S]` |
/// | `0x6100` | arguments lost to Ghidra | 1 `[S]` |
/// | `0x6101` | `004c9410:11` | 1 `[S]` |
/// | `0x6102` | `004c9290:17` | 1 `[S]` |
/// | `0x6103` | `004ce9d0:104` | 1 `[S]` |
/// | `0x6106` | `004c9480:11` | 1 `[S]` |
/// | `0x6107` | none in the corpus | `[U]` |
///
/// `0x6107` has no builder and no immediate anywhere in the decompile corpus;
/// it comes from xBot `Security.cs:877-882` alone (`silkroad_security.cpp:214-218`
/// omits it). It is kept because it is harmless — we never send `0x6107` — but
/// it is *not* sourced. `0x2002`, `0x6323` and `0x5000`/`0x9000` are flagged `0`
/// at their call sites and stay out.
///
/// Inbound needs no equivalent: the client decrypts on the wire `0x8000` bit,
/// not an opcode set, which `parse` already does.
pub const ENCRYPTED_SEND_OPCODES: [u16; 7] =
    [0x2001, 0x6100, 0x6101, 0x6102, 0x6103, 0x6106, 0x6107];

/// **Gameplay** opcodes the original encrypts per packet, independently of the
/// login set above — and therefore not gated by the login-encryption config
/// flag, which exists only because our plaintext *login* is what the reference
/// server accepts today (#243).
///
/// Evidence: xBot's `PacketBuilder` constructs exactly two packets with
/// `new Packet(opcode, /* encrypted */ true)` — `CLIENT_CHARACTER_SELECTION_
/// ACTION_REQUEST` (0x7007) and `CLIENT_INVENTORY_ITEM_USE` (0x704C,
/// `PacketBuilder.cs:168,349`). Every other builder in that file, movement and
/// combat included, is plaintext. This is a *separate* mechanism from
/// `Security.cs`'s login list, and it went unnoticed here until a live vSRO
/// session refused a plaintext `0x704C` with `0xB04C 02 89 18` and then reset
/// the connection on the retry (`docs/net-item-use-0x704c.md`).
///
/// Only `0x704C` is listed: our plaintext `0x7007` demonstrably works against
/// the reference server today, so flipping it would risk the login flow to fix
/// nothing. Revisit if character selection ever starts failing.
pub const ENCRYPTED_GAMEPLAY_OPCODES: [u16; 1] = [0x704C];

impl SilkroadFrame {
    /// Apply the original's outbound encryption policy to this frame.
    ///
    /// Only flags the frame; [`SilkroadFrame::serialize`] does the actual
    /// Blowfish pass. Deliberately a no-op until the handshake produced a key:
    /// `serialize` panics on `encrypted == 1` with no Blowfish, and the
    /// identity/login opcodes are the first thing sent after the handshake.
    ///
    /// `login_opcodes` carries the `outbound_encryption` config flag; the
    /// gameplay set is always applied.
    pub fn apply_send_encryption(
        &mut self,
        security: &Arc<RwLock<SilkroadSecurityState>>,
        login_opcodes: bool,
    ) {
        let SilkroadFrame::Packet {
            opcode, encrypted, ..
        } = self
        else {
            return;
        };
        let wanted = ENCRYPTED_GAMEPLAY_OPCODES.contains(opcode)
            || (login_opcodes && ENCRYPTED_SEND_OPCODES.contains(opcode));
        if !wanted {
            return;
        }
        let key_ready = security
            .read()
            .map(|s| s.context.blowfish.is_some())
            .unwrap_or(false);
        if key_ready {
            *encrypted = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::blowfish::Blowfish;

    fn security(with_key: bool) -> Arc<RwLock<SilkroadSecurityState>> {
        let mut state = SilkroadSecurityState::new();
        if with_key {
            state.state = SilkroadSecurity::Established;
            state.context.blowfish = Some(Blowfish::new(&[0u8; 8]).expect("test key"));
        }
        Arc::new(RwLock::new(state))
    }

    fn packet(opcode: u16) -> SilkroadFrame {
        SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode,
            encrypted: 0,
            data: Bytes::from_static(&[1, 2, 3]),
        }
    }

    fn encrypted_flag(frame: &SilkroadFrame) -> u8 {
        match frame {
            SilkroadFrame::Packet { encrypted, .. } => *encrypted,
            _ => panic!("expected a Packet frame"),
        }
    }

    /// #243: the gateway login carries the account password, and the original
    /// encrypts it. We shipped it in the clear because `Into<SilkroadFrame>`
    /// hardcodes `encrypted: 0` and nothing ever raised it.
    #[test]
    fn the_login_request_is_flagged_for_encryption() {
        let security = security(true);

        let mut frame = packet(0x6102);
        frame.apply_send_encryption(&security, true);

        assert_eq!(encrypted_flag(&frame), 1);
        // ...and stays cleartext while the login opt-in is off.
        let mut frame = packet(0x6102);
        frame.apply_send_encryption(&security, false);
        assert_eq!(encrypted_flag(&frame), 0);
    }

    /// Item use is encrypted per packet by the original (xBot
    /// `PacketBuilder.cs:168,349`), independently of the login opt-in — a live
    /// vSRO server refused our plaintext `0x704C` and reset the connection on
    /// the retry.
    #[test]
    fn item_use_is_encrypted_without_the_login_opt_in() {
        let security = security(true);

        let mut frame = packet(0x704C);
        frame.apply_send_encryption(&security, false);

        assert_eq!(encrypted_flag(&frame), 1);
    }

    /// The original decides per opcode, so ordinary gameplay traffic must stay
    /// cleartext even with Blowfish negotiated — encrypting everything would
    /// drift just as far the other way.
    #[test]
    fn gameplay_traffic_stays_cleartext() {
        let security = security(true);

        let mut frame = packet(0x7021);
        frame.apply_send_encryption(&security, true);

        assert_eq!(encrypted_flag(&frame), 0);
    }

    /// #462: `0x6106` (shard-list ping) is allocated with the encrypt flag set
    /// at `004c9480:11`, and unlike the rest of the allowlist we actually send
    /// it — `poll_gateway_connection` fires it as soon as the shard list lands.
    #[test]
    fn the_shard_list_ping_is_flagged_for_encryption() {
        let security = security(true);

        let mut frame = packet(0x6106);
        frame.apply_send_encryption(&security, true);

        assert_eq!(encrypted_flag(&frame), 1);
    }

    /// The gateway phase is not uniformly encrypted: `0x6323` (`004c96a0:10`)
    /// and `0x2002` (`004cf990:13`) pass the flag as `0`, so an "encrypt the
    /// whole handshake" reading of the allowlist would be wrong.
    #[test]
    fn gateway_opcodes_flagged_zero_stay_cleartext() {
        let security = security(true);

        for opcode in [0x6323u16, 0x2002] {
            let mut frame = packet(opcode);
            frame.apply_send_encryption(&security, true);

            assert_eq!(encrypted_flag(&frame), 0, "opcode {opcode:#06x}");
        }
    }

    /// `serialize` panics on `encrypted == 1` with no Blowfish key, so the
    /// policy must stay inert until the handshake produced one.
    #[test]
    fn nothing_is_flagged_before_a_key_exists() {
        let security = security(false);

        let mut frame = packet(0x6102);
        frame.apply_send_encryption(&security, true);
        assert_eq!(encrypted_flag(&frame), 0);

        // The gameplay set is subject to the same rule.
        let mut frame = packet(0x704C);
        frame.apply_send_encryption(&security, false);
        assert_eq!(encrypted_flag(&frame), 0);
    }

    /// #449: `parse` computed `content_len` and then sliced to the end of the
    /// decrypted blowfish block, so every encrypted dump carried up to 7 bytes
    /// of padding that were never on the wire. Fixture is the real 23-byte
    /// `0xA102` redirect body from `packet_dump/0xa102.log`, whose dumped line
    /// is 28 bytes with five trailing zeros.
    #[test]
    fn encrypted_body_is_sliced_to_the_wire_length() {
        let security = security(true);
        let captured: &[u8] = &[
            0x01, 0x45, 0x00, 0x00, 0x00, 0x0e, 0x00, b'1', b'7', b'8', b'.', b'6', b'3', b'.',
            b'1', b'5', b'7', b'.', b'1', b'1', b'4', 0x0c, 0x3e,
        ];
        assert_ne!((captured.len() + 4) % 8, 0, "fixture must need padding");

        let frame = SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode: 0xA102,
            encrypted: 1,
            data: Bytes::copy_from_slice(captured),
        };
        let mut wire = frame
            .serialize(security.clone())
            .expect("serialize")
            .to_vec();

        let (_, parsed) = SilkroadFrame::parse(&mut wire, security).expect("parse");
        let SilkroadFrame::Packet { data, .. } = parsed else {
            panic!("expected a Packet frame");
        };
        assert_eq!(data.len(), captured.len());
        assert_eq!(&data[..], captured);
    }

    /// The same slice also swallowed whatever followed in the buffer, so two
    /// frames arriving in one read merged into one over-long body.
    #[test]
    fn a_following_frame_is_not_absorbed_into_the_body() {
        let security = security(false);
        let first = packet(0x2322)
            .serialize(security.clone())
            .expect("serialize")
            .to_vec();
        let second = packet(0xA101)
            .serialize(security.clone())
            .expect("serialize")
            .to_vec();
        let mut wire = [first, second].concat();

        let (_, parsed) = SilkroadFrame::parse(&mut wire, security).expect("parse");
        let SilkroadFrame::Packet { data, opcode, .. } = parsed else {
            panic!("expected a Packet frame");
        };
        assert_eq!(opcode, 0x2322);
        assert_eq!(&data[..], &[1, 2, 3]);
    }
}
