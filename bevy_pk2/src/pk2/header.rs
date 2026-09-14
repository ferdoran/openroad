use std::io::{Cursor, Read};

use bytes::{Buf, Bytes};

use crate::pk2::blowfish::Blowfish;
use crate::pk2::constants::{CHECKSUM, HEADER_SIZE, SIGNATURE, VERSION};
use crate::pk2::errors::Error;
use crate::pk2::errors::Error::InvalidHeader;
use crate::pk2::util::as_u32_le;

/// Represents the raw header of a PK2 archive, which is used to verify the integrity of an archive.
pub struct Header {
    signature: [u8; 30],
    version: u32,
    encrypted: bool,
    checksum: [u8; 16],
    reserved: [u8; 205],
}

impl From<[u8; HEADER_SIZE]> for Header {
    fn from(buf: [u8; HEADER_SIZE]) -> Self {
        let mut header = Header {
            signature: [0; 30],
            version: as_u32_le(&buf[30..34]),
            encrypted: buf[34] == 1,
            checksum: [0; 16],
            reserved: [0; 205],
        };

        header.signature.copy_from_slice(&buf[0..30]);
        header.checksum.copy_from_slice(&buf[35..51]);
        header.reserved.copy_from_slice(&buf[51..]);

        return header;
    }
}

impl From<&mut Cursor<Bytes>> for Header {
    fn from(buf: &mut Cursor<Bytes>) -> Self {
        let mut signature: [u8; 30] = [0; 30];
        let mut checksum: [u8; 16] = [0; 16];
        let mut reserved: [u8; 205] = [0; 205];

        // A short cursor leaves the remaining fields at their zero default,
        // which then fails the signature/checksum verification - the same
        // outcome as before, without taking the process down first.
        let _ = buf.read_exact(&mut signature);
        let version = if buf.remaining() >= 4 {
            buf.get_u32_le()
        } else {
            0
        };
        let encrypted = buf.remaining() >= 1 && buf.get_u8() == 1;
        let _ = buf.read_exact(&mut checksum);
        let _ = buf.read_exact(&mut reserved);

        Self {
            signature,
            version,
            encrypted,
            checksum,
            reserved,
        }
    }
}

impl From<&[u8]> for Header {
    fn from(header_buf: &[u8]) -> Self {
        // A short slice yields a zeroed header, which fails verification -
        // callers get an InvalidHeader error instead of a panic.
        let mut buf = [0; HEADER_SIZE];
        let take = header_buf.len().min(HEADER_SIZE);
        buf[..take].copy_from_slice(&header_buf[..take]);
        return Header::from(buf);
    }
}

impl Header {
    fn verify_checksum(&self, blowfish: &Blowfish) -> Result<(), Error> {
        if !&self.encrypted {
            return Ok(());
        }

        let mut encrypted_checksum = CHECKSUM.clone();
        blowfish.encrypt(&mut encrypted_checksum);

        for i in 0..3 {
            if encrypted_checksum[i] != self.checksum[i] {
                return Err(InvalidHeader("Checksum is invalid"));
            }
        }

        return Ok(());
    }

    fn verify_signature(&self) -> Result<(), Error> {
        if &self.signature != SIGNATURE {
            Err(InvalidHeader("Invalid signature"))
        } else if self.version != VERSION {
            Err(InvalidHeader("Invalid version"))
        } else {
            Ok(())
        }
    }

    pub fn verify(&self, blowfish: &Blowfish) -> Result<(), Error> {
        self.verify_signature()?;
        self.verify_checksum(blowfish)
    }
}
