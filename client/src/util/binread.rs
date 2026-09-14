//! Bounds-checked little-endian cursor for binary SRO format parsers (DOF,
//! AINavData). Unlike `bytes::Buf` (which panics on overrun), every read
//! returns a `Result` naming the field being read, so corpus probes can
//! report the offending field and offset instead of crashing.

use bevy::math::Vec3;
use thiserror::Error;

use crate::assets::textdata::decode::decode_textdata;

#[derive(Error, Debug)]
pub enum BinReadError {
    #[error("unexpected end of data at offset {at} (need {need} more bytes for {what})")]
    Eof {
        what: &'static str,
        at: usize,
        need: usize,
    },
    #[error("seek to {to:#x} for {what} is beyond data end ({len:#x})")]
    BadOffset {
        what: &'static str,
        to: u32,
        len: usize,
    },
    #[error("implausible {what} count {count} at offset {at}")]
    ImplausibleCount {
        what: &'static str,
        count: u32,
        at: usize,
    },
}

pub struct Cur<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> Cur<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Cur { data, pos: 0 }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn seek(&mut self, what: &'static str, to: u32) -> Result<(), BinReadError> {
        if to as usize > self.data.len() {
            return Err(BinReadError::BadOffset {
                what,
                to,
                len: self.data.len(),
            });
        }
        self.pos = to as usize;
        Ok(())
    }

    pub fn take(&mut self, what: &'static str, n: usize) -> Result<&'a [u8], BinReadError> {
        if self.remaining() < n {
            return Err(BinReadError::Eof {
                what,
                at: self.pos,
                need: n - self.remaining(),
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    /// Reject a count whose elements (at `min_elem_size` bytes each) cannot
    /// fit in the remaining data — guards `Vec::with_capacity` against
    /// corrupt files.
    pub fn plausible(
        &self,
        what: &'static str,
        count: u32,
        min_elem_size: usize,
    ) -> Result<(), BinReadError> {
        if (count as usize).saturating_mul(min_elem_size) > self.remaining() {
            return Err(BinReadError::ImplausibleCount {
                what,
                count,
                at: self.pos,
            });
        }
        Ok(())
    }

    pub fn u8(&mut self, what: &'static str) -> Result<u8, BinReadError> {
        Ok(self.take(what, 1)?[0])
    }

    pub fn u16(&mut self, what: &'static str) -> Result<u16, BinReadError> {
        let b = self.take(what, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self, what: &'static str) -> Result<i16, BinReadError> {
        let b = self.take(what, 2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self, what: &'static str) -> Result<u32, BinReadError> {
        let b = self.take(what, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32(&mut self, what: &'static str) -> Result<f32, BinReadError> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    pub fn vec3(&mut self, what: &'static str) -> Result<Vec3, BinReadError> {
        Ok(Vec3::new(self.f32(what)?, self.f32(what)?, self.f32(what)?))
    }

    /// u32-length CP949 string, trimmed at the first NUL.
    pub fn string_cp949(&mut self, what: &'static str) -> Result<String, BinReadError> {
        let len = self.u32(what)?;
        let bytes = self.take(what, len as usize)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(decode_textdata(&bytes[..end]))
    }

    /// `u32 count` followed by `count` u32 values.
    pub fn u32_indices(&mut self, what: &'static str) -> Result<Vec<u32>, BinReadError> {
        let count = self.u32(what)?;
        self.plausible(what, count, 4)?;
        let mut indices = Vec::with_capacity(count as usize);
        for _ in 0..count {
            indices.push(self.u32(what)?);
        }
        Ok(indices)
    }
}
